use craft_pdf::filters::{self as shared, FilterChain, Limits, Mode};
use pdfcraft_filters::{Filter, Params, decode, decode_tolerant, encode};

fn decoded(filter: Filter, bytes: &[u8]) -> Vec<u8> {
    decode(&[(filter.clone(), Params::default())], bytes, 1 << 20).unwrap_or_else(|error| panic!("{filter:?} {bytes:?}: {error}"))
}

#[test]
fn legacy_ascii_and_runlength_vectors() {
    for (filter, encoded, plain) in [
        (Filter::AsciiHex, b"48656c6C6F>".as_slice(), b"Hello".as_slice()),
        (Filter::AsciiHex, b"901FA".as_slice(), &[0x90, 0x1f, 0xa0]),
        (Filter::Ascii85, b"9jqo^~>".as_slice(), b"Man ".as_slice()),
        (Filter::Ascii85, b"9jqo^~".as_slice(), b"Man ".as_slice()),
        (Filter::Ascii85, b"<~9jqo^~>".as_slice(), b"Man ".as_slice()),
        (Filter::RunLength, &[2, b'a', b'b', b'c', 254, b'x', 128], b"abcxxx".as_slice()),
        (Filter::RunLength, &[0, b'q'], b"q".as_slice()),
    ] {
        assert_eq!(decoded(filter, encoded), plain);
    }
    for (filter, plain, encoded) in [
        (Filter::AsciiHex, b"Hello".as_slice(), b"48656C6C6F>".as_slice()),
        (Filter::Ascii85, b"Man ".as_slice(), b"9jqo^~>".as_slice()),
        (Filter::RunLength, b"abcxxx".as_slice(), &[2, b'a', b'b', b'c', 254, b'x', 128]),
    ] {
        assert_eq!(encode(&filter, &Params::default(), plain).unwrap(), encoded);
    }
}

#[test]
fn legacy_flate_checksum_and_raw_vectors() {
    let zlib = [0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o', 0x06, 0x2c, 0x02, 0x15];
    for encoded in [&zlib[..], &zlib[..12]] {
        assert_eq!(decoded(Filter::Flate, encoded), b"hello");
    }
    let mut broken = zlib;
    broken[15] ^= 0xff;
    assert_eq!(decoded(Filter::Flate, &broken), b"hello");
    let mut trailing = zlib.to_vec();
    trailing.extend_from_slice(b"\r\nendstream");
    assert_eq!(decoded(Filter::Flate, &trailing), b"hello");
}

#[test]
fn lzw_spec_vector_and_weezl_cross_check() {
    let plain = [45, 45, 45, 45, 45, 65, 45, 45, 45, 66];
    let expected = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
    assert_eq!(decoded(Filter::Lzw, &expected), plain);
    assert_eq!(encode(&Filter::Lzw, &Params::default(), &plain).unwrap(), expected);
    let input: Vec<u8> = (0..150_000u32).map(|index| ((index * 31) ^ (index / 101)) as u8).collect();
    for early_change in [0, 1] {
        let params = Params { early_change, ..Params::default() };
        let encoded = encode(&Filter::Lzw, &params, &input).unwrap();
        assert_eq!(decode(&[(Filter::Lzw, params)], &encoded, input.len()).unwrap(), input);
        let mut weezl = if early_change == 1 {
            weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
        } else {
            weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8)
        };
        assert_eq!(weezl.decode(&encoded).unwrap(), input);
    }
}

#[test]
fn tolerant_chain_keeps_partial_data() {
    let data = b"one two three four five".repeat(100);
    let mut encoded = encode(&Filter::Flate, &Params::default(), &data).unwrap();
    encoded.truncate(encoded.len() / 2);
    let (partial, incomplete) = decode_tolerant(&[(Filter::Flate, Params::default())], &encoded, data.len()).unwrap();
    assert!(incomplete);
    assert_eq!(partial, data[..partial.len()]);
}

#[test]
fn partial_png_optimum_matches_legacy_encoded_bytes() {
    let data = [10, 20, 30, 40, 10, 20];
    let params = Params { predictor: 15, columns: 4, ..Params::default() };
    let predicted = [1, 10, 10, 10, 10, 2, 0, 0];
    let expected = shared::encode(shared::Filter::Flate, &predicted, shared::EncodeParams::default()).unwrap();
    let encoded = encode(&Filter::Flate, &params, &data).unwrap();
    assert_eq!(encoded, expected);
    let chain = [(Filter::Flate, params)];
    assert_eq!(decode(&chain, &encoded, data.len() + 2).unwrap(), data);
    let (tolerant, incomplete) = decode_tolerant(&chain, &encoded, data.len() + 2).unwrap();
    assert_eq!(tolerant, data);
    assert!(!incomplete);
    let raw = FilterChain {
        filters: vec![shared::Filter::Flate],
        limits: Limits { max_output: 32, max_input: u64::MAX, max_ratio: u64::MAX, max_depth: 1 },
        mode: Mode::Strict,
    };
    assert_eq!(raw.decode(&encoded).unwrap().data, predicted);
}

#[test]
fn partial_tiff_row_matches_legacy_encoded_bytes() {
    let data = [10, 20, 30, 40, 5, 7];
    let predicted = [10, 10, 10, 10, 5, 2];
    let params = Params { predictor: 2, columns: 4, ..Params::default() };
    for (filter, plain_stage) in [(Filter::Flate, shared::Filter::Flate), (Filter::Lzw, shared::Filter::Lzw { early_change: true })] {
        let expected = shared::encode(plain_stage, &predicted, shared::EncodeParams::default()).unwrap();
        let encoded = encode(&filter, &params, &data).unwrap();
        assert_eq!(encoded, expected, "{filter:?}");
        let chain = [(filter, params.clone())];
        assert_eq!(decode(&chain, &encoded, data.len()).unwrap(), data);
        assert_eq!(decode_tolerant(&chain, &encoded, data.len()).unwrap(), (data.to_vec(), false));
    }
}

#[test]
fn simple_filters_ignore_predictor_parameters() {
    let params = Params { predictor: i64::MIN, columns: -1, colors: -1, ..Params::default() };
    for filter in [Filter::AsciiHex, Filter::Ascii85, Filter::RunLength] {
        let encoded = encode(&filter, &params, b"sample").unwrap();
        assert_eq!(decode(&[(filter, params.clone())], &encoded, 6).unwrap(), b"sample");
    }
}

#[test]
fn tolerant_png_preserves_rows_before_an_invalid_tag() {
    let params = Params { predictor: 12, columns: 4, ..Params::default() };
    let predicted = [0, 1, 2, 3, 4, 5, 1, 1, 1, 1];
    let encoded = shared::encode(shared::Filter::Flate, &predicted, shared::EncodeParams::default()).unwrap();
    let chain = [(Filter::Flate, params)];
    assert!(decode(&chain, &encoded, 64).is_err());
    assert_eq!(decode_tolerant(&chain, &encoded, 64).unwrap(), (vec![1, 2, 3, 4], true));
}
