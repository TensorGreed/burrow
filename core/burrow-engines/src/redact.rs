// AWAITING #134, and said so rather than silenced. Nothing calls this: ADR 0022 forbids a
// public entry point that emits an unverified redaction, and #134 is the verification. The
// assembly and its order are complete and tested; the caller is what is missing.
//
// `expect` rather than `allow` because it becomes an error the moment the operation is wired
// in, so this note cannot rot into a blanket exemption. Conditional on `not(test)` because the
// tests DO use it, and an unconditional expectation is unfulfilled under `--all-targets`.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the assembly is complete and tested; #134's verified path is its first caller"
    )
)]

//! The order the steps of a redaction run in, and what happens when one fails.
//!
//! # Not a public entry point, and deliberately so
//!
//! ADR 0022: an operation verifies its own output before returning it, and redaction's
//! verification is [#134](https://github.com/TensorGreed/burrow/issues/134). Until that exists
//! this module assembles the steps and hands back bytes **to the crate**, with no exported
//! function that emits a redacted document. Shipping an unverified redaction "temporarily" is
//! the shortcut ADR 0029 will not take: an operation that removes a secret and cannot say
//! whether it did is, from outside, indistinguishable from one that did not.
//!
//! # The order is load-bearing, not incidental
//!
//! 1. **Every content edit, across every affected stream.** The page's own content, each Form
//!    XObject the region reaches that is not shared, each pattern.
//! 2. **Then font surgery**, computing "no longer drawn" from the **complete** result of step 1.
//! 3. **Then the page strip** — the keys outside ADR 0029 §2's allowlist.
//!
//! **Step 2 cannot run early, and the failure is silent.** Font surgery removes the `/Widths`,
//! `/ToUnicode` and `/Differences` entries for codes the document no longer draws. "No longer
//! drawn" is a fact about the *finished* content, and a form edited in step 1 after the fonts
//! were already cut would leave entries for codes nothing draws any more — which is spike
//! 0006's channel 4 and 23 residue put back by hand. The `/ToUnicode` entry for a removed
//! glyph is the removed character, in plain text, in the font.
//!
//! Running it early does not fail loudly: the output is a valid PDF, the page renders
//! correctly, and the secret is legible to anything that reads the font rather than the page.
//! So the order is expressed as a state machine a caller cannot drive out of sequence, rather
//! than as three calls a reader is trusted to make in the right order.
//!
//! **Step 3 is last** because the page-key allowlist is decided against the page as it will
//! ship. A key stripped before an edit that would have removed its last reference is a key
//! whose removal nothing observed.
//!
//! # Any failure discards the whole document
//!
//! ADR 0029's #130 amendment: a failed write poisons the document. There is no partial
//! emission, no retry, and no fallback to a partly-edited state — the in-memory document is
//! dropped and the operation refuses. A half-redacted page is the worst possible output,
//! because it looks like a redaction.
//!
//! That is enforced by shape rather than by discipline: [`Redaction::emit`] consumes `self`,
//! and every fallible step consumes it too, returning it only on success. A caller holding an
//! error has nothing left to emit from.

use burrow_types::{Error, Result};

/// Which stream an edit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StreamId {
    /// The page's own content.
    Page,
    /// A Form XObject or pattern, by object identity.
    Object(u64),
}

/// The document operations a redaction needs, so the order can be tested without an engine.
///
/// Every method takes `&mut self` because each mutates the in-memory document. None of them
/// emits: emission is [`Redaction::emit`]'s alone, and it runs only after every step.
pub(crate) trait Steps {
    /// Every stream the region reaches, in a stable order.
    ///
    /// # Errors
    /// Whatever resolving the page's resources failed with.
    fn affected_streams(&mut self) -> Result<Vec<StreamId>>;

    /// Rewrite one stream, removing the glyphs inside the region.
    ///
    /// # Errors
    /// Whatever the edit failed with. **This poisons the document**; see the module header.
    fn rewrite(&mut self, stream: StreamId) -> Result<()>;

