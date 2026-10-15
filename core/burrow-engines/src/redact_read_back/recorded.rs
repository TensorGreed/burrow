//! The second reading, recorded where it is read and replayed where it is judged (#206's web half).
//!
//! # Why a recording
//!
//! On the web the two halves of the second reading live in different workers: the redaction
//! module has the output and burrow's walk of it, and no PDFium; the render module has PDFium.
//! Neither can call the other and wait. So the render module **reads** — [`record`] over its
//! [`SecondReading`] — and hands back a [`Recorded`] as bytes ([`encode`]); the redaction module
//! decodes it ([`decode`]) and runs the same rules native runs ([`super::agrees`]) over a
//! replay of it. The rules run in one place, in Rust, on both platforms.
//!
//! # Bound to the exact bytes, by digest
//!
//! A reading is evidence about one document, and a reading of another one is no evidence at all:
//! a stale preview, the input in place of the output, or the previous run's output would each
//! agree with burrow about a page nobody is about to download. So [`record`] takes the SHA-256 of
//! the bytes **it** read, at the point its engine reads them, and [`agrees_recorded`] refuses as
//! `[second-reading-other-document]` unless both digests are of the bytes **the redaction module**
//! holds. Recomputed on each side from its own bytes; neither side trusts a digest it did not take.
//!
//! What this does not cover, stated: the digest proves the reading is *of* these bytes, not that
//! the render worker read them honestly. Both workers are this build's own code, and the render
//! module's reading is the same algorithm as native's (`web::WebPdfiumReading`).
//!
//! # Where `sha2` ships
//!
//! The digest-taking half -- [`digest`], [`record`] and [`agrees_recorded`] -- is
//! behind the `redaction-digest` feature, which only the redaction wasm module enables. The render
//! module takes its digests with `crypto.subtle.digest` and calls [`record_digested`]; the codec
//! needs no hash. So `sha2` is linked into the redaction module and into nothing else that ships.
//!
//! # The encoding
//!
//! Version 1, little-endian, fixed layout and exact length -- nothing optional, nothing skipped:
//!
//! ```text
//! u8 version (1) | [u8; 32] input digest | [u8; 32] output digest | u64 input pages
//! | u64 pages | u64 page | f64 width | f64 height | u32 count
//! | count × (f64 x, f64 y, u8 generated: 0 or 1)
//! ```
//!
//! [`decode`] refuses a wrong version, a length that is not exactly the one the count implies, a
//! count past [`super::MAX_CHARS`], and a `generated` byte other than 0 or 1. Floats are carried
//! as bits; whether they are finite is the rules' question ([`super::agrees`] refuses a NaN origin
//! as no answer), not the codec's.

use burrow_types::{Error, Result};
#[cfg(any(test, feature = "redaction-digest"))]
use sha2::{Digest as _, Sha256};

#[cfg(any(test, feature = "redaction-digest"))]
use super::{Agreement, agrees, rejected};
use super::{MAX_CHARS, PageReading, PlacedChar, SecondReading};

/// A SHA-256 digest.
pub type Digest = [u8; 32];

#[cfg(any(test, feature = "redaction-digest"))]
/// The SHA-256 of `bytes`.
#[must_use]
pub fn digest(bytes: &[u8]) -> Digest {
    Sha256::digest(bytes).into()
}

/// What the render module read, and of which bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    /// The SHA-256 of the input the page count was read from.
    pub input_digest: Digest,
    /// The SHA-256 of the output the page was read from.
    pub output_digest: Digest,
    /// The input's page count, as the reading saw it (R5).
    pub input_pages: u64,
    /// The output page, as the reading saw it.
    pub page: PageReading,
}

#[cfg(any(test, feature = "redaction-digest"))]
/// Read `input`'s page count and `output`'s page `page` through `reading`, and record both with
/// the digests of the bytes that were read.
///
/// # Errors
///
/// Whatever `reading` refuses; a reading with no answer is [`Error::Malformed`], so the replay
/// can only ever be given an answer or nothing.
pub fn record<R: SecondReading>(
    reading: &R,
    input: &[u8],
    output: &[u8],
    page: usize,
) -> Result<Recorded> {
    // THE DIGESTS FIRST, of the very slices the engine is about to read.
    record_digested(reading, input, output, page, digest(input), digest(output))
}

