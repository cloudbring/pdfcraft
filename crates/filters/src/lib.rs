//! PDF stream filters (ISO 32000-2 §7.4).
//!
//! Layer L0, standalone: this crate depends on no other PdfCraft crate.
//!
//! Decoding covers the general-purpose filters (`FlateDecode`, `LZWDecode` with
//! TIFF/PNG predictors, `ASCIIHexDecode`, `ASCII85Decode`, `RunLengthDecode`).
//! Image codecs (`DCTDecode`, `JPXDecode`, `JBIG2Decode`, `CCITTFaxDecode`),
//! `Crypt` and unknown filters are reported as [`FilterError::Unsupported`] so
//! callers keep such data encoded. CCITT fax and JBIG2 images decode on their own
//! ([`decode_ccitt_within`], [`decode_jbig2_within`]), each spending from an allowance its caller
//! shares across a run of images, as `pdfcraft_cos::ImageBudget::decode` does after the filters
//! before them. Neither decodes without one:
//!
//! ```compile_fail
//! let _ = pdfcraft_filters::decode_jbig2(&[], None, 1, 1, 1 << 20);
//! ```
//!
//! ```compile_fail
//! let _ = pdfcraft_filters::decode_ccitt(&[], &pdfcraft_filters::CcittParams::default(), 1);
//! ```
//!
//! ```
//! let mut work = 1 << 20;
//! assert!(pdfcraft_filters::decode_jbig2_within(&[], None, 1, 1, 1 << 20, &mut work).is_err());
//! let p = pdfcraft_filters::CcittParams { columns: 8, ..Default::default() };
//! assert!(pdfcraft_filters::decode_ccitt_within(&[], &p, 1, &mut work).is_ok());
//! ```
//!
//! 1-bit samples can be encoded as CCITT Group 4 ([`encode_g4`]) or lossless JBIG2
//! ([`encode_jbig2`]).
//!
//! Every decoder is bounded by a caller-supplied `max_output` (decompression-bomb
//! defence) and never panics on malformed input.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod ccitt;
mod jbig2;
mod mq;

use craft_pdf::filters::{self as shared, FilterChain, FlateWrapper, Limits, Mode};

pub use ccitt::{CcittParams, MAX_CCITT_COLUMNS, MAX_G4_COLUMNS, decode_ccitt_within, encode_g4};
pub use jbig2::{MAX_JBIG2_COLUMNS, MAX_JBIG2_PIXELS, decode_jbig2_within, encode_jbig2, encode_jbig2_generic};

/// A PDF stream filter, identified by its `/Filter` name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Flate,
    Lzw,
    AsciiHex,
    Ascii85,
    RunLength,
    Dct,
    Jpx,
    Jbig2,
    CcittFax,
    Crypt,
    /// Any other name (kept verbatim, lossily converted to UTF-8).
    Unknown(String),
}

impl Filter {
    /// Parses a filter name. Accepts the full names (ISO 32000-2 Table 6) and the
    /// inline-image abbreviations (Table 92): `Fl`, `LZW`, `AHx`, `A85`, `RL`, `DCT`, `CCF`.
    /// A leading `/` is ignored.
    pub fn from_name(name: &[u8]) -> Filter {
        let n = name.strip_prefix(b"/").unwrap_or(name);
        match n {
            b"FlateDecode" | b"Fl" => Filter::Flate,
            b"LZWDecode" | b"LZW" => Filter::Lzw,
            b"ASCIIHexDecode" | b"AHx" => Filter::AsciiHex,
            b"ASCII85Decode" | b"A85" => Filter::Ascii85,
            b"RunLengthDecode" | b"RL" => Filter::RunLength,
            b"DCTDecode" | b"DCT" => Filter::Dct,
            b"JPXDecode" => Filter::Jpx,
            b"JBIG2Decode" => Filter::Jbig2,
            b"CCITTFaxDecode" | b"CCF" => Filter::CcittFax,
            b"Crypt" => Filter::Crypt,
            other => Filter::Unknown(String::from_utf8_lossy(other).into_owned()),
        }
    }

    /// The full PDF name (without the leading `/`).
    pub fn name(&self) -> &str {
        match self {
            Filter::Flate => "FlateDecode",
            Filter::Lzw => "LZWDecode",
            Filter::AsciiHex => "ASCIIHexDecode",
            Filter::Ascii85 => "ASCII85Decode",
            Filter::RunLength => "RunLengthDecode",
            Filter::Dct => "DCTDecode",
            Filter::Jpx => "JPXDecode",
            Filter::Jbig2 => "JBIG2Decode",
            Filter::CcittFax => "CCITTFaxDecode",
            Filter::Crypt => "Crypt",
            Filter::Unknown(s) => s,
        }
    }

    /// True for the image codecs (DCT, JPX, JBIG2, CCITT fax).
    pub fn is_image_codec(&self) -> bool {
        matches!(self, Filter::Dct | Filter::Jpx | Filter::Jbig2 | Filter::CcittFax)
    }

    /// True for the general-purpose filters, which a chain can hold ([`decode`]).
    pub fn is_decodable(&self) -> bool {
        matches!(self, Filter::Flate | Filter::Lzw | Filter::AsciiHex | Filter::Ascii85 | Filter::RunLength)
    }
}

/// `/DecodeParms` values relevant to the general-purpose filters (ISO 32000-2 Tables 8–9).
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// 1 = none, 2 = TIFF predictor 2, 10–15 = PNG predictors.
    pub predictor: i64,
    pub colors: i64,
    pub bits_per_component: i64,
    pub columns: i64,
    /// LZW only: 1 (default) switches code width one code early, 0 does not.
    pub early_change: i64,
}

