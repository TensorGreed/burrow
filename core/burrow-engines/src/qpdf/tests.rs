//! Unit tests for the qpdf structure engine.
//!
//! The purely-arithmetic parts live beside the code they test, in `errors.rs`, `ffi.rs`
//! and `limits.rs` — including the bitmask test and the zero-code test. What is here needs
//! a real qpdf call.
//!
//! # These tests are also the exception-safety evidence
//!
//! [`ffi`] declares qpdf's C API as plain `extern "C"` on the strength of upstream's claim
//! that every function catches C++ exceptions internally. That claim is load-bearing: if
//! it were false, an exception would unwind into Rust and the behaviour would be
//! undefined.
//!
//! So rather than trusting it, the tests below drive an error through **every entry point
//! this module calls** and assert a typed `Err` comes back. A process that survives
//! `qpdf_read_memory` on deliberate garbage, on a truncated file, and on an encrypted file
//! with the wrong password is a process where the guarantee held — and if it ever stops
//! holding, these abort rather than pass.

use crate::minimal_pdf;

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Password, Result, Stage};

use super::Qpdf;
use crate::{CheckOptions, StructureEngine, StructureReport};

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn check(bytes: Vec<u8>) -> Result<StructureReport> {
    Qpdf::new().check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    )
}

fn check_with_password(bytes: Vec<u8>, password: &Password) -> Result<StructureReport> {
    let mut options = CheckOptions::new(Limits::default(), stopped());
    options.password = Some(password);
    Qpdf::new().check(bytes.into_boxed_slice(), &options)
}

#[test]
fn the_engine_names_itself() {
    assert_eq!(Qpdf::new().name(), "qpdf");
}

#[test]
fn a_valid_document_reports_its_structure() {
    let report = check(minimal_pdf::pdf_with_pages(3)).expect("a generated pdf should check out");
    assert_eq!(report.pages, 3);
}

#[test]
fn page_counts_agree_with_what_the_generator_wrote() {
    for pages in [1usize, 2, 10, 137] {
        let report = check(minimal_pdf::pdf_with_pages(pages)).expect("should check out");
        assert_eq!(report.pages, u64::try_from(pages).unwrap());
    }
}

/// A file that once aborted the process.
///
/// `qpdf_is_linearized` does not route through qpdf's `trap_errors`, and an object number
/// above `INT_MAX` makes it throw `std::range_error` straight through the FFI boundary.
/// This crate no longer calls it; if it ever does again, this aborts rather than fails.
#[test]
fn an_object_number_above_int_max_is_handled_not_fatal() {
    // The assertion is that this **returns at all**. Whether the document opens is beside
    // the point and depends on the generator's cross-reference table; what mattered before
    // the fix was that the process died here.
    let result = check(minimal_pdf::pdf_with_object_number_above_int_max());
    assert!(
        result.is_ok() || matches!(result, Err(Error::Malformed(_))),
        "expected a typed outcome, got {result:?}"
    );
}

/// The exception-safety evidence, entry point by entry point.
///
/// Each of these drives qpdf into a failure that its C++ core signals by throwing. A
/// typed `Err` here means the C API caught it; an abort or a crash would mean the
/// guarantee `ffi`'s docs rely on is false.
#[test]
fn every_failure_path_returns_a_typed_error_rather_than_unwinding() {
    let cases: [(&str, Vec<u8>); 4] = [
        ("not a pdf at all", minimal_pdf::not_a_pdf()),
        ("truncated mid-document", minimal_pdf::truncated_pdf()),
        ("a valid header and nothing else", b"%PDF-1.7\n".to_vec()),
        ("a single byte", vec![b'%']),
    ];
    for (what, bytes) in cases {
        let result = check(bytes);
        assert!(
            result.is_err(),
            "{what}: expected a typed error, got {result:?}"
        );
    }
}