/// [`record`], with the digests taken by the caller -- the render worker, which takes them with
/// the platform's own SHA-256 (`crypto.subtle.digest`) so that `sha2` is not linked into the
/// module every tool page loads (owner, 2026-10-15).
///
/// The caller's digests must be of exactly `input` and `output`. Nothing here can check that,
/// and nothing needs to for the verdict to be sound: [`agrees_recorded`] recomputes both digests
/// from the bytes the redaction module holds, so a digest of anything else refuses as
/// `[second-reading-other-document]`. What the caller's discipline adds is that a reading which
/// is refused was refused for the right reason.
///
/// # Errors
///
/// As [`record`].
pub fn record_digested<R: SecondReading>(
    reading: &R,
    input: &[u8],
    output: &[u8],
    page: usize,
    input_digest: Digest,
    output_digest: Digest,
) -> Result<Recorded> {
    let none = || Error::Malformed("the renderer gave no answer".to_owned());
    let input_pages = reading.page_count(input)?.ok_or_else(none)?;
    let page = reading.read_page(output, page)?.ok_or_else(none)?;
    Ok(Recorded {
        input_digest,
        output_digest,
        input_pages,
        page,
    })
}

/// The encoding's version byte.
const VERSION: u8 = 1;
/// Bytes before the characters: version, two digests, five 8-byte fields, the count.
const HEADER: usize = 1 + 32 + 32 + 8 * 5 + 4;
/// Bytes per character: two `f64` and one flag.
const PER_CHAR: usize = 8 + 8 + 1;

/// `recorded` as bytes, for the trip across the worker boundary.
///
/// # Errors
///
/// [`Error::Internal`] if the page has more characters than [`MAX_CHARS`], which the reading has
/// already refused, or than fits the count field.
pub fn encode(recorded: &Recorded) -> Result<Vec<u8>> {
    let chars = &recorded.page.chars;
    if chars.len() > MAX_CHARS {
        return Err(Error::Internal(
            "a recording with more characters than a reading may hold".to_owned(),
        ));
    }
    let count = u32::try_from(chars.len())
        .map_err(|_| Error::Internal("a character count past u32".to_owned()))?;
    let page = u64::try_from(recorded.page.page)
        .map_err(|_| Error::Internal("a page index past u64".to_owned()))?;
    let mut out = Vec::with_capacity(HEADER + chars.len() * PER_CHAR);
    out.push(VERSION);
    out.extend_from_slice(&recorded.input_digest);
    out.extend_from_slice(&recorded.output_digest);
    out.extend_from_slice(&recorded.input_pages.to_le_bytes());
    out.extend_from_slice(&recorded.page.pages.to_le_bytes());
    out.extend_from_slice(&page.to_le_bytes());
    out.extend_from_slice(&recorded.page.size.0.to_bits().to_le_bytes());
    out.extend_from_slice(&recorded.page.size.1.to_bits().to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for c in chars {
        out.extend_from_slice(&c.origin.0.to_bits().to_le_bytes());
        out.extend_from_slice(&c.origin.1.to_bits().to_le_bytes());
        out.push(u8::from(c.generated));
    }
    Ok(out)
}

/// A cursor over the encoding that refuses to read past it.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .at
            .checked_add(N)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(malformed)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(self.bytes.get(self.at..end).ok_or_else(malformed)?);
        self.at = end;
        Ok(out)
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }
}

fn malformed() -> Error {
    Error::Malformed("a second-reading recording that does not decode".to_owned())
}

/// The [`Recorded`] that `bytes` encode.
///
/// # Errors
///
/// [`Error::Malformed`] for a wrong version, a count past [`MAX_CHARS`], a length other than the
/// one the count implies, or a `generated` flag other than 0 or 1.
pub fn decode(bytes: &[u8]) -> Result<Recorded> {
    let mut r = Reader { bytes, at: 0 };
    if r.take::<1>()? != [VERSION] {
        return Err(malformed());
    }
    let input_digest = r.take::<32>()?;
    let output_digest = r.take::<32>()?;
    let input_pages = r.u64()?;
    let pages = r.u64()?;
    let page = usize::try_from(r.u64()?).map_err(|_| malformed())?;
    let size = (r.f64()?, r.f64()?);
    let count = usize::try_from(u32::from_le_bytes(r.take()?)).map_err(|_| malformed())?;
    // THE COUNT AGAINST ITS CEILING AND THE LENGTH, BEFORE ANYTHING IS ALLOCATED for it.
    let expected = count
        .checked_mul(PER_CHAR)
        .and_then(|chars| chars.checked_add(HEADER));
    if count > MAX_CHARS || expected != Some(bytes.len()) {
        return Err(malformed());
    }
    let mut chars = Vec::with_capacity(count);
    for _ in 0..count {
        let origin = (r.f64()?, r.f64()?);
        let generated = match r.take::<1>()? {
            [0] => false,
            [1] => true,
            _ => return Err(malformed()),
        };
        chars.push(PlacedChar { origin, generated });
    }
    Ok(Recorded {
        input_digest,
        output_digest,
        input_pages,
        page: PageReading {
            pages,
            page,
            size,
            chars,
        },
    })
}