    /// Which character codes each font still draws, **after** every content edit.
    ///
    /// # Errors
    /// Whatever walking the finished content failed with.
    fn codes_still_drawn(&mut self) -> Result<Vec<(u64, Vec<u32>)>>;

    /// Remove font entries for codes nothing draws any more.
    ///
    /// # Errors
    /// Whatever the edit failed with.
    fn cut_fonts(&mut self, still_drawn: &[(u64, Vec<u32>)]) -> Result<()>;

    /// Strip the page keys outside ADR 0029 §2's allowlist.
    ///
    /// # Errors
    /// Whatever the edit failed with.
    fn strip_page_keys(&mut self) -> Result<()>;

    /// Write the document out. Called at most once, and only after every step succeeded.
    ///
    /// # Errors
    /// Whatever writing failed with.
    fn write(&mut self) -> Result<Vec<u8>>;
}

/// A redaction in progress, which can only be driven in one order.
///
/// Each method consumes `self` and returns the next state, so a caller cannot run font surgery
/// before the content edits, cannot run a step twice, and cannot hold on to a document whose
/// edit failed. The compiler enforces the order that a comment would only describe.
pub(crate) struct Redaction<S: Steps> {
    steps: S,
}

/// Content edits are done; font surgery is next.
pub(crate) struct ContentEdited<S: Steps> {
    steps: S,
    /// How many streams were rewritten, so a caller can assert the work happened.
    rewritten: usize,
}

/// Fonts are cut; the page strip is next.
pub(crate) struct FontsCut<S: Steps> {
    steps: S,
}

/// Everything is done; the only thing left is to emit.
pub(crate) struct Finished<S: Steps> {
    steps: S,
}

impl<S: Steps> Redaction<S> {
    /// Begin.
    pub(crate) const fn new(steps: S) -> Self {
        Self { steps }
    }

    /// Step 1: every content edit, across every affected stream.
    ///
    /// # Errors
    ///
    /// The first failure ends the redaction. `self` is consumed, so the partly-edited document
    /// goes out of scope with the error and cannot be emitted from — which is the
    /// poisoned-document rule expressed as ownership rather than as a warning.
    pub(crate) fn edit_content(mut self) -> Result<ContentEdited<S>> {
        let streams = self.steps.affected_streams()?;
        let mut rewritten = 0;
        for stream in streams {
            // NO `continue` ON ERROR, and no collecting failures to report at the end. The
            // first failure means the document is already part-edited, and every later step
            // would be computing from a state nothing describes.
            self.steps.rewrite(stream)?;
            rewritten += 1;
        }
        Ok(ContentEdited {
            steps: self.steps,
            rewritten,
        })
    }
}

impl<S: Steps> ContentEdited<S> {
    /// How many streams were rewritten.
    pub(crate) const fn rewritten(&self) -> usize {
        self.rewritten
    }

    /// Step 2: font surgery, from the **complete** result of step 1.
    ///
    /// # Errors
    ///
    /// As [`Redaction::edit_content`]: the document is discarded with the error.
    pub(crate) fn cut_fonts(mut self) -> Result<FontsCut<S>> {
        // READ AFTER EVERY EDIT, not before any. This call is the reason the type exists: a
        // caller cannot reach it without having finished step 1, so "no longer drawn" is a
        // fact about the finished content rather than about a snapshot taken part-way.
        let still_drawn = self.steps.codes_still_drawn()?;
        self.steps.cut_fonts(&still_drawn)?;
        Ok(FontsCut { steps: self.steps })
    }
}

impl<S: Steps> FontsCut<S> {
    /// Step 3: the page strip.
    ///
    /// # Errors
    ///
    /// As above.
    pub(crate) fn strip_page(mut self) -> Result<Finished<S>> {
        self.steps.strip_page_keys()?;
        Ok(Finished { steps: self.steps })
    }
}