#[test]
fn bytes_that_are_not_a_pdf_are_malformed() {
    assert!(matches!(
        check(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn empty_input_is_malformed() {
    assert!(matches!(check(Vec::new()), Err(Error::Malformed(_))));
}

/// The reason qpdf earns its place in this PR.
///
/// PDFium reports a password failure through a global error code that is only meaningful
/// immediately after the failing call. qpdf reports it as `qpdf_e_password`, a value with
/// one meaning, which is what makes this mapping safe to rely on.
#[test]
fn an_encrypted_document_without_a_password_is_password_required() {
    match check(minimal_pdf::pdf_encrypted_with_unusable_credentials()) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn an_encrypted_document_with_the_wrong_password_is_password_required() {
    let password = Password::new(b"not the password");
    match check_with_password(
        minimal_pdf::pdf_encrypted_with_unusable_credentials(),
        &password,
    ) {
        Err(Error::PasswordRequired) => {}
        other => panic!("expected PasswordRequired, got {other:?}"),
    }
}

#[test]
fn a_password_containing_a_nul_is_rejected_without_echoing_it() {
    let password = Password::new(b"before\0after");
    let error = check_with_password(minimal_pdf::pdf_with_pages(1), &password)
        .expect_err("a NUL in the password should be rejected");
    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
    let rendered = error.to_string();
    for leaked in ["before", "after"] {
        assert!(
            !rendered.contains(leaked),
            "{leaked:?} leaked into {rendered:?}"
        );
    }
}

#[test]
fn an_oversized_input_is_rejected_before_qpdf_sees_it() {
    let limits = Limits::with(|l| l.max_input_bytes = 16);
    let bytes = minimal_pdf::pdf_with_pages(1);
    let requested = u64::try_from(bytes.len()).unwrap();
    match Qpdf::new().check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            stage,
            requested: r,
            allowed,
        }) => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(stage, Stage::InputSize);
            assert_eq!(r, requested);
            assert_eq!(allowed, 16);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn too_many_pages_is_a_limit_error() {
    let limits = Limits::with(|l| l.max_pages = 2);
    match Qpdf::new().check(
        minimal_pdf::pdf_with_pages(5).into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit, requested, ..
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(requested, 5);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

/// The pre-scan guards qpdf too, not just PDFium. qpdf is a C++ parser and this is
/// untrusted input; its own global limits are a layer under the pre-scan, not instead of
/// it.
#[test]
fn the_declared_size_bomb_is_refused_before_qpdf_parses_it() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures/xref-bomb.pdf");
    let Ok(bytes) = std::fs::read(&path) else {
        // The unit tests must not depend on a fixture the integration tests own; if it is
        // missing, `tests/limits.rs` is where that failure belongs.
        return;
    };
    match check(bytes) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("the pre-scan should have refused the bomb, got {other:?}"),
    }
}

#[test]
fn a_report_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StructureReport>();
}

// What the last redaction told its region check, for the tests that assert on the wiring:
// `LAST_EXPECTATION` and `last_expectation`, which moved to `crate::redact::hooks` with the policy
// (#191).

#[cfg(all(test, feature = "native-engines", burrow_native_engines))]
mod wiring {
    use crate::PageRedactor;
    use crate::pdfsyntax::region::Region;
    use crate::redact::hooks::{LAST_EXPECTATION, last_expectation};
    use burrow_types::{Limits, SystemClock};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    /// A one-page document whose font is cuttable and whose page draws two codes.
    fn document() -> Vec<u8> {
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
        let content = "BT /F1 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (K) Tj ET\n";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [3 0 R 6 0 R] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding \
                 /WinAnsiEncoding /FirstChar 32 /LastChar 94 /Widths {widths} >>"
            ),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_owned(),
        ];
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn redact(page: usize, redacted: &[usize]) -> burrow_types::Result<()> {
        LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = None);
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let covered: BTreeSet<usize> = redacted.iter().copied().collect();
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        super::super::Qpdf
            .redact_page(&document(), page, &covered, region, &options)
            .map(|_| ())
    }

    /// A one-page redaction over a font with `/Differences` AND `/ToUnicode` naming both drawn
    /// codes, numbered so qpdf must renumber it when it writes (#218).
    ///
    /// The font is object 3, ahead of the page that names it, so a writer that emits objects in
    /// traversal order gives it another number -- the condition the old check failed under: it
    /// matched the report's INPUT id against the read-back's OUTPUT id, found no match, and
    /// skipped the font.
    fn renumbered_document() -> Vec<u8> {
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
        let content = "BT /F1 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (K) Tj ET\n";
        let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                    /CMapName /Burrow218 def 1 begincodespacerange <00> <FF> endcodespacerange \
                    2 beginbfchar <4B> <004B> <53> <0053> endbfchar endcmap \
                    CMapName currentdict /CMap defineresource pop end end\n";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [4 0 R 7 0 R] >>".to_owned(),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding \
                 << /BaseEncoding /WinAnsiEncoding /Differences [75 /K 83 /S] >> \
                 /ToUnicode 6 0 R /FirstChar 32 /LastChar 94 /Widths {widths} >>"
            ),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            format!("<< /Length {} >>\nstream\n{cmap}endstream", cmap.len()),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_owned(),
        ];
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn redact_renumbered(skip_narrowing: bool) -> burrow_types::Result<()> {
        use crate::redact::hooks::{NARROWINGS_SKIPPED, SKIP_NARROWING};
        LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = None);
        NARROWINGS_SKIPPED.with(|n| n.set(0));
        SKIP_NARROWING.with(|flag| flag.set(skip_narrowing));
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        let outcome = super::super::Qpdf
            .redact_page(
                &renumbered_document(),
                0,
                &BTreeSet::from([0, 1]),
                region,
                &options,
            )
            .map(|_| ());
        SKIP_NARROWING.with(|flag| flag.set(false));
        outcome
    }

    #[test]
    fn a_cut_font_left_mapping_a_removed_code_is_refused_by_verification_itself() {
        // #218. With the narrowing switched off, `/Differences` still names `/S` and
        // `/ToUnicode` still maps 0x53 after the `S` is removed. The OLD check compared the
        // report's input id with the renumbered output id, skipped the font, and returned `Ok`;
        // a review got exactly this to return `Ok` natively and on the web.
        let outcome = redact_renumbered(true);
        let skipped = crate::redact::hooks::NARROWINGS_SKIPPED.with(std::cell::Cell::get);
        assert!(
            skipped > 0,
            "the narrowing hook was never reached, so this measured nothing"
        );
        match outcome {
            Err(burrow_types::Error::OutputRejected(message)) => assert!(
                message.contains("still maps"),
                "refused, but not for the orphaned mapping: {message}"
            ),
            other => panic!("verification passed a font still mapping a removed code: {other:?}"),
        }
    }

    #[test]
    fn the_same_redaction_with_the_narrowing_on_is_verified_and_names_the_font_it_cut() {
        // THE NEAR-MISS: the refusal above is the narrowing's absence, not the document.
        redact_renumbered(false).expect("a correct redaction of this document verifies");
        let skipped = crate::redact::hooks::NARROWINGS_SKIPPED.with(std::cell::Cell::get);
        assert_eq!(
            skipped, 0,
            "the hook skipped a narrowing it was not asked to"
        );
        let expected = last_expectation().expect("the check was called");
        assert_eq!(
            expected.cut_fonts,
            BTreeSet::from([crate::redact_verify::FontPath {
                form: Vec::new(),
                name: b"F1".to_vec(),
            }]),
            "the check must be told the page font it cut, by the path the writer keeps"
        );
    }

    /// A PDF from its object bodies, numbered from 1, with a correct cross-reference table.
    fn pdf(objects: &[String]) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    fn stream(body: &str) -> String {
        format!("<< /Length {} >>\nstream\n{body}endstream", body.len())
    }

    const WIDTHS: &str = "/FirstChar 32 /LastChar 94 /Widths [556 556 556 556 556 556 556 556 \
        556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
        556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
        556 556 556 556 556 556 556 556 556 556 556]";

    fn helvetica(extra: &str) -> String {
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {extra} {WIDTHS} >>")
    }

    /// Redact page 0 of `bytes` over the band `redact` uses, covering `covered`.
    fn redact_bytes(bytes: &[u8], covered: &[usize]) -> burrow_types::Result<()> {
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        super::super::Qpdf
            .redact_page(
                bytes,
                0,
                &covered.iter().copied().collect(),
                region,
                &options,
            )
            .map(|_| ())
    }

    /// A two-page catalog whose first page carries `resources` and draws `content`; page 2 has
    /// `second_resources`. Objects 1-5 are fixed; `extra` follow from 6.
    fn two_pages(
        resources: &str,
        content: &str,
        second_resources: &str,
        extra: &[String],
    ) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [3 0 R 5 0 R] >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} \
                 /Contents 4 0 R >>"
            ),
            stream(content),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {second_resources} >>"
            ),
        ];
        objects.extend(extra.iter().cloned());
        pdf(&objects)
    }

    // THE OVER-REFUSALS A REVIEW DEMONSTRATED (#218). An earlier fix examined every unshared font
    // the page reached, while the operation narrows only the page's fonts and those its cut
    // glyphs came from; each of these redacted `Ok` before it and was refused by it. The check
    // examines exactly what the operation cut, so each must redact.

    #[test]
    fn a_forms_own_font_nothing_was_cut_from_is_not_examined() {
        // A form below the region draws `A` with its own font, whose `/Differences` also names
        // `B`, which it never draws. Nothing was cut from it; its mappings are the document's.
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> /XObject << /Fm0 7 0 R >> >>",
            "BT /F1 24 Tf 72 700 Td (S) Tj ET\nq /Fm0 Do Q\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << \
                     /F2 8 0 R >> >> /Length {} >>\nstream\nBT /F2 24 Tf 72 300 Td (A) Tj ET\n\
                     endstream",
                    "BT /F2 24 Tf 72 300 Td (A) Tj ET\n".len()
                ),
                helvetica("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [65 /A /B] >>"),
            ],
        );
        redact_bytes(&bytes, &[0]).expect("the form's font was never touched");
    }

    #[test]
    fn a_font_entry_that_is_not_a_dictionary_is_left_alone() {
        // The operation skips a `/Font` entry that is not a dictionary; the check must too.
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R /F9 42 >> >>",
            "BT /F1 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (K) Tj ET\n",
            "<< >>",
            &[helvetica("/Encoding /WinAnsiEncoding")],
        );
        redact_bytes(&bytes, &[0]).expect("a non-dictionary entry is not a font to check");
    }

    #[test]
    fn an_unreadable_to_unicode_on_a_page_outside_the_operation_does_not_refuse() {
        // Page 2's font carries a `/ToUnicode` the parser rejects. The operation never reads it,
        // and neither may the check of page 1.
        let broken = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                      1 begincodespacerange <00> <FF> endcodespacerange \
                      1 beginbfrange <41> <42> <FFFF> endbfrange endcmap end end\n";
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> >>",
            "BT /F1 24 Tf 72 700 Td (S) Tj ET\n",
            "<< /Font << /F2 7 0 R >> >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 8 0 R"),
                stream(broken),
            ],
        );
        redact_bytes(&bytes, &[0]).expect("page 2's fonts are not this redaction's to check");
    }

    #[test]
    fn an_annotation_appearance_font_outside_the_region_is_not_examined() {
        // A widget below the region, whose appearance draws `A` in a font whose `/Differences`
        // names `A` and `B`: the common filled-form shape. The operation does not narrow it.
        let appearance = "BT /Helv 12 Tf 2 2 Td (A) Tj ET\n";
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> >> /Annots [<< /Type /Annot /Subtype /Widget \
             /Rect [72 300 172 320] /AP << /N 7 0 R >> >>]",
            "BT /F1 24 Tf 72 700 Td (S) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << \
                     /Helv 8 0 R >> >> /Length {} >>\nstream\n{appearance}endstream",
                    appearance.len()
                ),
                helvetica("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [65 /A /B] >>"),
            ],
        );
        assert!(
            String::from_utf8_lossy(&bytes).contains("/Annots [<< /Type /Annot"),
            "the annotation is not on the page"
        );
        redact_bytes(&bytes, &[0]).expect("an annotation's appearance font was never touched");
    }

    #[test]
    fn two_fonts_written_inline_are_refused_by_name() {
        // `[direct-font]` (#218): both fonts are `(0, 0)` to the engine, so the operation
        // narrowed only the first and the check merged them; a review got that to return `Ok`
        // with a removed character still mapped.
        let bytes = two_pages(
            &format!(
                "<< /Font << /F1 {} /F2 {} >> >>",
                helvetica("/Encoding /WinAnsiEncoding"),
                helvetica("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [83 /S] >>")
            ),
            "BT /F2 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (S) Tj ET\n",
            "<< >>",
            &[],
        );
        match redact_bytes(&bytes, &[0]) {
            Err(burrow_types::Error::Unsupported(message)) => {
                assert!(
                    message.contains("[direct-font]"),
                    "refused, but not by name: {message}"
                );
            }
            other => panic!("two inline fonts were not refused: {other:?}"),
        }
    }

    #[test]
    fn the_same_fonts_written_indirectly_are_redacted() {
        // THE NEAR-MISS: the refusal is the fonts' lack of identity, not the document.
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R /F2 7 0 R >> >>",
            "BT /F2 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (S) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                helvetica("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [83 /S] >>"),
            ],
        );
        redact_bytes(&bytes, &[0]).expect("two indirect fonts have identities to check by");
    }

    /// Page → `X0` → `X1`, where `X1`'s own `/F9` draws `S` inside the region. With `decoy`,
    /// the page also has an `/F9` of its own, drawing `S` below the region (#221).
    fn nested(decoy: bool) -> Vec<u8> {
        let inner = "BT /F9 24 Tf 72 700 Td (S) Tj ET\n";
        let outer = "/X1 Do\n";
        let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                    1 begincodespacerange <00> <FF> endcodespacerange \
                    1 beginbfchar <53> <0058> endbfchar endcmap end end\n";
        let (page_resources, content) = if decoy {
            (
                "<< /Font << /F9 6 0 R >> /XObject << /X0 7 0 R >> >>",
                "BT /F9 24 Tf 72 300 Td (S) Tj ET\nq /X0 Do Q\n",
            )
        } else {
            ("<< /XObject << /X0 7 0 R >> >>", "q /X0 Do Q\n")
        };
        two_pages(
            page_resources,
            content,
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << \
                     /XObject << /X1 8 0 R >> >> /Length {} >>\nstream\n{outer}endstream",
                    outer.len()
                ),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << \
                     /Font << /F9 9 0 R >> >> /Length {} >>\nstream\n{inner}endstream",
                    inner.len()
                ),
                helvetica(
                    "/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [83 /Sacute] >> \
                     /ToUnicode 10 0 R",
                ),
                stream(cmap),
            ],
        )
    }

    #[test]
    fn a_glyph_in_a_nested_form_narrows_the_font_that_drew_it_not_a_same_named_page_font() {
        // #221: resolution looked for the form among the page's own `/XObject` and fell back to
        // the page's `/F9`, so the decoy was narrowed, the real font kept `/Sacute`, and the
        // redaction returned `Ok`. The path recorded is now the real one.
        LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = None);
        redact_bytes(&nested(true), &[0]).expect("the nested font is found and narrowed");
        let expected = last_expectation().expect("the check was called");
        let real = crate::redact_verify::FontPath {
            form: vec![b"X0".to_vec(), b"X1".to_vec()],
            name: b"F9".to_vec(),
        };
        assert!(
            expected.cut_fonts.contains(&real),
            "the font that drew the removed glyph was not the one cut: {:?}",
            expected.cut_fonts
        );
    }

    #[test]
    fn the_nested_font_left_mapping_the_removed_code_is_refused() {
        // AND THE CHECK EXAMINES THAT FONT: with the narrowing off, the real `/F9` still maps
        // `S`, and verification refuses rather than passing over the decoy.
        use crate::redact::hooks::{NARROWINGS_SKIPPED, SKIP_NARROWING};
        NARROWINGS_SKIPPED.with(|n| n.set(0));
        SKIP_NARROWING.with(|flag| flag.set(true));
        let outcome = redact_bytes(&nested(true), &[0]);
        SKIP_NARROWING.with(|flag| flag.set(false));
        assert!(
            NARROWINGS_SKIPPED.with(std::cell::Cell::get) > 0,
            "the narrowing hook was never reached"
        );
        match outcome {
            Err(burrow_types::Error::OutputRejected(message)) => {
                assert!(
                    message.contains("still maps"),
                    "refused, but not for the mapping: {message}"
                );
            }
            other => panic!("the nested font's mapping of a removed code passed: {other:?}"),
        }
    }

    #[test]
    fn a_nested_form_with_no_same_named_page_font_is_redacted() {
        // Without the decoy the old resolution found nothing and failed with `Internal` --
        // burrow disagreeing with itself over a valid document.
        redact_bytes(&nested(false), &[0]).expect("a nested form's own font is found");
    }

    #[test]
    fn the_check_is_told_the_fonts_the_report_says_were_cut() {
        // KILLS: forcing `cut_fonts` empty in the observe closure, which disables the mapping
        // assertion for every document. Nothing else in the suite observes this argument.
        redact(0, &[0, 1]).expect("a redactable document");
        let expected = last_expectation().expect("the check was called");
        assert!(
            !expected.cut_fonts.is_empty(),
            "the page's font is cuttable and the check must be told so; it was told {:?}",
            expected.cut_fonts
        );
    }

    #[test]
    fn the_check_is_told_the_page_that_was_redacted() {
        // KILLS: `Cleared { page: 0 }` instead of `page`. Every end-to-end test redacts page 0,
        // so a verification that always checked page 0 would ship.
        redact(1, &[1]).ok();
        let expected = last_expectation().expect("the check was called");
        assert_eq!(
            expected.page, 1,
            "the check must be told the page it verifies"
        );
    }

    #[test]
    fn the_check_is_told_the_region_that_was_asked_for() {
        // KILLS: a zeroed or constant region reaching the check. A zero region makes check 1
        // pass over every document.
        redact(0, &[0, 1]).expect("a redactable document");
        let expected = last_expectation().expect("the check was called");
        assert!(
            expected.region.width > 1.0 && expected.region.height > 1.0,
            "the check must be told a real region, got {:?}",
            expected.region
        );
    }
}