/// The longest reply the render worker can send: a recording of a page at [`MAX_CHARS`].
///
/// The redaction worker checks a reply's length against this **before** copying it into the wasm
/// heap, so a reply that cannot be a recording is never allocated for (design review L3).
pub const MAX_ANSWER_LEN: usize = HEADER + MAX_CHARS * PER_CHAR;

/// The tag of a failure answer. A recording's first byte is its version, [`VERSION`].
const FAILED: u8 = 0x80;
/// A failure answer's length: the tag, the kind, the limit and stage indexes, two `u64`.
const FAILED_LEN: usize = 1 + 1 + 1 + 1 + 8 + 8;

/// The ceilings a failure can name, by index. Fixed, so an index means one name on both sides.
const LIMITS: [&str; 5] = [
    "max_input_bytes",
    "max_memory_bytes",
    "max_duration_ms",
    "max_pages",
    "max_pixels",
];

/// The stages a failure can name, by index.
const STAGES: [burrow_types::Stage; 7] = [
    burrow_types::Stage::InputSize,
    burrow_types::Stage::SizeEstimate,
    burrow_types::Stage::Prescan,
    burrow_types::Stage::PageCount,
    burrow_types::Stage::Pixels,
    burrow_types::Stage::Measured,
    burrow_types::Stage::Deadline,
];

/// Why the render worker has no recording, as a type and never as text (design review L1).
///
/// Rebuilt into an [`Error`] the way [`super::agrees`]'s `wrapped` treats a reading's errors: a
/// ceiling passes through as itself, an internal fault as an internal fault, and everything else
/// is no answer, which is a refusal. **No error text crosses**: the render worker's message is
/// about the person's document, and it is dropped where it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// A ceiling, named by the render worker, with its numbers.
    Limit {
        /// Which ceiling.
        limit: &'static str,
        /// Which check.
        stage: burrow_types::Stage,
        /// What the reading asked for.
        requested: u64,
        /// What the ceiling permitted.
        allowed: u64,
    },
    /// The render worker's own code failed.
    Internal,
    /// Anything else: the engine could not read the documents, or gave no answer.
    NoAnswer,
}

impl Failure {
    /// What the render worker's `error` becomes on the way back.
    #[must_use]
    pub fn of(error: &Error) -> Self {
        match error {
            Error::LimitExceeded {
                limit,
                stage,
                requested,
                allowed,
            } if LIMITS.contains(limit) && STAGES.contains(stage) => Self::Limit {
                limit,
                stage: *stage,
                requested: *requested,
                allowed: *allowed,
            },
            Error::Internal(_) => Self::Internal,
            _ => Self::NoAnswer,
        }
    }

    /// The failure as the redaction worker reports it: as `wrapped` would.
    #[must_use]
    pub fn into_error(self) -> Error {
        match self {
            Self::Limit {
                limit,
                stage,
                requested,
                allowed,
            } => Error::LimitExceeded {
                limit,
                stage,
                requested,
                allowed,
            },
            Self::Internal => Error::Internal(
                "redact: the renderer's second reading failed in its own code".to_owned(),
            ),
            Self::NoAnswer => unavailable(),
        }
    }
}

/// `[second-reading-unavailable]`: the renderer gave no reading, which is a refusal.
fn unavailable() -> Error {
    Error::OutputRejected(
        "redact: the second reading refuses the output -- [second-reading-unavailable] the \
         renderer gave no reading of the documents"
            .to_owned(),
    )
}

/// What the render worker sends back: a recording, or why there is none.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The reading.
    Read(Recorded),
    /// No reading, and why, as a type.
    Failed(Failure),
}

