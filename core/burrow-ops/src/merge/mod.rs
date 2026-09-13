//! Combine several PDFs into one, in the order given.
//!
//! The first operation in this crate, and the first anywhere in burrow that produces
//! bytes. [ADR 0017](../../../docs/adr/0017-merge-engine-and-failure-semantics.md) settles
//! the three questions a function signature could not: which engine writes, what happens
//! when one input fails, and whether `Limits` are per input or aggregate.

use std::sync::Arc;

use burrow_engines::{OpenOptions, PageAssembler};
use burrow_types::{Deadline, Error, Limits, Password, Result, Stage};

/// One document to merge, with the password that opens it if it has one.
///
/// A struct rather than a bare `Box<[u8]>` because a password belongs to *its* input: five
/// documents can have five different passwords, and a single password parameter would have
/// silently meant "they all share one".
#[non_exhaustive]
pub struct Input<'a> {
    /// The document's bytes.
    ///
    /// Owned, because the engine does not copy what it is given: qpdf reads the buffer for
    /// as long as it holds the document (`QPDF.hh:88-90`).
    pub bytes: Box<[u8]>,
    /// The password, if this document is encrypted.
    ///
    /// [`Password`] rather than `String`: the type exists because a lossy conversion
    /// changes the password, and it wipes itself on drop.
    pub password: Option<&'a Password>,
}

impl<'a> Input<'a> {
    /// An input with no password.
    #[must_use]
    pub fn new(bytes: Box<[u8]>) -> Self {
        Self {
            bytes,
            password: None,
        }
    }

    /// An input with a password.
    #[must_use]
    pub fn with_password(bytes: Box<[u8]>, password: &'a Password) -> Self {
        Self {
            bytes,
            password: Some(password),
        }
    }
}

// A hand-written `Debug`, so a document's bytes cannot reach a log through a derive.
impl core::fmt::Debug for Input<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Input")
            .field("bytes", &self.bytes.len())
            .field("has_password", &self.password.is_some())
            .finish()
    }
}