impl Default for Params {
    fn default() -> Self {
        Params { predictor: 1, colors: 1, bits_per_component: 8, columns: 1, early_change: 1 }
    }
}

/// Errors from decoding or encoding.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("{filter}: corrupt data: {detail}")]
    Corrupt { filter: &'static str, detail: String },
    #[error("decoded output exceeds the limit of {0} bytes")]
    LimitExceeded(usize),
    #[error("unsupported filter: {0}")]
    Unsupported(String),
    /// A run of images has nothing left to spend ([`decode_tolerant_within`]).
    #[error("more image decoding than one operation may do")]
    Spent,
}

/// Takes `steps` from `work`, what a run of images may still cost. What it can't pay for spends
/// all of it.
pub(crate) fn pay(work: &mut u64, steps: u64) -> Result<(), FilterError> {
    match work.checked_sub(steps) {
        Some(left) => {
            *work = left;
            Ok(())
        }
        None => {
            *work = 0;
            Err(FilterError::Spent)
        }
    }
}

/// Decodes `data` through `chain` (first element applied first).
///
/// Image codecs, `Crypt` and unknown filters anywhere in the chain give
/// [`FilterError::Unsupported`] (callers keep such data encoded), as does a chain longer than
/// [`MAX_CHAIN`]. `max_output` bounds every intermediate buffer.
pub fn decode(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    run(chain, data, max_output, None, true).map(|(v, _, _)| v).map_err(|(e, _)| e)
}

/// Like [`decode`] for a caller charging an allowance: `max_output` bounds the bytes all stages
/// decode together, each stage getting what the earlier ones left, and the result gives those
/// bytes, on success and on failure. A stage's output counts even when a later stage or its
/// predictor shrinks it.
pub fn decode_counted(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, usize), (FilterError, usize)> {
    run(chain, data, max_output, Some(max_output), true).map(|(v, _, spent)| (v, spent))
}

/// Like [`decode`], but on corrupt data returns what was decoded before the error and
/// reports `partial = true` (the behaviour viewers expect). The remaining filters of the
/// chain still run on the partial data. Limit and unsupported errors are still errors.
pub fn decode_tolerant(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, bool), FilterError> {
    run(chain, data, max_output, None, false).map(|(v, partial, _)| (v, partial)).map_err(|(e, _)| e)
}

/// [`decode_tolerant`] bounded and counted as [`decode_counted`] is.
pub fn decode_tolerant_counted(chain: &[(Filter, Params)], data: &[u8], max_output: usize) -> Result<(Vec<u8>, bool, usize), (FilterError, usize)> {
    run(chain, data, max_output, Some(max_output), false)
}

/// [`decode_tolerant`], spending from `work`, what decoding may still cost across a run of images:
/// a step for each byte of `data`, paid before anything is decoded, and one for each byte a stage
/// decodes, so a chain that fails, or a stage that goes past `max_output`, has paid for what it did.
/// No stage decodes past what is left: a chain that `work` can't pay for spends all of it and stops
/// there, so with nothing left nothing is read. A chain that can't be decoded at all costs nothing,
/// whatever is left, so what an image's filters are is known from the error.
///
/// Unlike [`decode_tolerant`], no filters at all gives `data` itself, within `max_output`.
pub fn decode_tolerant_within(chain: &[(Filter, Params)], data: &[u8], max_output: usize, work: &mut u64) -> Result<Vec<u8>, FilterError> {
    spending(chain, data, max_output, work, false)
}

/// [`decode`], spending from `work` as [`decode_tolerant_within`] does: corrupt data is an error,
/// having paid for what was decoded before it.
pub fn decode_within(chain: &[(Filter, Params)], data: &[u8], max_output: usize, work: &mut u64) -> Result<Vec<u8>, FilterError> {
    spending(chain, data, max_output, work, true)
}

fn spending(chain: &[(Filter, Params)], data: &[u8], max_output: usize, work: &mut u64, strict: bool) -> Result<Vec<u8>, FilterError> {
    decodable(chain)?;
    pay(work, data.len() as u64)?;
    if chain.is_empty() {
        return if data.len() > max_output { Err(FilterError::LimitExceeded(max_output)) } else { Ok(data.to_vec()) };
    }
    let left = usize::try_from(*work).unwrap_or(usize::MAX);
    match run(chain, data, max_output, Some(left), strict) {
        Ok((out, _, decoded)) => pay(work, decoded as u64).map(|()| out),
        // The stage stopped at the lower of the two, which took as much to find out.
        Err((FilterError::LimitExceeded(_), decoded)) if left.saturating_sub(decoded) > 0 => {
            pay(work, decoded as u64)?;
            Err(FilterError::LimitExceeded(max_output))
        }
        Err((FilterError::LimitExceeded(_), _)) => {
            *work = 0;
            Err(FilterError::Spent)
        }
        Err((e, decoded)) => pay(work, decoded as u64).and(Err(e)),
    }
}

/// Longest filter chain decoded. Real streams use one to three filters; every stage may produce
/// up to the output limit, so the chain length multiplies the work.
pub const MAX_CHAIN: usize = 8;

/// Whether a chain is one [`run`] takes, and if not, the error.
fn decodable(chain: &[(Filter, Params)]) -> Result<(), FilterError> {
    if chain.len() > MAX_CHAIN {
        return Err(FilterError::Unsupported(format!("a chain of {} filters", chain.len())));
    }
    match chain.iter().find(|(f, _)| !f.is_decodable()) {
        Some((f, _)) => Err(FilterError::Unsupported(f.name().to_owned())),
        None => Ok(()),
    }
}