impl<S: Steps> Finished<S> {
    /// Emit the bytes.
    ///
    /// # The crate's own boundary, not a public one
    ///
    /// Reaching this state means every step succeeded. It does **not** mean the output is
    /// verified: that is #134, and until it exists nothing outside this crate may call a path
    /// that reaches here.
    ///
    /// # Errors
    ///
    /// Whatever writing failed with.
    pub(crate) fn emit(mut self) -> Result<Vec<u8>> {
        self.steps.write()
    }
}

/// Run the whole sequence.
///
/// # Errors
///
/// The first failing step's error, with the document discarded. See the module header.
pub(crate) fn run<S: Steps>(steps: S) -> Result<Vec<u8>> {
    Redaction::new(steps)
        .edit_content()?
        .cut_fonts()?
        .strip_page()?
        .emit()
}

/// The refusal a poisoned document produces, so callers can name it.
pub(crate) fn poisoned(detail: &str) -> Error {
    Error::Malformed(format!("pdf redaction [document-poisoned]: {detail}"))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{Redaction, Steps, StreamId, poisoned, run};
    use burrow_types::Result;

    /// What the document was asked to do, in order.
    ///
    /// Shared with the test rather than owned by the fake, because every step **consumes** the
    /// document -- that is the poisoned-document rule -- so a test that owned it could not read
    /// the log back after a failure. Sharing the log does not weaken the rule: the log is not
    /// the document, and nothing can emit from it.
    type Log = Rc<RefCell<Vec<String>>>;

    /// A document that records what was asked of it, and fails whichever step it is told to.
    struct Fake {
        log: Log,
        /// Which rewrite to fail, counting from 1.
        fail_rewrite: Option<usize>,
        /// Fail `codes_still_drawn`, for the step-2 poisoning case.
        fail_codes: bool,
        rewrites: usize,
        streams: Vec<StreamId>,
    }

    impl Fake {
        fn with_streams(count: u64) -> (Self, Log) {
            let log: Log = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    log: Rc::clone(&log),
                    fail_rewrite: None,
                    fail_codes: false,
                    rewrites: 0,
                    streams: core::iter::once(StreamId::Page)
                        .chain((1..count).map(StreamId::Object))
                        .collect(),
                },
                log,
            )
        }

        fn note(&self, entry: &str) {
            self.log.borrow_mut().push(entry.to_owned());
        }
    }

    impl Steps for Fake {
        fn affected_streams(&mut self) -> Result<Vec<StreamId>> {
            self.note("streams");
            Ok(self.streams.clone())
        }

        fn rewrite(&mut self, stream: StreamId) -> Result<()> {
            self.rewrites += 1;
            self.note(&format!("rewrite {stream:?}"));
            if self.fail_rewrite == Some(self.rewrites) {
                return Err(poisoned("a stream rewrite failed"));
            }
            Ok(())
        }

        fn codes_still_drawn(&mut self) -> Result<Vec<(u64, Vec<u32>)>> {
            self.note("codes_still_drawn");
            if self.fail_codes {
                return Err(poisoned("the finished content could not be walked"));
            }
            Ok(vec![(1, vec![65])])
        }

        fn cut_fonts(&mut self, still_drawn: &[(u64, Vec<u32>)]) -> Result<()> {
            self.note(&format!("cut_fonts {}", still_drawn.len()));
            Ok(())
        }

        fn strip_page_keys(&mut self) -> Result<()> {
            self.note("strip_page_keys");
            Ok(())
        }

        fn write(&mut self) -> Result<Vec<u8>> {
            self.note("write");
            Ok(b"%PDF-1.7\n".to_vec())
        }
    }

    #[test]
    fn the_order_is_every_content_edit_then_fonts_then_the_page() {
        // THE ORDER IS THE ASSERTION, and it is asserted against `run` rather than against the
        // same calls made by hand -- a first draft drove the fake directly, which established
        // the fake's order and nothing about the code under test.
        let (fake, log) = Fake::with_streams(4);
        run(fake).expect("a clean run succeeds");

        assert_eq!(
            log.borrow().as_slice(),
            [
                "streams".to_owned(),
                "rewrite Page".to_owned(),
                "rewrite Object(1)".to_owned(),
                "rewrite Object(2)".to_owned(),
                "rewrite Object(3)".to_owned(),
                "codes_still_drawn".to_owned(),
                "cut_fonts 1".to_owned(),
                "strip_page_keys".to_owned(),
                "write".to_owned(),
            ],
            "font surgery must follow the LAST rewrite, and the page strip must be last"
        );
    }

    #[test]
    fn font_surgery_reads_the_content_after_every_edit_not_before_any() {
        // The specific ordering claim the type exists for, stated as an index comparison so a
        // failure names the two steps rather than printing two lists to diff by eye.
        let (fake, log) = Fake::with_streams(4);
        run(fake).expect("succeeds");
        let log = log.borrow();
        let last_rewrite = log
            .iter()
            .rposition(|entry| entry.starts_with("rewrite"))
            .expect("some stream was rewritten");
        let read_codes = log
            .iter()
            .position(|entry| entry == "codes_still_drawn")
            .expect("the finished content was read");
        assert!(
            read_codes > last_rewrite,
            "font surgery read the content at step {read_codes}, before the last rewrite at \
             {last_rewrite} -- entries would survive for codes a later edit removed, and the \
             /ToUnicode of a removed glyph IS the removed character, in the font"
        );
    }

    #[test]
    fn failing_the_third_of_four_rewrites_emits_nothing_and_refuses_by_name() {
        // THE POISONED-DOCUMENT RULE, measured. A half-redacted page is the worst possible
        // output because it looks like a redaction: no partial emission, no retry, no fallback
        // to the partly-edited state.
        let (mut fake, log) = Fake::with_streams(4);
        fake.fail_rewrite = Some(3);
        let error = run(fake).expect_err("the third rewrite fails, so the redaction must refuse");

        assert!(
            format!("{error:?}").contains("[document-poisoned]"),
            "refused, but by a different rule: {error:?}"
        );

        let log = log.borrow();
        assert!(
            !log.iter().any(|entry| entry == "write"),
            "nothing may be emitted from a poisoned document: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("rewrite"))
                .count(),
            3,
            "the fourth rewrite must not have been attempted: {log:?}"
        );
        for later in ["codes_still_drawn", "cut_fonts 1", "strip_page_keys"] {
            assert!(
                !log.iter().any(|entry| entry == later),
                "`{later}` ran after a failed rewrite, computing from a state nothing \
                 describes: {log:?}"
            );
        }
    }

    #[test]
    fn a_failure_in_font_surgery_also_emits_nothing() {
        // The rule is about ANY step, not only the content edits.
        let (mut fake, log) = Fake::with_streams(4);
        fake.fail_codes = true;
        let error = run(fake).expect_err("font surgery fails, so the redaction must refuse");
        assert!(
            format!("{error:?}").contains("[document-poisoned]"),
            "refused, but by a different rule: {error:?}"
        );
        assert!(
            !log.borrow().iter().any(|entry| entry == "write"),
            "nothing may be emitted: {:?}",
            log.borrow()
        );
    }

    #[test]
    fn a_successful_run_rewrites_every_stream_and_emits() {
        // THE NON-VACUITY CONTROL. A `run` that rewrote nothing, or that never reached
        // `write`, would satisfy every assertion above about what does NOT happen.
        let (fake, log) = Fake::with_streams(4);
        let edited = Redaction::new(fake).edit_content().expect("edits");
        assert_eq!(
            edited.rewritten(),
            4,
            "every affected stream must be rewritten, or the failure tests assert nothing"
        );
        let bytes = edited
            .cut_fonts()
            .expect("cuts")
            .strip_page()
            .expect("strips")
            .emit()
            .expect("emits");
        assert!(bytes.starts_with(b"%PDF"), "a successful run emits bytes");
        assert!(log.borrow().iter().any(|entry| entry == "write"));
    }
}
