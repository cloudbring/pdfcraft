//! In-memory Office ZIP archives, backed by craft-archive.

use std::io::{self, Cursor};

use craft_archive::ZipWriter;

/// Builds a zip archive in memory.
pub struct Zip {
    writer: ZipWriter<Cursor<Vec<u8>>>,
    error: Option<io::Error>,
}

impl Default for Zip {
    fn default() -> Self {
        Self { writer: ZipWriter::in_memory(), error: None }
    }
}

impl Zip {
    /// Add a file. Already-compressed data (images) is stored; the rest is deflated.
    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        if self.error.is_some() {
            return;
        }
        // The old writer set UTF-8 for every name; craft-archive sets it only for non-ASCII names.
        let result = if compress { self.writer.add_deflated(name, data, 6) } else { self.writer.add_stored(name, data) };
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Finish with a recoverable error if any entry could not be written.
    pub fn try_finish(self) -> io::Result<Vec<u8>> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(self.writer.finish()?.into_inner())
    }

    /// Finish an archive using the legacy infallible export API.
    pub fn finish(self) -> Vec<u8> {
        self.try_finish().unwrap_or_else(|error| {
            eprintln!("Office ZIP export failed: {error}");
            Vec::new()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn old_crc32(data: &[u8]) -> u32 {
        static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
        let table = TABLE.get_or_init(|| {
            let mut table = [0u32; 256];
            for (index, entry) in table.iter_mut().enumerate() {
                let mut value = index as u32;
                for _ in 0..8 {
                    value = if value & 1 != 0 { 0xEDB8_8320 ^ (value >> 1) } else { value >> 1 };
                }
                *entry = value;
            }
            table
        });
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc = table[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    #[derive(Default)]
    struct OldZip {
        out: Vec<u8>,
        central: Vec<u8>,
        count: u16,
    }

    impl OldZip {
        fn add(&mut self, name: &str, data: &[u8], compress: bool) {
            let crc = old_crc32(data);
            let (method, body) = if compress {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(data).unwrap();
                (8u16, encoder.finish().unwrap())
            } else {
                (0u16, data.to_vec())
            };
            let offset = self.out.len() as u32;
            let header = |signature: u32, central: bool| {
                let mut header = Vec::new();
                header.extend(signature.to_le_bytes());
                if central {
                    header.extend(20u16.to_le_bytes());
                }
                header.extend(20u16.to_le_bytes());
                header.extend(0x0800u16.to_le_bytes());
                header.extend(method.to_le_bytes());
                header.extend([0, 0, 0x21, 0]);
                header.extend(crc.to_le_bytes());
                header.extend((body.len() as u32).to_le_bytes());
                header.extend((data.len() as u32).to_le_bytes());
                header.extend((name.len() as u16).to_le_bytes());
                header.extend(0u16.to_le_bytes());
                if central {
                    header.extend(0u16.to_le_bytes());
                    header.extend(0u16.to_le_bytes());
                    header.extend(0u16.to_le_bytes());
                    header.extend(0u32.to_le_bytes());
                    header.extend(offset.to_le_bytes());
                }
                header.extend(name.as_bytes());
                header
            };
            self.out.extend(header(0x0403_4b50, false));
            self.out.extend(&body);
            self.central.extend(header(0x0201_4b50, true));
            self.count += 1;
        }

        fn finish(mut self) -> Vec<u8> {
            let start = self.out.len() as u32;
            let size = self.central.len() as u32;
            self.out.append(&mut self.central);
            self.out.extend(0x0605_4b50u32.to_le_bytes());
            self.out.extend([0u8; 4]);
            self.out.extend(self.count.to_le_bytes());
            self.out.extend(self.count.to_le_bytes());
            self.out.extend(size.to_le_bytes());
            self.out.extend(start.to_le_bytes());
            self.out.extend(0u16.to_le_bytes());
            self.out
        }
    }

    #[test]
    fn crc_matches_the_reference() {
        assert_eq!(old_crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn archive_has_both_directories() {
        let mut z = Zip::default();
        z.add("a.txt", b"hello hello hello", true);
        z.add("b.png", b"\x89PNG", false);
        let b = z.finish();
        assert!(b.starts_with(b"PK\x03\x04"));
        assert_eq!(b.windows(4).filter(|w| *w == b"PK\x01\x02").count(), 2);
        assert_eq!(&b[b.len() - 22..b.len() - 18], b"PK\x05\x06");
    }

    #[test]
    fn shared_writer_matches_old_archive_entries_and_central_directory() {
        let entries: &[(&str, &[u8], bool)] = &[
            ("[Content_Types].xml", b"<Types>hello hello hello</Types>", true),
            ("word/media/image1.png", b"\x89PNG\r\n\x1a\n", false),
            ("word/caf\u{e9}.xml", b"<x>Unicode</x>", true),
        ];
        let mut old = OldZip::default();
        let mut new = Zip::default();
        for &(name, data, compress) in entries {
            old.add(name, data, compress);
            new.add(name, data, compress);
        }
        let old_bytes = old.finish();
        let new_bytes = new.try_finish().unwrap();
        let mut old_archive = zip::ZipArchive::new(Cursor::new(old_bytes)).unwrap();
        let mut new_archive = zip::ZipArchive::new(Cursor::new(new_bytes)).unwrap();
        assert_eq!(old_archive.len(), new_archive.len());
        for &(name, bytes, compress) in entries {
            let mut old_file = old_archive.by_name(name).unwrap();
            let mut new_file = new_archive.by_name(name).unwrap();
            let method = if compress { zip::CompressionMethod::Deflated } else { zip::CompressionMethod::Stored };
            assert_eq!(old_file.name().unwrap(), new_file.name().unwrap());
            assert_eq!(old_file.compression(), method);
            assert_eq!(new_file.compression(), method);
            assert_eq!(old_file.size(), new_file.size());
            assert_eq!(old_file.compressed_size(), new_file.compressed_size());
            assert_eq!(old_file.crc32(), new_file.crc32());
            let mut old_data = Vec::new();
            let mut new_data = Vec::new();
            old_file.read_to_end(&mut old_data).unwrap();
            new_file.read_to_end(&mut new_data).unwrap();
            assert_eq!(old_data, bytes);
            assert_eq!(new_data, old_data);
        }
    }
}