/// Translate PDF parameters before asking the shared codec to allocate anything.
fn shared_filter(filter: &Filter, params: &Params) -> Result<shared::Filter, FilterError> {
    let predictor = if params.predictor == 1 || !matches!(filter, Filter::Flate | Filter::Lzw) {
        None
    } else {
        Some(shared::Predictor {
            predictor: params.predictor as u8,
            colors: params.colors as usize,
            bits_per_component: params.bits_per_component as u8,
            columns: usize::try_from(params.columns)
                .map_err(|_| FilterError::Corrupt { filter: filter.name_static(), detail: "Columns too large".into() })?,
        })
    };
    Ok(match filter {
        Filter::Flate => shared::Filter::FlateWithWrapper { wrapper: FlateWrapper::Auto, predictor },
        Filter::Lzw => match predictor {
            Some(predictor) => shared::Filter::LzwPredictor { early_change: params.early_change != 0, predictor },
            None => shared::Filter::Lzw { early_change: params.early_change != 0 },
        },
        Filter::AsciiHex => shared::Filter::AsciiHex,
        Filter::Ascii85 => shared::Filter::Ascii85,
        Filter::RunLength => shared::Filter::RunLength,
        _ => return Err(FilterError::Unsupported(filter.name().to_owned())),
    })
}

impl Filter {
    fn name_static(&self) -> &'static str {
        match self {
            Self::Flate => "FlateDecode",
            Self::Lzw => "LZWDecode",
            _ => "stream filter",
        }
    }
}

fn shared_error(error: shared::Error, filter: &Filter, max: usize) -> FilterError {
    match error {
        shared::Error::OutputLimit => FilterError::LimitExceeded(max),
        shared::Error::DepthLimit | shared::Error::Unsupported(_) => FilterError::Unsupported(filter.name().to_owned()),
        other => FilterError::Corrupt { filter: filter.name_static(), detail: format!("{other:?}") },
    }
}

/// A corrupt raw block can fail before the shared codec's 8 KiB read returns its
/// produced bytes. Probe the bounded codec at smaller ceilings to charge those bytes.
fn failed_raw_output(raw: &FilterChain, input: &[u8], limit: usize) -> u64 {
    let (mut lower, mut upper) = (0usize, limit);
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let mut probe = raw.clone();
        probe.limits.max_output = middle as u64;
        if matches!(probe.decode(input), Err(shared::Error::OutputLimit)) {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    // A corrupt stored block can have one final byte buffered when the shared
    // inflater reports the error instead of returning its last read chunk.
    (lower as u64).saturating_add(u64::from(lower > 0)).min(limit as u64)
}

/// Every stage uses the shared bounded chain; an optional total also caps cumulative output.
fn run(
    chain: &[(Filter, Params)],
    data: &[u8],
    max: usize,
    total: Option<usize>,
    strict: bool,
) -> Result<(Vec<u8>, bool, usize), (FilterError, usize)> {
    decodable(chain).map_err(|e| (e, 0))?;
    let mut cur = data.to_vec();
    let mut partial = false;
    let mut spent = 0usize;
    for (filter, params) in chain {
        let limit = total.map_or(max, |total| max.min(total.saturating_sub(spent)));
        let invalid_predictor = invalid_predictor(filter, params);
        let plain = Params { predictor: 1, ..params.clone() };
        let shared_filter = shared_filter(filter, if invalid_predictor { &plain } else { params }).map_err(|error| (error, spent))?;
        let input = input_for_shared(filter, &cur);
        let has_predictor =
            matches!(shared_filter, shared::Filter::FlateWithWrapper { predictor: Some(_), .. } | shared::Filter::LzwPredictor { .. });
        let decoder = FilterChain {
            filters: vec![shared_filter],
            limits: Limits { max_output: limit as u64, max_input: u64::MAX, max_ratio: u64::MAX, max_depth: MAX_CHAIN },
            mode: if strict && !has_predictor { Mode::Strict } else { Mode::Tolerant },
        };
        let (mut result, mut accounting) = decoder.decode_with_accounting(&input);
        // A valid-looking zlib header may belong to raw DEFLATE. The shared Auto
        // framing intentionally does not retry; retain pdfcraft's legacy fallback here.
        if matches!(filter, Filter::Flate) && matches!(result, Err(shared::Error::Corrupt | shared::Error::Truncated)) {
            let initial = accounting.total;
            let mut raw = decoder.clone();
            raw.filters = vec![match shared_filter {
                shared::Filter::FlateWithWrapper { predictor, .. } => shared::Filter::FlateWithWrapper { wrapper: FlateWrapper::Raw, predictor },
                _ => shared::Filter::FlateWithWrapper { wrapper: FlateWrapper::Raw, predictor: None },
            }];
            if initial == 0 {
                let (retry, progress) = raw.decode_with_accounting(&cur);
                accounting.total = accounting.total.max(progress.total);
                if retry.is_ok() || matches!(retry, Err(shared::Error::OutputLimit)) {
                    result = retry;
                    accounting = progress;
                } else {
                    accounting.total = accounting.total.max(failed_raw_output(&raw, &cur, limit));
                    if has_zlib_header(&cur) {
                        let (stripped, progress) = raw.decode_with_accounting(&cur[2..]);
                        if stripped.is_ok() {
                            result = stripped;
                            accounting = progress;
                        }
                    }
                    if !strict && result.is_err() {
                        partial = true;
                        result = Ok(shared::Decoded { data: Vec::new(), stages: Vec::new() });
                    }
                }
            } else if has_zlib_header(&cur) {
                let (retry, progress) = raw.decode_with_accounting(&cur[2..]);
                if retry.is_ok() {
                    result = retry;
                    accounting = progress;
                }
            }
        }
        if !strict
            && has_predictor
            && matches!(result, Err(shared::Error::Corrupt))
            && let Some(prefix) = recover_png_prefix(filter, params, &input, limit)
        {
            result = Ok(shared::Decoded { data: prefix, stages: Vec::new() });
            partial = true;
        }
        spent = spent.saturating_add(usize::try_from(accounting.total).unwrap_or(usize::MAX));
        let decoded = result.map_err(|error| {
            let error = shared_error(error, filter, max);
            (error, spent)
        })?;
        let incomplete = decoded.stages.iter().any(|stage| stage.incomplete);
        if incomplete && has_predictor {
            let plain_filter = match filter {
                Filter::Flate => shared::Filter::FlateWithWrapper { wrapper: FlateWrapper::Auto, predictor: None },
                Filter::Lzw => shared::Filter::Lzw { early_change: params.early_change != 0 },
                _ => shared_filter,
            };
            let check = FilterChain { filters: vec![plain_filter], mode: Mode::Strict, ..decoder.clone() };
            if let Err(error) = check.decode(&input) {
                if strict {
                    return Err((shared_error(error, filter, max), spent));
                }
                partial = true;
            }
        } else {
            partial |= incomplete;
        }
        cur = if invalid_predictor {
            if strict {
                return Err((FilterError::Corrupt { filter: filter.name_static(), detail: "invalid predictor parameters".into() }, spent));
            }
            partial = true;
            Vec::new()
        } else {
            decoded.data
        };
    }
    Ok((cur, partial, spent))
}

fn has_zlib_header(data: &[u8]) -> bool {
    let [cmf, flg, ..] = data else { return false };
    cmf & 15 == 8 && cmf >> 4 <= 7 && u16::from_be_bytes([*cmf, *flg]) % 31 == 0
}

/// Add the implicit end markers and strip optional ASCII85 framing accepted by pdfcraft.
fn input_for_shared(filter: &Filter, data: &[u8]) -> Vec<u8> {
    let mut input = data.to_vec();
    match filter {
        Filter::AsciiHex if !input.contains(&b'>') => input.push(b'>'),
        Filter::Ascii85 if !input.windows(2).any(|pair| pair == b"~>") => {
            if input.last() == Some(&b'~') {
                input.pop();
            }
            input.extend_from_slice(b"~>");
        }
        Filter::RunLength if !input.contains(&128) => input.push(128),
        _ => {}
    }
    if matches!(filter, Filter::Ascii85) {
        let start = input.iter().position(|byte| !matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)).unwrap_or(input.len());
        if input.get(start..start.saturating_add(2)) == Some(b"<~") {
            input.drain(start..start + 2);
        }
    }
    input
}