/// `answer` as bytes: a recording ([`encode`]), or a fixed-length failure.
///
/// # Errors
///
/// As [`encode`].
pub fn encode_answer(answer: &Answer) -> Result<Vec<u8>> {
    match answer {
        Answer::Read(recorded) => encode(recorded),
        Answer::Failed(failure) => {
            let (kind, limit, stage, requested, allowed) = match *failure {
                Failure::Limit {
                    limit,
                    stage,
                    requested,
                    allowed,
                } => {
                    let at =
                        |what: &str| Error::Internal(format!("a failure naming no indexed {what}"));
                    let l = LIMITS
                        .iter()
                        .position(|n| *n == limit)
                        .ok_or_else(|| at("ceiling"))?;
                    let s = STAGES
                        .iter()
                        .position(|n| *n == stage)
                        .ok_or_else(|| at("stage"))?;
                    (0_u8, l, s, requested, allowed)
                }
                Failure::Internal => (1, 0, 0, 0, 0),
                Failure::NoAnswer => (2, 0, 0, 0, 0),
            };
            let index = |n: usize| {
                u8::try_from(n).map_err(|_| Error::Internal("an index past u8".to_owned()))
            };
            let mut out = Vec::with_capacity(FAILED_LEN);
            out.push(FAILED);
            out.push(kind);
            out.push(index(limit)?);
            out.push(index(stage)?);
            out.extend_from_slice(&requested.to_le_bytes());
            out.extend_from_slice(&allowed.to_le_bytes());
            Ok(out)
        }
    }
}

/// The [`Answer`] that `bytes` encode.
///
/// # Errors
///
/// [`Error::Malformed`] for anything [`decode`] refuses, a length past [`MAX_ANSWER_LEN`], or a
/// failure that is not exactly [`FAILED_LEN`] bytes naming a known kind, ceiling and stage.
pub fn decode_answer(bytes: &[u8]) -> Result<Answer> {
    if bytes.len() > MAX_ANSWER_LEN {
        return Err(malformed());
    }
    if bytes.first() != Some(&FAILED) {
        return decode(bytes).map(Answer::Read);
    }
    let mut r = Reader { bytes, at: 0 };
    let [_, kind, limit, stage] = r.take::<4>()?;
    let requested = r.u64()?;
    let allowed = r.u64()?;
    if bytes.len() != FAILED_LEN {
        return Err(malformed());
    }
    let failure = match kind {
        0 => Failure::Limit {
            limit: LIMITS
                .get(usize::from(limit))
                .copied()
                .ok_or_else(malformed)?,
            stage: *STAGES.get(usize::from(stage)).ok_or_else(malformed)?,
            requested,
            allowed,
        },
        1 => Failure::Internal,
        2 => Failure::NoAnswer,
        _ => return Err(malformed()),
    };
    Ok(Answer::Failed(failure))
}

/// A reading that replays a recording whose digests were checked once, by [`agrees_recorded`],
/// against exactly the bytes [`super::agrees`] is then handed (design review L2: hash once).
#[cfg(any(test, feature = "redaction-digest"))]
struct Replay {
    recorded: Recorded,
}

#[cfg(any(test, feature = "redaction-digest"))]
impl SecondReading for Replay {
    fn page_count(&self, _bytes: &[u8]) -> Result<Option<u64>> {
        Ok(Some(self.recorded.input_pages))
    }

    fn read_page(&self, _bytes: &[u8], _page: usize) -> Result<Option<PageReading>> {
        // THE PAGE IS NOT MATCHED HERE: the recording says which page it read, and `agrees`
        // refuses a reading of another page as `[second-reading-wrong-page]`.
        Ok(Some(self.recorded.page.clone()))
    }
}

/// [`super::agrees`], over a recording: the digests first, taken once, then the rules.
///
/// # Errors
///
/// `[second-reading-other-document]` unless the recording is of exactly `input` and `output`;
/// then whatever [`super::agrees`] refuses.
#[cfg(any(test, feature = "redaction-digest"))]
pub fn agrees_recorded(
    recorded: Recorded,
    input: &[u8],
    output: &[u8],
    expected: &Agreement,
) -> Result<()> {
    // ONE DIGEST OF EACH, here, of the very slices `agrees` reads below.
    if digest(input) != recorded.input_digest || digest(output) != recorded.output_digest {
        return Err(rejected(format!(
            "[second-reading-other-document] the renderer's reading of page {} is not a reading of \
             this document",
            expected.page
        )));
    }
    agrees(&Replay { recorded }, input, output, expected)
}