// `FORCED_FAILURE` moved to `crate::redact::hooks` with the policy (#191).

#[cfg(all(test, feature = "native-engines", burrow_native_engines))]
mod propagation {
    use super::super::Qpdf;
    use crate::PageRedactor;
    use crate::pdfsyntax::region::Region;
    use crate::redact::hooks::FORCED_FAILURE;
    use burrow_types::{Limits, SystemClock};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn attempt() -> burrow_types::Result<Vec<u8>> {
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
        let content = "BT /F1 24 Tf 72 700 Td (S) Tj ET\nBT /F1 24 Tf 72 300 Td (K) Tj ET\n";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding \
                 /WinAnsiEncoding /FirstChar 32 /LastChar 94 /Widths {widths} >>"
            ),
        ];
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));

        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let covered: BTreeSet<usize> = [0].into_iter().collect();
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        Qpdf.redact_page(out.as_bytes(), 0, &covered, region, &options)
            .map(|(bytes, _)| bytes)
    }

    #[test]
    fn a_failing_read_back_stops_the_operation_returning_bytes() {
        // KILLS: discarding the verification's result. That mutation survived the entire suite
        // before this existed -- #134's central claim, with nothing behind it on the path that
        // ships.
        //
        // NON-VACUITY FIRST: the same document must succeed with the hook off, or this test
        // would pass for an operation that refuses everything.
        FORCED_FAILURE.with(|flag| flag.set(false));
        let bytes = attempt().expect("the document redacts when the read-back works");
        assert!(bytes.starts_with(b"%PDF"));

        FORCED_FAILURE.with(|flag| flag.set(true));
        let result = attempt();
        FORCED_FAILURE.with(|flag| flag.set(false));

        let error = result.expect_err("a failing read-back must stop the bytes");
        let text = format!("{error}");
        assert!(
            text.contains("planted read-back failure"),
            "the read-back's failure must reach the caller: {text}"
        );
    }
}