fn invalid_predictor(filter: &Filter, params: &Params) -> bool {
    matches!(filter, Filter::Flate | Filter::Lzw)
        && params.predictor != 1
        && (!matches!(params.predictor, 2 | 10..=15)
            || !(1..=255).contains(&params.colors)
            || !matches!(params.bits_per_component, 1 | 2 | 4 | 8 | 16)
            || params.columns < 1)
}

/// Preserve completed PNG rows when a later predictor tag is invalid. The shared
/// chain owns both codec and predictor; the shim only selects its valid prefix.
fn recover_png_prefix(filter: &Filter, params: &Params, input: &[u8], limit: usize) -> Option<Vec<u8>> {
    if !matches!(params.predictor, 10..=15) {
        return None;
    }
    let row = predictor_row(params, filter).ok()??.checked_add(1)?;
    let plain_stage = shared_filter(filter, &Params { predictor: 1, ..params.clone() }).ok()?;
    let limits = Limits { max_output: limit as u64, max_input: u64::MAX, max_ratio: u64::MAX, max_depth: 1 };
    let plain = FilterChain { filters: vec![plain_stage], limits, mode: Mode::Strict }.decode(input).ok()?;
    let bad_row = plain.data.chunks(row).position(|chunk| chunk.first().is_some_and(|tag| *tag > 4))?;
    let prefix = plain.data.get(..bad_row.checked_mul(row)?)?;
    let encoded = shared::encode(plain_stage, prefix, shared::EncodeParams::default()).ok()?;
    let predictor_stage = shared_filter(filter, params).ok()?;
    FilterChain { filters: vec![predictor_stage], limits, mode: Mode::Tolerant }.decode(&encoded).ok().map(|decoded| decoded.data)
}