/// Merge `inputs` into one document, preserving their order.
///
/// # Limits, and which of them are per input
///
/// Both, and the difference matters — per input alone is a hole a caller cannot see. A
/// hundred inputs each a byte under `max_input_bytes` is a hundred times the ceiling they
/// set. See [ADR 0017](../../../docs/adr/0017-merge-engine-and-failure-semantics.md) §3.
///
/// | limit | per input | across the operation |
/// |---|---|---|
/// | `max_input_bytes` | yes, at [`Stage::InputSize`] | yes, against the sum |
/// | `max_pages` | yes | yes, against the output total |
/// | `max_duration_ms` | — | the whole operation, checkpointed between inputs |
/// | `max_memory_bytes` | detected, not bounded | detected, not bounded |
/// | `max_pixels` | not applicable: merge decodes no raster | — |
///
/// **`max_memory_bytes` bounds nothing here, as everywhere else.** The structural pre-scan
/// reads declarations, and the measured check fires after the allocation. An overrun is
/// *detected*, not prevented — [ADR 0007](../../../docs/adr/0007-limit-enforcement-per-platform.md)'s
/// 2026-09-12 amendment, and this operation does not change it.
///
/// **`max_duration_ms` is cooperative**, checked between inputs and around engine calls. A
/// single engine call that runs forever is not interrupted by it; on the web the worker
/// watchdog is what covers that ([ADR 0015](../../../docs/adr/0015-web-worker-lifecycle.md) §2).
///
/// # What survives
///
/// The output inherits the **first** input's catalog: its outline, `/AcroForm` and
/// attachments are the ones that carry over. Later inputs contribute their pages. Merging
/// outlines from several documents is a feature with no obviously right answer and is not
/// attempted here; ADR 0017 says so rather than leaving it to be discovered.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] — no inputs. Merging nothing has no sensible output, and
///   returning an empty PDF would be a guess about what the caller meant.
/// - [`Error::InputFailed`] — an input could not be read or appended, naming **which**.
///   Any one failing fails the whole operation and no output is produced: a merged
///   document that silently omits an input looks complete, which is data loss wearing the
///   costume of success (ADR 0017 §2).
/// - [`Error::LimitExceeded`] — a ceiling in `limits`, per the table above.
/// - [`Error::Io`] — the assembled document could not be serialised.
/// - [`Error::Internal`] — a length or count that does not fit its type. A bug here, not
///   in the file.
///
/// Note that a failure from a *later* stage still names the input it came from: an input
/// the engine can open is not necessarily one it can append, which ADR 0017 measured.
///
/// # Panics
///
/// Never. Every fallible step returns a typed error.
pub fn merge<E: PageAssembler>(
    engine: &E,
    inputs: Vec<Input<'_>>,
    options: &OpenOptions<'_>,
) -> Result<Vec<u8>> {
    let limits = options.limits;

    if inputs.is_empty() {
        return Err(Error::InvalidArgument(
            "merge needs at least one input".into(),
        ));
    }

    // THE AGGREGATE SIZE CHECK COMES FIRST, before a single document is opened.
    //
    // Every input is also checked individually by the engine, and that is not enough on its
    // own: the per-input ceiling says nothing about a hundred of them. Doing it here, up
    // front, means a caller who hands us 4 GB in small pieces is refused before any of it
    // reaches a C++ parser -- which is the ordering non-negotiable #3 asks for, "checked
    // exactly, before anything is allocated".
    let mut total_bytes: u64 = 0;
    for input in &inputs {
        let len = u64::try_from(input.bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".into()))?;
        total_bytes = total_bytes
            .checked_add(len)
            .ok_or_else(|| Error::Internal("total input length does not fit in u64".into()))?;
    }
    Limits::check(
        Stage::InputSize,
        "max_input_bytes",
        total_bytes,
        limits.max_input_bytes,
    )?;

    // One deadline for the whole operation, started before any engine work. The engine
    // starts its own per document -- it has to, since it is also used on its own -- but
    // that one cannot see the aggregate, and a merge of twenty files must not get twenty
    // full budgets.
    let clock = Arc::clone(&options.clock);
    let deadline = Deadline::start(clock.as_ref(), &limits);
    deadline.checkpoint(clock.as_ref())?;

    let mut inputs = inputs.into_iter().enumerate();

    // The first input becomes the destination; the output inherits its catalog.
    let (first_index, first) = inputs
        .next()
        .ok_or_else(|| Error::Internal("inputs vanished between checks".into()))?;
    let mut assembly = engine
        .begin(first.bytes, &options_for(options, first.password))
        .map_err(|source| at(first_index, source))?;

    for (index, input) in inputs {
        // The deadline is checked BETWEEN inputs. That is the only granularity available:
        // the work inside `append` is one engine call per page and no engine here offers a
        // timeout, a cancellation or an abort hook (ADR 0007). Saying so is better than
        // implying a limit that is checked more often than it is.
        //
        // NOT wrapped in `at(index, ..)`, deliberately. Running out of time is the
        // operation's outcome, not this input's -- blaming the file the clock happened to
        // stop on would tell someone to remove a document that is perfectly fine. That is
        // the same inversion PR 2 fixed natively, where a caller delayed behind someone
        // else's work was told its own deadline had expired, and that ADR 0015 §2 fixed
        // again on the web by starting the watchdog at the worker's ack.
        deadline.checkpoint(clock.as_ref())?;

        engine
            .append(
                &mut assembly,
                input.bytes,
                &options_for(options, input.password),
            )
            .map_err(|source| at(index, source))?;
    }

    deadline.checkpoint(clock.as_ref())?;

    engine.finish(assembly)
}

/// Wrap an input's failure with the position it came from.
///
/// Never re-wraps: a nested `InputFailed` would read as "input 2 of input 0", and the
/// inner index is the one that means anything to a caller.
fn at(index: usize, source: Error) -> Error {
    match source {
        already @ Error::InputFailed { .. } => already,
        source => Error::InputFailed {
            index,
            source: Box::new(source),
        },
    }
}

/// `options` with this input's password substituted.
fn options_for<'a>(options: &OpenOptions<'a>, password: Option<&'a Password>) -> OpenOptions<'a> {
    let mut per_input = OpenOptions::new(options.limits, Arc::clone(&options.clock));
    per_input.password = password;
    per_input
}

#[cfg(test)]
mod tests;