/// Encodes `data` with `filter`. Supported: Flate (zlib, level 6), LZW, ASCIIHex,
/// ASCII85, RunLength. For Flate and LZW, `params.predictor >= 2` applies the
/// predictor first (TIFF 2, PNG 10–15; 15 chooses the per-row optimum).
pub fn encode(filter: &Filter, params: &Params, data: &[u8]) -> Result<Vec<u8>, FilterError> {
    if !filter.is_decodable() {
        return Err(FilterError::Unsupported(format!("encoding with {}", filter.name())));
    }
    if invalid_predictor(filter, params) {
        return Err(FilterError::Unsupported(format!("Predictor {}", params.predictor)));
    }
    let stage = shared_filter(filter, params)?;
    let Some(row) = predictor_row(params, filter)? else {
        return shared::encode(stage, data, shared::EncodeParams::default()).map_err(|error| shared_error(error, filter, data.len()));
    };
    if data.len().is_multiple_of(row) {
        return shared::encode(stage, data, shared::EncodeParams::default()).map_err(|error| shared_error(error, filter, data.len()));
    }
    let padding = row - data.len() % row;
    if padding > 1 << 20 {
        return Err(FilterError::Unsupported("partial predictor row exceeds compatibility padding limit".into()));
    }
    let padded_len = data.len().checked_add(padding).ok_or_else(|| FilterError::Unsupported("predictor row too large".into()))?;
    let mut padded = data.to_vec();
    padded.resize(padded_len, 0);
    let encoded = shared::encode(stage, &padded, shared::EncodeParams::default()).map_err(|error| shared_error(error, filter, data.len()))?;
    let plain_stage = shared_filter(filter, &Params { predictor: 1, ..params.clone() })?;
    let decoded = FilterChain {
        filters: vec![plain_stage],
        limits: Limits {
            max_output: (padded.len() as u64).saturating_add(padded.len().div_ceil(row) as u64),
            max_input: u64::MAX,
            max_ratio: u64::MAX,
            max_depth: 1,
        },
        mode: Mode::Strict,
    }
    .decode(&encoded)
    .map_err(|error| shared_error(error, filter, data.len()))?;
    let mut predicted = decoded.data;
    predicted.truncate(predicted.len().saturating_sub(padding));
    if params.predictor == 2
        && params.bits_per_component == 16
        && !data.len().is_multiple_of(2)
        && let (Some(last), Some(original)) = (predicted.last_mut(), data.last())
    {
        *last = *original;
    }
    if params.predictor == 15 {
        let tail = data.len() % row + 1;
        let prior = data.len() - data.len() % row;
        let start = prior.saturating_sub(row);
        let tail_data = &padded[start..];
        let mut best = None;
        for predictor in 10..=14 {
            let candidate = shared_filter(filter, &Params { predictor, ..params.clone() })?;
            let candidate_encoded =
                shared::encode(candidate, tail_data, shared::EncodeParams::default()).map_err(|error| shared_error(error, filter, data.len()))?;
            let candidate_decoded = FilterChain {
                filters: vec![plain_stage],
                limits: Limits { max_output: tail_data.len() as u64 + 2, max_input: u64::MAX, max_ratio: u64::MAX, max_depth: 1 },
                mode: Mode::Strict,
            }
            .decode(&candidate_encoded)
            .map_err(|error| shared_error(error, filter, data.len()))?;
            let row_start = candidate_decoded.data.len().saturating_sub(row + 1);
            let candidate_row = candidate_decoded
                .data
                .get(row_start..row_start + tail)
                .ok_or_else(|| FilterError::Corrupt { filter: filter.name_static(), detail: "short predictor row".into() })?;
            let cost: u64 = candidate_row[1..].iter().map(|byte| u64::from((*byte as i8).unsigned_abs())).sum();
            if best.as_ref().is_none_or(|(best_cost, _)| cost < *best_cost) {
                best = Some((cost, candidate_row.to_vec()));
            }
        }
        if let Some((_, row)) = best {
            let tail_start = predicted.len() - tail;
            let suffix = predicted
                .get_mut(tail_start..)
                .ok_or_else(|| FilterError::Corrupt { filter: filter.name_static(), detail: "short predictor row".into() })?;
            suffix.copy_from_slice(&row);
        }
    }
    shared::encode(plain_stage, &predicted, shared::EncodeParams::default()).map_err(|error| shared_error(error, filter, data.len()))
}

/// The shared encoder needs full rows; pdfcraft historically also writes final partial rows.
/// The caller pads only that row and strips the encoded padding before compression.
fn predictor_row(params: &Params, filter: &Filter) -> Result<Option<usize>, FilterError> {
    if !matches!(filter, Filter::Flate | Filter::Lzw) || params.predictor == 1 {
        return Ok(None);
    }
    let bits = usize::try_from(params.colors).ok().and_then(|colors| colors.checked_mul(params.bits_per_component as usize));
    let bytes = bits
        .and_then(|bits| usize::try_from(params.columns).ok().and_then(|columns| bits.checked_mul(columns)))
        .and_then(|bits| bits.checked_add(7))
        .map(|bits| bits / 8)
        .ok_or_else(|| FilterError::Unsupported("predictor row too large".into()))?;
    Ok(Some(bytes))
}

/// zlib (RFC 1950) at compression level 6.
pub fn encode_flate(data: &[u8]) -> Vec<u8> {
    shared::encode(shared::Filter::Flate, data, shared::EncodeParams::default()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_long_filter_chain_is_refused_before_decoding() {
        // `FF FF` run-length decodes to itself, so every stage of the chain repeats the work.
        let data = vec![0xFF; 4096];
        let chain = |n: usize| vec![(super::Filter::RunLength, super::Params::default()); n];
        let long = super::decode(&chain(1000), &data, 1 << 20);
        assert!(matches!(long, Err(super::FilterError::Unsupported(_))), "{long:?}");
        assert_eq!(super::decode(&chain(super::MAX_CHAIN), &data, 1 << 20).ok(), Some(data));
    }

    use super::*;

    #[test]
    fn names_and_abbreviations() {
        for (full, abbr, f) in [
            (&b"FlateDecode"[..], &b"Fl"[..], Filter::Flate),
            (b"LZWDecode", b"LZW", Filter::Lzw),
            (b"ASCIIHexDecode", b"AHx", Filter::AsciiHex),
            (b"ASCII85Decode", b"A85", Filter::Ascii85),
            (b"RunLengthDecode", b"RL", Filter::RunLength),
            (b"DCTDecode", b"DCT", Filter::Dct),
            (b"CCITTFaxDecode", b"CCF", Filter::CcittFax),
        ] {
            assert_eq!(Filter::from_name(full), f);
            assert_eq!(Filter::from_name(abbr), f);
            assert_eq!(f.name().as_bytes(), full);
        }
        assert_eq!(Filter::from_name(b"/JPXDecode"), Filter::Jpx);
        assert_eq!(Filter::from_name(b"JBIG2Decode"), Filter::Jbig2);
        assert_eq!(Filter::from_name(b"Crypt"), Filter::Crypt);
        assert_eq!(Filter::from_name(b"Foo"), Filter::Unknown("Foo".into()));
        assert_eq!(Filter::Unknown("Foo".into()).name(), "Foo");
        assert!(Filter::Dct.is_image_codec() && Filter::CcittFax.is_image_codec());
        assert!(!Filter::Flate.is_image_codec() && !Filter::Crypt.is_image_codec());
    }

    #[test]
    fn default_params() {
        let p = Params::default();
        assert_eq!((p.predictor, p.colors, p.bits_per_component, p.columns, p.early_change), (1, 1, 8, 1, 1));
    }

    #[test]
    fn unsupported_filters() {
        for f in [Filter::Dct, Filter::Jpx, Filter::Jbig2, Filter::CcittFax, Filter::Crypt, Filter::Unknown("X".into())] {
            let chain = [(Filter::AsciiHex, Params::default()), (f.clone(), Params::default())];
            assert!(matches!(decode(&chain, b"00>", 100), Err(FilterError::Unsupported(_))));
            assert!(matches!(decode_tolerant(&chain, b"00>", 100), Err(FilterError::Unsupported(_))));
            assert!(matches!(encode(&f, &Params::default(), b"x"), Err(FilterError::Unsupported(_))));
        }
    }

    #[test]
    fn empty_chain_is_identity() {
        assert_eq!(decode(&[], b"abc", 1).unwrap(), b"abc");
    }

    #[test]
    fn chain_ascii85_flate() {
        let data = b"Hello, chained filters! Hello, chained filters!".repeat(10);
        let z = encode_flate(&data);
        let a = encode(&Filter::Ascii85, &Params::default(), &z).unwrap();
        let chain = [(Filter::Ascii85, Params::default()), (Filter::Flate, Params::default())];
        assert_eq!(decode(&chain, &a, 1 << 20).unwrap(), data);
    }

    #[test]
    fn chain_three_filters_with_predictor() {
        let p = Params { predictor: 12, colors: 3, columns: 5, ..Params::default() };
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 % 251) as u8).collect();
        let lzw = encode(&Filter::Lzw, &p, &data).unwrap();
        let rl = encode(&Filter::RunLength, &Params::default(), &lzw).unwrap();
        let hex = encode(&Filter::AsciiHex, &Params::default(), &rl).unwrap();
        let chain = [(Filter::AsciiHex, Params::default()), (Filter::RunLength, Params::default()), (Filter::Lzw, p)];
        assert_eq!(decode(&chain, &hex, 1 << 20).unwrap(), data);
    }

    #[test]
    fn limit_applies_to_intermediate_buffers() {
        let hex = encode(&Filter::AsciiHex, &Params::default(), &[7u8; 100]).unwrap();
        let chain = [(Filter::AsciiHex, Params::default())];
        assert!(matches!(decode(&chain, &hex, 99), Err(FilterError::LimitExceeded(99))));
        assert_eq!(decode(&chain, &hex, 100).unwrap().len(), 100);
    }

    #[test]
    fn raw_flate_fallback_preserves_output_limits() {
        // RFC 1951 stored blocks: the first is nonfinal with LEN=257 and
        // NLEN=!257, followed by a final empty block. Its ignored alignment
        // bits make the first two bytes also look like a valid zlib header.
        let data = vec![b'A'; 257];
        let mut raw = vec![0x78, 0x01, 0x01, 0xfe, 0xfe];
        raw.extend_from_slice(&data);
        raw.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
        let chain = [(Filter::Flate, Params::default())];
        for max in [0, 1, 128, 256] {
            assert!(matches!(decode(&chain, &raw, max), Err(FilterError::LimitExceeded(m)) if m == max));
            assert!(matches!(decode_tolerant(&chain, &raw, max), Err(FilterError::LimitExceeded(m)) if m == max));
        }
        for max in [257, 258] {
            assert_eq!(decode(&chain, &raw, max).unwrap(), data);
            assert_eq!(decode_tolerant(&chain, &raw, max).unwrap(), (data.clone(), false));
        }
    }

    #[test]
    fn tolerant_flate_returns_partial() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 97) as u8 ^ (i / 300) as u8).collect();
        let mut z = encode_flate(&data);
        let cut = z.len() / 2;
        z.truncate(cut);
        let chain = [(Filter::Flate, Params::default())];
        assert!(matches!(decode(&chain, &z, 1 << 20), Err(FilterError::Corrupt { .. })));
        let (out, partial) = decode_tolerant(&chain, &z, 1 << 20).unwrap();
        assert!(partial);
        assert!(!out.is_empty() && out.len() < data.len());
        assert_eq!(&data[..out.len()], &out[..]);
        // Complete data is not partial.
        let (out, partial) = decode_tolerant(&chain, &encode_flate(&data), 1 << 20).unwrap();
        assert!(!partial);
        assert_eq!(out, data);
    }

    #[test]
    fn tolerant_partial_flows_through_predictor() {
        let p = Params { predictor: 12, columns: 4, ..Params::default() };
        let data: Vec<u8> = (0..4000u32).map(|i| (i * 31 % 256) as u8).collect();
        let mut z = encode(&Filter::Flate, &p, &data).unwrap();
        z.truncate(z.len() - 20);
        let (out, partial) = decode_tolerant(&[(Filter::Flate, p)], &z, 1 << 20).unwrap();
        assert!(partial);
        assert_eq!(&data[..out.len()], &out[..]);
    }

    /// A counted decode bounds what all stages decode together: Flate inflating 1 MiB of hex
    /// that ASCIIHex halves is 1.5 MiB of work, past a 1.25 MiB allowance. A plain decode keeps
    /// the limit per stage.
    #[test]
    fn a_counted_decode_bounds_all_stages_together() {
        let hex: Vec<u8> = std::iter::repeat_n(b"ab".as_slice(), 1 << 19).flatten().copied().collect();
        let data = encode_flate(&hex);
        let chain = [(Filter::Flate, Params::default()), (Filter::AsciiHex, Params::default())];
        let max = (1 << 20) + (1 << 18);
        assert_eq!(decode(&chain, &data, max).map(|d| d.len()).ok(), Some(1 << 19));
        assert!(matches!(decode_counted(&chain, &data, max), Err((FilterError::LimitExceeded(m), _)) if m == max));
        assert_eq!(decode_counted(&chain, &data, 2 << 20).map(|(_, spent)| spent).ok(), Some((1 << 20) + (1 << 19)));
        let tolerant = decode_tolerant_counted(&chain, &data, 2 << 20).map(|(out, partial, spent)| (out.len(), partial, spent));
        assert_eq!(tolerant.ok(), Some((1 << 19, false, (1 << 20) + (1 << 19))));
        assert!(matches!(decode_tolerant_counted(&chain, &data, max), Err((FilterError::LimitExceeded(m), _)) if m == max));
    }

    /// A PNG predictor drops a tag byte per row, but Flate produced it: the count includes it,
    /// also when the predictor fails on a bad tag.
    #[test]
    fn a_counted_decode_counts_output_before_the_predictor() {
        let params = Params { predictor: 12, ..Params::default() };
        let rows = vec![7u8; 1 << 16];
        let data = encode(&Filter::Flate, &params, &rows).unwrap();
        let chain = [(Filter::Flate, params.clone())];
        let (out, spent) = decode_counted(&chain, &data, usize::MAX).unwrap();
        assert!(out == rows);
        assert_eq!(spent, 2 << 16);
        let bad: Vec<u8> = std::iter::repeat_n([0u8, 7], 1 << 15).flatten().chain([9, 7]).collect();
        let (e, spent) = decode_counted(&chain, &encode_flate(&bad), usize::MAX).unwrap_err();
        assert!(matches!(e, FilterError::Corrupt { .. }), "{e}");
        assert_eq!(spent, bad.len());
    }

    fn refused(e: &FilterError) -> bool {
        matches!(e, FilterError::Spent)
    }

    #[test]
    fn bomb_output_boundaries_are_unchanged() {
        let size = 16 << 20;
        let compressed = encode_flate(&vec![0; size]);
        let chain = [(Filter::Flate, Params::default())];
        for limit in [0, 1 << 20, size - 1] {
            assert!(matches!(decode(&chain, &compressed, limit), Err(FilterError::LimitExceeded(_))));
            assert!(matches!(decode_tolerant(&chain, &compressed, limit), Err(FilterError::LimitExceeded(_))));
        }
        assert_eq!(decode(&chain, &compressed, size).unwrap().len(), size);
        assert_eq!(decode_tolerant(&chain, &compressed, size).unwrap().0.len(), size);

        let inner = encode_flate(&vec![0; 32 << 20]);
        let bomb = encode_flate(&inner);
        let chain = [(Filter::Flate, Params::default()), (Filter::Flate, Params::default())];
        assert!(matches!(decode(&chain, &bomb, 1 << 20), Err(FilterError::LimitExceeded(_))));
        assert!(matches!(decode_tolerant(&chain, &bomb, 1 << 20), Err(FilterError::LimitExceeded(_))));
    }

    /// A chain pays for what it reads and what each stage decodes, down to the byte: Flate inflates
    /// hex that ASCIIHex halves, so a stage counts when a later one shrinks it.
    #[test]
    fn a_chain_spends_down_to_the_byte() {
        let hex = b"ab".repeat(1 << 10);
        let data = encode_flate(&hex);
        let chain = [(Filter::Flate, Params::default()), (Filter::AsciiHex, Params::default())];
        let whole = decode(&chain, &data, 1 << 20).unwrap();
        let cost = (data.len() + hex.len() + whole.len()) as u64;
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), whole);
        assert_eq!(u64::MAX - work, cost);
        let mut work = cost;
        assert_eq!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap(), whole, "exactly enough");
        assert_eq!(work, 0);
        let mut work = cost + 7;
        decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap();
        assert_eq!(work, 7, "only what it took");
        let mut work = cost - 1;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap_err()), "a step short");
        assert_eq!(work, 0, "spends all that was left");
        // Each stage has `max_output` to itself, as in a plain decode: together they are past it.
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &data, hex.len(), &mut work).unwrap(), whole);
    }

    /// No stage runs past what is left: the first stage of this bomb stops there, so the second
    /// isn't given anything to read, and with too little to read it nothing runs at all.
    #[test]
    fn a_chain_stops_at_what_is_left() {
        let inner = encode_flate(&vec![0; 1 << 20]);
        let bomb = encode_flate(&inner);
        let chain = [(Filter::Flate, Params::default()), (Filter::Flate, Params::default())];
        let read = |work: u64| {
            let mut work = work;
            assert!(refused(&decode_tolerant_within(&chain, &bomb, usize::MAX, &mut work).unwrap_err()));
            assert_eq!(work, 0);
        };
        read((bomb.len() + inner.len()) as u64 + 10);
        read(bomb.len() as u64 + 10);
        read(bomb.len() as u64 - 1);
    }

    /// Past `max_output` the chain has failed, and hasn't run out of anything: it pays what it took
    /// to find out, and what is left stays. Whichever limit a stage stops at is the one reported.
    #[test]
    fn a_stage_past_its_limit_pays_for_finding_out() {
        let data = encode_flate(&vec![0; 1 << 16]);
        let chain = [(Filter::Flate, Params::default())];
        let mut work = u64::MAX;
        assert!(matches!(decode_tolerant_within(&chain, &data, 1000, &mut work), Err(FilterError::LimitExceeded(1000))));
        assert_eq!(u64::MAX - work, data.len() as u64 + 1000);
        // The later stage of two, which pays for the first as well.
        let bomb = encode_flate(&data);
        let two = [(Filter::Flate, Params::default()), (Filter::Flate, Params::default())];
        let mut work = u64::MAX;
        assert!(matches!(decode_tolerant_within(&two, &bomb, data.len(), &mut work), Err(FilterError::LimitExceeded(_))));
        assert_eq!(u64::MAX - work, (bomb.len() + 2 * data.len()) as u64);
        // Both limits at once: a run that can't go on is what a caller needs to hear.
        let mut work = data.len() as u64 + 1000;
        assert!(refused(&decode_tolerant_within(&chain, &data, 1000, &mut work).unwrap_err()));
        assert_eq!(work, 0);
        let mut work = data.len() as u64 + 1001;
        assert!(matches!(decode_tolerant_within(&chain, &data, 1000, &mut work), Err(FilterError::LimitExceeded(1000))));
        assert_eq!(work, 1, "the image's limit, with a step to spare");
    }

    /// What a damaged stream decoded is paid for, however the decode ends.
    #[test]
    fn a_chain_that_fails_has_paid_for_what_it_did() {
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 97) as u8 ^ (i / 300) as u8).collect();
        let mut cut = encode_flate(&data);
        cut.truncate(cut.len() / 2);
        let chain = [(Filter::Flate, Params::default())];
        let (partial, _) = decode_tolerant(&chain, &cut, 1 << 20).unwrap();
        assert!(!partial.is_empty());
        let mut work = u64::MAX;
        assert_eq!(decode_tolerant_within(&chain, &cut, 1 << 20, &mut work).unwrap(), partial);
        assert_eq!(u64::MAX - work, (cut.len() + partial.len()) as u64);
        // Read strictly it is an error, for the same work.
        let mut work = u64::MAX;
        assert!(matches!(decode_within(&chain, &cut, 1 << 20, &mut work), Err(FilterError::Corrupt { .. })));
        assert_eq!(u64::MAX - work, (cut.len() + partial.len()) as u64);
        let whole = encode_flate(&data);
        let mut work = u64::MAX;
        assert_eq!(decode_within(&chain, &whole, 1 << 20, &mut work).unwrap(), data);
        assert_eq!(u64::MAX - work, (whole.len() + data.len()) as u64);
        // A predictor that comes to nothing, after all of this was inflated for it.
        let p = Params { predictor: 99, ..Params::default() };
        let data = encode_flate(&vec![0; 1 << 16]);
        let mut work = u64::MAX;
        assert!(decode_tolerant_within(&[(Filter::Flate, p)], &data, 1 << 20, &mut work).unwrap().is_empty());
        assert_eq!(u64::MAX - work, data.len() as u64 + (1 << 16));
    }

    /// A stream that reads as zlib only until it fails, and as raw deflate until that fails too,
    /// gives back no data, and has still decoded everything the retry did.
    #[test]
    fn a_retry_that_fails_pays_for_what_it_decoded() {
        // Stored as raw deflate: this many bytes, then a block of a reserved type. As zlib: the
        // same reserved type at once, so the first reading fails with nothing.
        let mut data = vec![0x78, 0x01, 0x06, 0xFE, 0xF9];
        data.extend(vec![0u8; 0x0601]);
        data.push(0x07);
        let chain = [(Filter::Flate, Params::default())];
        let (out, partial) = decode_tolerant(&chain, &data, 1 << 20).unwrap();
        assert!(out.is_empty() && partial, "what is read stays as it was");
        let (_, _, spent) = decode_tolerant_counted(&chain, &data, 1 << 20).unwrap();
        assert_eq!(spent, 0x0601);
        let (e, spent) = decode_counted(&chain, &data, 1 << 20).unwrap_err();
        assert!(matches!(e, FilterError::Corrupt { .. }), "{e}");
        assert_eq!(spent, 0x0601);
        let mut work = u64::MAX;
        assert!(decode_tolerant_within(&chain, &data, 1 << 20, &mut work).unwrap().is_empty());
        assert_eq!(u64::MAX - work, (data.len() + 0x0601) as u64);
        // Through a predictor, which would otherwise lose the count.
        let p = Params { predictor: 12, columns: 4, ..Params::default() };
        let (_, _, spent) = decode_tolerant_counted(&[(Filter::Flate, p)], &data, 1 << 20).unwrap();
        assert_eq!(spent, 0x0601);
    }

    /// With nothing left nothing is read. What can't be decoded at all never costs anything, and
    /// says so whatever is left. Without filters, what is read is the data.
    #[test]
    fn what_a_chain_is_shows_whatever_is_left() {
        let flate = [(Filter::Flate, Params::default())];
        let mut work = 0;
        assert!(refused(&decode_tolerant_within(&flate, b"garbage", 1 << 20, &mut work).unwrap_err()));
        let long = vec![(Filter::Flate, Params::default()); MAX_CHAIN + 1];
        for chain in [vec![(Filter::Dct, Params::default())], vec![(Filter::Jbig2, Params::default())], long] {
            for left in [0, 100] {
                let mut work = left;
                let e = decode_tolerant_within(&chain, b"data", 1 << 20, &mut work).unwrap_err();
                assert!(matches!(e, FilterError::Unsupported(_)) && !refused(&e), "{e}");
                assert_eq!(work, left);
            }
        }
        let mut work = 100;
        assert_eq!(decode_tolerant_within(&[], b"data", 4, &mut work).unwrap(), b"data");
        assert_eq!(work, 96);
        assert!(matches!(decode_tolerant_within(&[], b"data", 3, &mut work), Err(FilterError::LimitExceeded(3))));
        assert_eq!(work, 92);
        let mut work = 3;
        assert!(refused(&decode_tolerant_within(&[], b"data", 4, &mut work).unwrap_err()));
        assert_eq!(work, 0);
    }
}
