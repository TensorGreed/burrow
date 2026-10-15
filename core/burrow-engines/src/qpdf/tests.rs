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

    #[test]
    fn the_renumbered_fixture_is_renumbered() {
        // THE PREMISE, asserted rather than assumed: the tests below say the old id-matching
        // returned `Ok` because qpdf renumbered the font. A writer that kept object 3 would turn
        // them into tests over a font nothing renumbered, and they would still pass.
        use crate::redact::graph::{OpensForRedaction, PdfDocument};
        let output = redact_output(&renumbered_document(), &[0]).expect("redacts");
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let (document, _) = super::super::Qpdf
            .open_for_redaction(&output, &options)
            .expect("the output opens");
        let page = document.page(0).expect("page 0");
        let resources = crate::redact::resources::PageResources::of(&page).expect("resources");
        let font = resources
            .font_at(&crate::redact_verify::FontPath {
                form: Vec::new(),
                name: b"F1".to_vec(),
            })
            .expect("the font is in the output");
        let (number, _) = font.object().expect("an indirect font");
        assert_ne!(
            number, 3,
            "qpdf kept the font's number; nothing was renumbered"
        );
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
        redact_output(bytes, covered).map(|_| ())
    }

    /// [`redact_bytes`], keeping the output.
    fn redact_output(bytes: &[u8], covered: &[usize]) -> burrow_types::Result<Vec<u8>> {
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
            .map(|(output, _)| output)
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

    /// A form `X` with no `/Resources` of its own, listed twice: on the page, never drawn there,
    /// and inside form `A`, which draws it -- so `X`'s glyphs are drawn with `A`'s `/F1`, `S`.
    /// The page draws `(S)` in the region with its own `/F1`, `P`. With `a_draws_too`, `A` also
    /// draws `(S)` in the region with `S`, and `X` draws `(K)` with `S` below it. `order` names
    /// the page's two `/XObject` entries, `(for X, for A)`: qpdf sorts them, and the search this
    /// replaces took whichever came first (#218, round 3).
    fn two_scopes(order: (&str, &str), a_draws_too: bool) -> Vec<u8> {
        let (x, a) = order;
        let cmap = |pairs: &str, n: usize| {
            stream(&format!(
                "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
                 1 begincodespacerange <00> <FF> endcodespacerange \
                 {n} beginbfchar {pairs} endbfchar endcmap end end\n"
            ))
        };
        let form = |resources: &str, body: &str| {
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] {resources} /Length {} \
                 >>\nstream\n{body}endstream",
                body.len()
            )
        };
        let (page_content, a_body, x_body) = if a_draws_too {
            (
                format!("q /{a} Do Q\n"),
                "BT /F1 24 Tf 72 700 Td (S) Tj ET\nq /X Do Q\n",
                "BT /F1 24 Tf 72 300 Td (K) Tj ET\n",
            )
        } else {
            (
                format!("BT /F1 24 Tf 72 700 Td (S) Tj ET\nq /{a} Do Q\n"),
                "q /X Do Q\n",
                "BT /F1 24 Tf 72 300 Td (S) Tj ET\n",
            )
        };
        two_pages(
            &format!("<< /Font << /F1 6 0 R >> /XObject << /{x} 8 0 R /{a} 9 0 R >> >>"),
            &page_content,
            "<< >>",
            &[
                // 6, 7: P, the page's font. `S` is `Q` to it.
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 7 0 R"),
                cmap("<53> <0051>", 1),
                // 8: X, no resources of its own.
                form("", x_body),
                // 9: A, whose `/F1` is S and whose `/X` is X.
                form(
                    "/Resources << /Font << /F1 10 0 R >> /XObject << /X 8 0 R >> >>",
                    a_body,
                ),
                // 10, 11: S. `S` is `x` to it, and `K` is `K`.
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 11 0 R"),
                cmap("<4B> <004B> <53> <0078>", 2),
            ],
        )
    }

    /// Whether the font at `form`/`name` on page 0 of `output` still maps `code`, read back
    /// through the engine by a path the test names. NOT A BYTE SEARCH: qpdf flates on write, so
    /// a search of the output for a CMap entry finds nothing and passes every absence test.
    fn maps(output: &[u8], form: &[&str], name: &str, code: u32) -> bool {
        use crate::redact::graph::{OpensForRedaction, PdfDocument};
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let (document, _) = super::super::Qpdf
            .open_for_redaction(output, &options)
            .expect("the output opens");
        let page = document.page(0).expect("page 0");
        let resources = crate::redact::resources::PageResources::of(&page).expect("resources");
        let font = resources
            .font_at(&crate::redact_verify::FontPath {
                form: form.iter().map(|step| step.as_bytes().to_vec()).collect(),
                name: name.as_bytes().to_vec(),
            })
            .expect("the font the test names is in the output");
        let to_unicode = font.key(&crate::name::Name::literal(b"/ToUnicode\0"));
        // Removed outright when nothing it mapped is still drawn.
        if to_unicode.type_code() != crate::codes::qpdf::object_type::STREAM {
            return false;
        }
        match to_unicode.stream_data().expect("readable") {
            None => panic!("a /ToUnicode the test cannot decode answers nothing"),
            Some(program) => crate::pdfsyntax::tounicode::ToUnicode::parse(&program)
                .expect("a CMap burrow wrote or kept")
                .maps(code),
        }
    }

    #[test]
    fn a_form_drawn_from_two_scopes_credits_its_glyphs_to_the_scope_that_drew_them() {
        // #218 round 3: `X`'s `(S)` is drawn with `S`, through `A`. Credited to `P` instead --
        // the page's `/F1`, reached through the page's never-drawn `/XObject` entry for `X` --
        // it kept `P`'s mapping of the removed `S` alive, and the read-back made the same
        // mistake, so it returned `Ok`. In one key order and not the other.
        for order in [("B", "Z"), ("Z", "B")] {
            let output = redact_output(&two_scopes(order, false), &[0])
                .unwrap_or_else(|error| panic!("{order:?}: refused: {error:?}"));
            assert!(
                maps(&output, &[order.1], "F1", 0x53),
                "{order:?}: S, which still draws S through A, lost its mapping"
            );
            assert!(
                !maps(&output, &[], "F1", 0x53),
                "{order:?}: P still maps the S the page no longer draws"
            );
        }
    }

    #[test]
    fn a_form_drawn_from_two_scopes_keeps_what_it_still_draws() {
        // THE MIRROR: `A` draws `(S)` in the region with `S`, and `X` still draws `(K)` with
        // `S`. `K` credited to `P` left `S` drawing nothing, so it lost its `/ToUnicode` and
        // every width -- `Ok`, and the `K` on a part of the page nobody selected corrupted.
        for order in [("B", "Z"), ("Z", "B")] {
            let output = redact_output(&two_scopes(order, true), &[0])
                .unwrap_or_else(|error| panic!("{order:?}: refused: {error:?}"));
            assert!(
                maps(&output, &[order.1], "F1", 0x4B),
                "{order:?}: the K still drawn below the region lost its mapping"
            );
            assert!(
                !maps(&output, &[order.1], "F1", 0x53),
                "{order:?}: the S removed from the region is still mapped"
            );
        }
    }

    /// A Form XObject stream with `resources` and `body`, for the round-4 fixtures.
    fn form_stream(resources: &str, body: &str) -> String {
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] {resources} /Length {} >>\n\
             stream\n{body}endstream",
            body.len()
        )
    }

    /// A `/ToUnicode` program mapping each `(code, unicode)` pair, in hex.
    fn mappings(pairs: &[(&str, &str)]) -> String {
        let entries: String = pairs
            .iter()
            .map(|(code, unicode)| format!("<{code}> <{unicode}> "))
            .collect();
        stream(&format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
             1 begincodespacerange <00> <FF> endcodespacerange \
             {} beginbfchar {entries}endbfchar endcmap end end\n",
            pairs.len()
        ))
    }

    fn refused_as(outcome: burrow_types::Result<()>, rule: &str) {
        match outcome {
            Err(burrow_types::Error::Unsupported(message)) => assert!(
                message.contains(rule),
                "refused, but not as [{rule}]: {message}"
            ),
            other => panic!("expected [{rule}], got {other:?}"),
        }
    }

    #[test]
    fn a_secret_shown_in_a_form_with_the_pages_font_is_refused_not_misplaced() {
        // #218 round 4, and older than #218: the page selects `/F1` -- 556 wide -- and draws
        // `X`, whose own `/F1` is zero wide and which shows `AAASECRET` with no `Tf` of its own.
        // PDFium draws it with the page's font, so `SECRET` lands inside the region; this walk
        // resolved `/F1` in `X`'s resources, placed every glyph at x = 20 with no width, found
        // nothing to remove, and returned `Ok` with the secret in the output in plain text.
        let zero_widths = format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths [{}] >>",
            "0 ".repeat(63)
        );
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> /XObject << /X 8 0 R >> >>",
            "BT /F1 24 Tf 72 300 Td (ACERST) Tj ET\nq /X Do Q\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                zero_widths,
                form_stream(
                    "/Resources << /Font << /F1 7 0 R >> >>",
                    "BT 20 700 Td (AAASECRET) Tj ET\n",
                ),
            ],
        );
        refused_as(redact_bytes(&bytes, &[0]), "font-selected-in-another-scope");
    }

    #[test]
    fn a_removed_glyph_shown_with_an_inherited_font_is_refused_not_credited_elsewhere() {
        // #218 round 4, and a regression of this branch: `A` selects its `/F1` (`S` is `Q` to
        // it), ends the text object, and draws `B`, whose own `/F1` (`S` is `x`) is never
        // selected. `B` shows `S` in the region. PDFium extracts `Q`: the font is `A`'s. The
        // route credited the glyph to `B`'s font, narrowed that, and returned `Ok` with `A`'s
        // font still mapping the removed `S`. `main` refused it, as `Internal`.
        let bytes = two_pages(
            "<< /XObject << /A 6 0 R >> >>",
            "q /A Do Q\n",
            "<< >>",
            &[
                form_stream(
                    "/Resources << /Font << /F1 7 0 R >> /XObject << /B 9 0 R >> >>",
                    "BT /F1 24 Tf ET\n/B Do\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 8 0 R"),
                mappings(&[("53", "0051")]),
                form_stream(
                    "/Resources << /Font << /F1 10 0 R >> >>",
                    "BT 72 700 Td (S) Tj ET\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 11 0 R"),
                mappings(&[("53", "0078")]),
            ],
        );
        refused_as(redact_bytes(&bytes, &[0]), "font-selected-in-another-scope");
    }

    /// One name, two fonts, one page, both still drawing: the page's `/F1` (`P`) draws `S` in
    /// the region and `K` below it; form `X`'s own `/F1` (`F`) draws `S` below it. Resolution
    /// cached by NAME would credit `X`'s `S` to whichever `/F1` it met first (#218 round 4).
    fn one_name_two_fonts() -> Vec<u8> {
        two_pages(
            "<< /Font << /F1 6 0 R >> /XObject << /X 8 0 R >> >>",
            "BT /F1 24 Tf 72 700 Td (S) Tj 72 -400 Td (K) Tj ET\nq /X Do Q\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 7 0 R"),
                mappings(&[("4B", "004B"), ("53", "0051")]),
                form_stream(
                    "/Resources << /Font << /F1 9 0 R >> >>",
                    "BT /F1 24 Tf 72 300 Td (S) Tj ET\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 10 0 R"),
                mappings(&[("53", "0078")]),
            ],
        )
    }

    #[test]
    fn one_name_for_two_fonts_on_a_page_is_resolved_per_scope() {
        // The operation's cache: `X`'s `S` credited to `P` keeps `P` mapping the removed `S`.
        let output = redact_output(&one_name_two_fonts(), &[0]).expect("redacts");
        assert!(
            !maps(&output, &[], "F1", 0x53),
            "the page font still maps the S removed from the page"
        );
        assert!(
            maps(&output, &[], "F1", 0x4B),
            "the page font lost the K it still draws"
        );
        assert!(
            maps(&output, &["X"], "F1", 0x53),
            "the form's font lost the S it still draws"
        );
    }

    #[test]
    fn one_name_for_two_fonts_is_resolved_per_scope_by_the_read_back_too() {
        // THE READ-BACK'S CACHE: with the narrowing off, `P` still maps the removed `S`, and a
        // read-back crediting `X`'s `S` to `P` would call that mapping drawn and pass it.
        use crate::redact::hooks::{NARROWINGS_SKIPPED, SKIP_NARROWING};
        NARROWINGS_SKIPPED.with(|n| n.set(0));
        SKIP_NARROWING.with(|flag| flag.set(true));
        let outcome = redact_bytes(&one_name_two_fonts(), &[0]);
        SKIP_NARROWING.with(|flag| flag.set(false));
        assert!(
            NARROWINGS_SKIPPED.with(std::cell::Cell::get) > 0,
            "the narrowing hook was never reached"
        );
        match outcome {
            Err(burrow_types::Error::OutputRejected(message)) => assert!(
                message.contains("still maps"),
                "refused, but not for the mapping: {message}"
            ),
            other => panic!("the page font's mapping of a removed code passed: {other:?}"),
        }
    }

    #[test]
    fn one_name_for_two_fonts_is_cached_per_scope_when_the_form_comes_first() {
        // THE CACHE'S OTHER HALF (#218 round 5): the tests above meet the page's `/F1` first, so
        // a cache that looked up by scope but STORED by name passed them. Here form `X`'s `/F1`
        // (`F`) is met first: it draws `S` in the region and `K` below it, and the page's `/F1`
        // (`P`) then draws `S` below. Stored by name, `P`'s `S` is credited to `F`, which then
        // keeps mapping the removed `S` -- with both caches mutated, `Ok`.
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> /XObject << /X 8 0 R >> >>",
            "q /X Do Q\nBT /F1 24 Tf 72 300 Td (S) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 7 0 R"),
                mappings(&[("53", "0071")]),
                form_stream(
                    "/Resources << /Font << /F1 9 0 R >> >>",
                    "BT /F1 24 Tf 72 700 Td (S) Tj 0 -300 Td (K) Tj ET\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 10 0 R"),
                mappings(&[("4B", "004B"), ("53", "0078")]),
            ],
        );
        let output = redact_output(&bytes, &[0]).expect("redacts");
        assert!(
            !maps(&output, &["X"], "F1", 0x53),
            "the form's font still maps the S removed from the form"
        );
        assert!(
            maps(&output, &["X"], "F1", 0x4B),
            "the form's font lost the K it still draws"
        );
        assert!(
            maps(&output, &[], "F1", 0x53),
            "the page font lost the S it still draws"
        );
    }

    #[test]
    fn a_font_selected_one_form_up_under_the_same_name_is_still_another_scope() {
        // #218 round 5: form `A`, drawn as `/X`, selects its `/F1` and draws a DIFFERENT form
        // through its own `/X`. The routes are `[X]` and `[X, X]`: equal in their last step, so
        // a comparison of only that passed, and `A`'s font kept mapping the removed `S`.
        let bytes = two_pages(
            "<< /XObject << /X 6 0 R >> >>",
            "q /X Do Q\n",
            "<< >>",
            &[
                form_stream(
                    "/Resources << /Font << /F1 7 0 R >> /XObject << /X 9 0 R >> >>",
                    "BT /F1 24 Tf ET\n/X Do\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 8 0 R"),
                mappings(&[("53", "0051")]),
                form_stream(
                    "/Resources << /Font << /F1 10 0 R >> >>",
                    "BT 72 700 Td (S) Tj ET\n",
                ),
                helvetica("/Encoding /WinAnsiEncoding /ToUnicode 11 0 R"),
                mappings(&[("53", "0078")]),
            ],
        );
        refused_as(redact_bytes(&bytes, &[0]), "font-selected-in-another-scope");
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
    fn the_check_holds_its_read_back_to_the_reports_own_cut_count() {
        // #218 round 7: the gate compares the fonts examined with `Cleared::cut`. Were `cut`
        // filled from the paths handed over rather than from the report, a hand-off that lost a
        // path would lower both sides together -- and the suite stayed green with exactly that
        // planted. Two fonts cut, and the check must be told two, from the report.
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = None);
        let two_fonts = two_pages(
            "<< /Font << /F1 6 0 R /F2 7 0 R >> >>",
            "BT /F1 24 Tf 72 700 Td (S) Tj /F2 24 Tf 72 0 Td (K) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                helvetica("/Encoding /WinAnsiEncoding"),
            ],
        );
        let (_, report) = super::super::Qpdf
            .redact_page(&two_fonts, 0, &BTreeSet::from([0]), region, &options)
            .expect("redacts");
        let reported = report.fonts.iter().filter(|font| font.cut).count();
        assert_eq!(reported, 2, "the fixture cuts both of the page's fonts");
        let expected = last_expectation().expect("the check was called");
        assert_eq!(
            expected.cut, reported,
            "the check's count is not the report's"
        );
        assert_eq!(
            expected.cut_fonts.len(),
            reported,
            "the check was handed fewer paths than the report cut"
        );
    }

    /// A one-page document: the page carries `page_extra`, its `/Pages` parent `parent_extra`.
    /// `SECRET` sits at (20, 350) and `KEEP` at (20, 50), both in object 5's Helvetica (#224).
    fn page_under_a_tree(page_extra: &str, parent_extra: &str) -> Vec<u8> {
        let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\nBT /F1 12 Tf 20 50 Td (KEEP) Tj ET\n";
        pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Count 1 /Kids [3 0 R] {parent_extra} >>"),
            format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R {page_extra} >>"),
            stream(content),
            helvetica("/Encoding /WinAnsiEncoding"),
        ])
    }

    /// The band `SECRET` sits in on a 300 x 400 page, measured from the top.
    const UPPER_BAND: Region = Region {
        left: 0.0,
        top: 30.0,
        width: 300.0,
        height: 40.0,
    };

    fn redact_in(bytes: &[u8], region: Region) -> burrow_types::Result<Vec<u8>> {
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        super::super::Qpdf
            .redact_page(bytes, 0, &BTreeSet::from([0]), region, &options)
            .map(|(output, _)| output)
    }

    fn refused_by(outcome: burrow_types::Result<Vec<u8>>, rule: &str, what: &str) {
        match outcome {
            Err(error) => assert!(
                format!("{error:?}").contains(&format!("[{rule}]")),
                "{what}: refused, but not by [{rule}]: {error:?}"
            ),
            Ok(_) => panic!("{what}: redacted, where [{rule}] must refuse"),
        }
    }

    const RESOURCES: &str = "/Resources << /Font << /F1 5 0 R >> >>";
    const BOX: &str = "/MediaBox [0 0 300 400]";

    #[test]
    fn a_page_attribute_the_tree_supplies_is_refused_whether_the_page_is_null_or_silent() {
        // #224: a page that sets /Resources, /CropBox or /Rotate to null is shown by PDFium
        // WITHOUT its ancestor's, and qpdf reads that null as an absent key -- so burrow took the
        // ancestor's, measured against a page the viewer did not show, and returned Ok with the
        // secret intact. The null cannot be seen, so the inherited value is refused either way;
        // and the same value on the page itself -- the near-miss -- redacts.
        let cases: [(&str, String, String, String); 3] = [
            (
                "/Resources",
                BOX.to_owned(),
                RESOURCES.to_owned(),
                format!("{BOX} {RESOURCES}"),
            ),
            (
                "/CropBox",
                format!("{BOX} {RESOURCES}"),
                "/CropBox [0 0 300 400]".to_owned(),
                format!("{BOX} {RESOURCES} /CropBox [0 0 300 400]"),
            ),
            (
                "/Rotate",
                format!("{BOX} {RESOURCES}"),
                "/Rotate 90".to_owned(),
                format!("{BOX} {RESOURCES} /Rotate 90"),
            ),
        ];
        for (key, page, parent, declared) in cases {
            refused_by(
                redact_in(
                    &page_under_a_tree(&format!("{page} {key} null"), &parent),
                    UPPER_BAND,
                ),
                "page-attribute-inherited",
                &format!("{key} null over the tree"),
            );
            refused_by(
                redact_in(&page_under_a_tree(&page, &parent), UPPER_BAND),
                "page-attribute-inherited",
                &format!("{key} absent over the tree"),
            );
            redact_in(&page_under_a_tree(&declared, ""), UPPER_BAND).unwrap_or_else(|error| {
                panic!("{key} declared on the page must redact: {error:?}")
            });
        }
    }

    #[test]
    fn a_media_box_the_tree_supplies_redacts_only_where_the_renderer_agrees() {
        // #224, /MediaBox: inherited in 20 of 200 documents measured, so refusing it was not
        // affordable. PDFium's size decides instead -- a null there gives US Letter.
        //
        // INHERITED AND AGREED: redacts, and the secret really goes.
        let inherited = page_under_a_tree(RESOURCES, BOX);
        let output = redact_in(&inherited, UPPER_BAND).expect("the renderer shows this box");
        let expanded = crate::pdf_reading::expanded(&output);
        assert!(
            !expanded.windows(6).any(|window| window == b"SECRET"),
            "redacted, but SECRET is still in the output"
        );
        assert!(
            expanded.windows(4).any(|window| window == b"KEEP"),
            "KEEP, below the region, was removed: the region was not where it was drawn"
        );
        // NULL: PDFium shows Letter, burrow read 300 x 400.
        refused_by(
            redact_in(
                &page_under_a_tree(&format!("{RESOURCES} /MediaBox null"), BOX),
                UPPER_BAND,
            ),
            "media-box-unverified",
            "/MediaBox null over the tree",
        );
        // A LETTER-SIZED BOX AWAY FROM THE ORIGIN: the renderer's size is the same whether it
        // used this box or its null fallback, so the size cannot decide, absent or not.
        refused_by(
            redact_in(
                &page_under_a_tree(RESOURCES, "/MediaBox [100 100 712 892]"),
                UPPER_BAND,
            ),
            "media-box-unverified",
            "a Letter-sized box away from the origin",
        );
        // THE PAGE'S OWN /CropBox DECIDES: both readers show at most that box. Its null /MediaBox
        // is refused since 2026-10-14 as a value qpdf drops on write (`[null-value]`); the same
        // crop box over no /MediaBox of the page's own still redacts.
        let refused = redact_in(
            &page_under_a_tree(
                &format!("{RESOURCES} /MediaBox null /CropBox [0 0 300 400]"),
                BOX,
            ),
            UPPER_BAND,
        );
        assert!(
            matches!(&refused, Err(error) if format!("{error:?}").contains("[null-value]")),
            "a null /MediaBox under the page's own crop box: {refused:?}"
        );
        redact_in(
            &page_under_a_tree(&format!("{RESOURCES} /CropBox [0 0 300 400]"), BOX),
            UPPER_BAND,
        )
        .expect("the page's own crop box is what both show");
    }

    #[test]
    fn a_media_box_check_compares_each_axis_the_rotation_and_the_shown_box() {
        // #224, round 2: each case is one the first tests could not tell from a mutation of the
        // check -- every earlier fixture was one unrotated 300 x 400 box whose crop was itself,
        // so both axes always differed together and the shown box was the inherited one.
        //
        // ONE AXIS AT A TIME: a null over a box that differs from US Letter in width only, then
        // in height only. A check that compared one axis would pass one of them.
        for (parent, which) in [
            ("/MediaBox [50 0 562 792]", "width only"),
            ("/MediaBox [0 0 612 700]", "height only"),
        ] {
            refused_by(
                redact_in(
                    &page_under_a_tree(&format!("{RESOURCES} /MediaBox null"), parent),
                    UPPER_BAND,
                ),
                "media-box-unverified",
                &format!("a null over a box differing from Letter in {which}"),
            );
        }
        // ROTATED: burrow shows the inherited 792 x 612 turned to 612 x 792; PDFium shows Letter
        // turned to 792 x 612. Compared unrotated, the two sizes are equal.
        refused_by(
            redact_in(
                &page_under_a_tree(
                    &format!("{RESOURCES} /MediaBox null /Rotate 90"),
                    "/MediaBox [0 0 792 612]",
                ),
                UPPER_BAND,
            ),
            "media-box-unverified",
            "a null under a rotated page",
        );
        // THE SHOWN BOX, not the inherited one: the page's own crop is half the inherited box,
        // and both readers show the crop. Compared against the inherited box, this refuses.
        redact_in(
            &page_under_a_tree(&format!("{RESOURCES} /CropBox [0 0 300 200]"), BOX),
            UPPER_BAND,
        )
        .expect("both readers show the page's own crop");
        // THE TOLERANCE: 38 points apart -- PDFium clips the crop to Letter, burrow to the
        // inherited box. A tolerance of tens of points passes it.
        refused_by(
            redact_in(
                &page_under_a_tree(
                    &format!("{RESOURCES} /MediaBox null /CropBox [0 0 612 830]"),
                    "/MediaBox [0 0 612 830]",
                ),
                UPPER_BAND,
            ),
            "media-box-unverified",
            "a crop clipped to Letter by one reader and not the other",
        );
        // AND LETTER AT THE ORIGIN, INHERITED, redacts: a null there gives the same box.
        redact_in(
            &page_under_a_tree(RESOURCES, "/MediaBox [0 0 612 792]"),
            UPPER_BAND,
        )
        .expect("an inherited Letter box at the origin is the box a null would give");
    }

    #[test]
    fn a_rotate_that_is_not_a_number_is_refused_by_the_frame_reader() {
        // #224: `/Rotate [90]` -- burrow's reader pulled 90 out of the text, and PDFium, which
        // reads by type, shows the page upright. On `burrow_ops`' path the rotation reader refuses
        // it first; this is the engine's own reader, which a direct caller meets.
        refused_by(
            redact_in(
                &page_under_a_tree(&format!("{BOX} {RESOURCES} /Rotate [90]"), ""),
                UPPER_BAND,
            ),
            "page-frame-unreadable",
            "/Rotate [90]",
        );
    }

    #[test]
    fn a_crop_wider_than_both_media_boxes_is_refused() {
        // #224, code review round 2: burrow clips the crop to the inherited box, PDFium to US
        // Letter -- both 612 x 792, 100 points apart. The size agreed and the secret stayed.
        refused_by(
            redact_in(
                &page_under_a_tree(
                    &format!("{RESOURCES} /MediaBox null /CropBox [0 0 812 792]"),
                    "/MediaBox [100 0 712 792]",
                ),
                UPPER_BAND,
            ),
            "media-box-unverified",
            "a crop wider than both media boxes",
        );
    }

    #[test]
    fn a_renderer_that_cannot_answer_is_this_rules_refusal_and_a_limit_stays_a_limit() {
        // #224, code review round 2: the mapping of the renderer's errors had no witness -- no
        // renderer in the suite ever failed. Asked directly, with each failure planted.
        use crate::redact::graph::{OpensForRedaction, PdfDocument};
        let bytes = page_under_a_tree(RESOURCES, BOX);
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let (document, _) = super::super::Qpdf
            .open_for_redaction(&bytes, &options)
            .expect("opens");
        let page = document.page(0).expect("page 0");
        let failing = crate::redact::frame::check_inherited_media_box(&page, || {
            Err(burrow_types::Error::Malformed(
                "a renderer that could not open it".to_owned(),
            ))
        });
        assert!(
            matches!(&failing, Err(burrow_types::Error::Unsupported(m)) if m.contains("[media-box-unverified]")),
            "a renderer failure must refuse by this rule: {failing:?}"
        );
        let limited = crate::redact::frame::check_inherited_media_box(&page, || {
            Err(burrow_types::Error::LimitExceeded {
                limit: "max_duration_ms",
                stage: burrow_types::Stage::Deadline,
                requested: 2,
                allowed: 1,
            })
        });
        assert!(
            matches!(limited, Err(burrow_types::Error::LimitExceeded { .. })),
            "a limit the renderer hits is the operation's limit, not a verdict: {limited:?}"
        );
    }

    #[test]
    fn a_repair_after_the_open_is_refused_after_the_write() {
        // #224, security review round 3: an object reached only from the catalog is not read by
        // the walk, so qpdf repaired it later, after the open's check. The warnings are asked
        // again after the write and the bytes are dropped.
        //
        // #227 MOVED WHERE THE REPAIR HAPPENS, and this is the witness for that check now. The
        // reference check resolves every object the file refers to, before the walk; a stray `)`
        // is repaired there -- `[1 2 null 3]`, with a warning -- and the object is not null, so
        // that check passes it and only the warnings after the write can refuse it.
        let with_object_six = |six: &str| {
            pdf(&[
                "<< /Type /Catalog /Pages 2 0 R /X 6 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R {BOX} {RESOURCES} >>"),
                stream("BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n"),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                    .to_owned(),
                six.to_owned(),
            ])
        };
        refused_by(
            redact_in(&with_object_six("<< /A [1 2 ) 3] >>"), UPPER_BAND),
            "engine-repaired-input",
            "an object qpdf repaired outside the walk",
        );
        // THE ROUND-3 FIXTURE: a stream whose `/Length` is wrong. With recovery off qpdf cannot
        // read it, warns, and hands back a null where the file declares a stream -- so the
        // reference check refuses it first, as the disagreement it is (#227). The warning would
        // refuse it after the write too; the stray `)` above is what pins that check alone.
        refused_by(
            redact_in(
                &with_object_six("<< /Length 3 >>\nstream\nsomething longer than three\nendstream"),
                UPPER_BAND,
            ),
            "reference-to-nothing",
            "a stream qpdf cannot read, referred to from the catalog",
        );
    }

    #[test]
    fn an_annotation_rect_past_what_readers_agree_on_is_refused() {
        // #224, security reviews rounds 3 and 4: PDFium reads a whole number outside 32 bits as
        // 0, so a `/Rect` of `[20 4294967296 120 380]` is 0..380 to a viewer that draws
        // annotations -- over the secret at 350 -- and 380 upwards here, clear of the band: the
        // annotation was kept, `Ok` (measured with the bound removed, round 4). Refused as out of
        // range, not as a malformed `/Rect`: the kind and the message say which.
        let page = |rect: &str| {
            pdf(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /Contents 4 0 R {BOX} {RESOURCES} \
                     /Annots [<< /Type /Annot /Subtype /Square /Rect {rect} >>] >>"
                ),
                stream("BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n"),
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                    .to_owned(),
            ])
        };
        match redact_in(&page("[20 4294967296 120 380]"), UPPER_BAND) {
            Err(burrow_types::Error::Unsupported(message)) => assert!(
                message.contains("[annotation-rect]")
                    && message.contains("larger than any reader agrees on"),
                "a /Rect edge PDFium reads as 0: refused, but not as out of range: {message}"
            ),
            other => panic!("a /Rect edge PDFium reads as 0, over the secret: {other:?}"),
        }
        // A CONTAINER AMONG THE ITEMS (round 4): four numbers to a scan of the text -- 200 0 400
        // 120, clear of the band -- and `[0 400 120 0]` to PDFium, which reads an item that is
        // not a number as 0: over the secret. Measured `Ok` with the appearance drawn there.
        // AND EXACTLY FOUR (round 5): PDFium reads any other length as `[0 0 0 0]`, and an
        // appearance with no `/BBox` (#229) then draws from the page's origin -- measured over
        // the secret with the length check loosened to `< 4`. Only the five-item case pins the
        // length: the three-item one is also refused through its missing fourth item.
        for nested in [
            "[[200 0] 400 120 []]",
            "[<< /A 200 /B 0 >> 400 120 << >>]",
            "[20 380 220 400 7]",
            "[20 380 220]",
        ] {
            refused_by(
                redact_in(&page(nested), UPPER_BAND),
                "annotation-rect",
                &format!("a /Rect of {nested}"),
            );
        }
        // THESE TWO, BELOW, are refused by the out-of-range bound without being leaks themselves.
        refused_by(
            redact_in(&page("[4294967316 330 120 350]"), UPPER_BAND),
            "annotation-rect",
            "a /Rect corner past 32 bits",
        );
        refused_by(
            redact_in(&page("[20 330 120 -16777217]"), UPPER_BAND),
            "annotation-rect",
            "a /Rect corner below -2^24",
        );
        // THE NEAR-MISS: the same annotation where both readers put it is removed, and redacts.
        let output = redact_in(&page("[20 330 120 350]"), UPPER_BAND).expect("redacts");
        assert!(
            !String::from_utf8_lossy(&output).contains("/Square"),
            "the annotation over the region was removed"
        );
        // AND THE SEARCH CAN SEE IT: a region away from it keeps the annotation, readable.
        let away = Region {
            left: 0.0,
            top: 300.0,
            width: 300.0,
            height: 40.0,
        };
        let kept = redact_in(&page("[20 330 120 350]"), away).expect("redacts");
        assert!(
            String::from_utf8_lossy(&kept).contains("/Square"),
            "an annotation away from the region is kept, where this search would find it"
        );
    }

    #[test]
    fn a_repair_during_the_walk_is_refused() {
        // #224, security review round 2: qpdf reads a font lazily, so a stray `)` in it is
        // repaired -- and warned about -- during the walk, after the open's check; the check
        // after the write sees the warning, which persists.
        //
        // The stray `)` sits in a key NO reader interprets (#125's third security review): in
        // `/Widths`, where #224 put it, the strict number reader now refuses first, and the
        // warning check would no longer be what this pins.
        let font = |widths: &str, unread: &str| {
            pdf(&[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
                format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R {BOX} {RESOURCES} >>"),
                stream("BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n"),
                format!(
                    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding \
                     /WinAnsiEncoding /FirstChar 31 /LastChar 94 /Widths [{widths}] \
                     /BurrowUnread [{unread}] >>"
                ),
            ])
        };
        let widths = format!("0 9000 {}", "556 ".repeat(62));
        refused_by(
            redact_in(&font(&widths, "0 ) 1"), UPPER_BAND),
            "engine-repaired-input",
            "a font qpdf repaired while the walk read it",
        );
        // THE NEAR-MISS: the same font with nothing to repair redacts.
        redact_in(&font(&widths, "0 1"), UPPER_BAND).expect("nothing repaired, so it redacts");
        // #224'S OWN SHAPE still refuses, now by the reader that meets it first.
        refused_by(
            redact_in(
                &font(&format!("0 ) 9000 {}", "556 ".repeat(62)), "0 1"),
                UPPER_BAND,
            ),
            "number-unreadable",
            "a stray `)` inside /Widths",
        );
    }

    #[test]
    fn the_renderer_is_asked_about_the_page_being_redacted() {
        // #224, round 2: page 0 declares 300 x 400; page 1 sets its /MediaBox to null over the
        // same box on /Pages, so PDFium shows page 1 at US Letter. A check that asked about page
        // 0 would hear 300 x 400 and pass.
        let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\n";
        let resources = "/Resources << /Font << /F1 6 0 R >> >>";
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] /MediaBox [0 0 300 400] >>".to_owned(),
            format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R {BOX} {resources} >>"),
            format!("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /MediaBox null {resources} >>"),
            stream(content),
            helvetica("/Encoding /WinAnsiEncoding"),
        ]);
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let outcome = super::super::Qpdf
            .redact_page(&bytes, 1, &BTreeSet::from([1]), UPPER_BAND, &options)
            .map(|(output, _)| output);
        refused_by(
            outcome,
            "media-box-unverified",
            "page 1's null, asked about page 1",
        );
    }

    /// #152's measured shape: the page's `/F1` is zero wide, and `/GS0` names a `/Font`.
    /// `state` is the ExtGState's body; the secret sits in the band `redact_bytes` clears.
    fn behind_a_graphics_state(state: &str) -> Vec<u8> {
        let zero = format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths [{}] >>",
            "0 ".repeat(63)
        );
        two_pages(
            "<< /Font << /F1 6 0 R >> /ExtGState << /GS0 7 0 R >> >>",
            "BT /F1 24 Tf /GS0 gs 72 700 Td (AAASECRET) Tj ET\n",
            "<< >>",
            &[zero, state.to_owned()],
        )
    }

    #[test]
    fn a_graphics_state_that_sets_the_font_is_refused_end_to_end() {
        // #152, measured by the #218 review: PDFium drew SECRET across the region and the walk put
        // every glyph at x = 72, so the redaction returned `Ok` with it intact. The control is a
        // graphics state for transparency, which every real document with a shadow carries.
        match redact_bytes(
            &behind_a_graphics_state("<< /Type /ExtGState /Font [6 0 R 24] >>"),
            &[0],
        ) {
            Err(error) => assert!(
                format!("{error:?}").contains("[ext-gstate-sets-font]"),
                "refused, but not by [ext-gstate-sets-font]: {error:?}"
            ),
            Ok(()) => panic!("a font-setting graphics state was redacted"),
        }
        redact_bytes(
            &behind_a_graphics_state("<< /Type /ExtGState /CA 0.5 /ca 0.5 >>"),
            &[0],
        )
        .expect("a graphics state for transparency must redact");
    }

    /// The secret drawn inside form `/X`, whose own resources hold only the zero-width `/F1`;
    /// `/GS0`, with body `state`, lives in the PAGE's `/ExtGState` (#152, code review).
    fn graphics_state_on_the_page_used_in_a_form(state: &str) -> Vec<u8> {
        let zero = format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths [{}] >>",
            "0 ".repeat(63)
        );
        let body = "BT /F1 24 Tf /GS0 gs 72 700 Td (AAASECRET) Tj ET\n";
        two_pages(
            "<< /XObject << /X 8 0 R >> /ExtGState << /GS0 7 0 R >> >>",
            "q /X Do Q\n",
            "<< >>",
            &[
                zero,
                state.to_owned(),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font \
                     << /F1 6 0 R >> >> /Length {} >>\nstream\n{body}endstream",
                    body.len()
                ),
            ],
        )
    }

    #[test]
    fn a_form_without_graphics_states_is_held_to_the_pages() {
        // PDFium resolves the form's `/GS0` in the page's `/ExtGState`, since the form has none.
        // The first version asked only the form's own scope and returned `Ok` with the secret
        // inked across the region (#152's code review, measured).
        match redact_bytes(
            &graphics_state_on_the_page_used_in_a_form("<< /Type /ExtGState /Font [6 0 R 24] >>"),
            &[0],
        ) {
            Err(error) => assert!(
                format!("{error:?}").contains("[ext-gstate-sets-font]"),
                "refused, but not by [ext-gstate-sets-font]: {error:?}"
            ),
            Ok(()) => panic!("a form's gs resolved in the page's graphics states was redacted"),
        }
        redact_bytes(
            &graphics_state_on_the_page_used_in_a_form("<< /Type /ExtGState /CA 0.5 >>"),
            &[0],
        )
        .expect("the same form under a transparency state must redact");
    }

    #[test]
    fn a_graphics_state_category_written_as_a_stream_is_refused() {
        // PDFium reads a stream-valued `/ExtGState`'s own dictionary as the category; qpdf's
        // lookup on a stream finds nothing, so the font-setting `/GS0` was walked past -- `Ok`
        // with the secret intact (#152's security review). The #166 type gate now covers it.
        let body = "";
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> /ExtGState 7 0 R >>",
            "BT /F1 24 Tf /GS0 gs 72 700 Td (AAASECRET) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                format!(
                    "<< /GS0 << /Type /ExtGState /Font [6 0 R 24] >> /Length {} >>\nstream\n\
                     {body}endstream",
                    body.len()
                ),
            ],
        );
        match redact_bytes(&bytes, &[0]) {
            Err(error) => assert!(
                format!("{error:?}").contains("[not-a-dictionary-where-one-belongs]"),
                "refused, but not by the type gate: {error:?}"
            ),
            Ok(()) => panic!("a stream-valued /ExtGState was redacted"),
        }
    }

    #[test]
    fn a_form_whose_own_graphics_state_sets_the_font_is_refused() {
        // THE CHAIN'S OTHER HALF (#152 round 2): the form's OWN `/ExtGState` sets the font, and
        // the page has none. A chain that asked only the enclosing scopes survived the suite.
        let zero = format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths [{}] >>",
            "0 ".repeat(63)
        );
        let body = "BT /F1 24 Tf /GS0 gs 72 700 Td (AAASECRET) Tj ET\n";
        let bytes = two_pages(
            "<< /XObject << /X 8 0 R >> >>",
            "q /X Do Q\n",
            "<< >>",
            &[
                zero,
                "<< /Type /ExtGState /Font [6 0 R 24] >>".to_owned(),
                format!(
                    "<< /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font \
                     << /F1 6 0 R >> /ExtGState << /GS0 7 0 R >> >> /Length {} >>\nstream\n\
                     {body}endstream",
                    body.len()
                ),
            ],
        );
        match redact_bytes(&bytes, &[0]) {
            Err(error) => assert!(
                format!("{error:?}").contains("[ext-gstate-sets-font]"),
                "refused, but not by [ext-gstate-sets-font]: {error:?}"
            ),
            Ok(()) => panic!("a form's own font-setting graphics state was redacted"),
        }
    }

    #[test]
    fn a_token_readers_cut_differently_is_refused_end_to_end() {
        // #152 round 2: PDFium keeps a token's first 255 raw bytes and decodes escapes after.
        // An escape-inflated name reaches a font-setting state PDFium sees and this walk did not;
        // a padded number moves text into the region PDFium draws it in. Each was `Ok` with the
        // secret visible.
        let long = format!("{}{}", "G".repeat(200), "#47".repeat(20));
        let state_name = "G".repeat(218);
        let escaped = two_pages(
            &format!("<< /Font << /F1 6 0 R >> /ExtGState << /{state_name} 7 0 R >> >>"),
            &format!("BT /F1 24 Tf /{long} gs 72 700 Td (AAASECRET) Tj ET\n"),
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                "<< /Type /ExtGState /Font [6 0 R 24] >>".to_owned(),
            ],
        );
        let padded = two_pages(
            "<< /Font << /F1 6 0 R >> >>",
            &format!(
                "BT /F1 24 Tf 60 700 Td 0 -{}700 Td (AAASECRET) Tj ET\n",
                "0".repeat(300)
            ),
            "<< >>",
            &[helvetica("/Encoding /WinAnsiEncoding")],
        );
        // The NAME case is now refused one layer earlier, by the lexer's raw-token cut
        // (`[name-token-too-long]`, #257), before the operand-length check here is reached -- the
        // name/operand cut is one boundary (`lexer::MAX_TOKEN_BYTES`). The padded NUMBER is not a
        // name, so the lexer does not touch it and the operand check still refuses it.
        for (what, bytes, expected) in [
            (
                "an escape-inflated gs name",
                escaped,
                "[name-token-too-long]",
            ),
            ("a padded number", padded, "token longer than readers agree"),
        ] {
            match redact_bytes(&bytes, &[0]) {
                Err(error) => assert!(
                    format!("{error:?}").contains(expected),
                    "{what}: refused, but not for the token's length: {error:?}"
                ),
                Ok(()) => panic!("{what} was redacted"),
            }
        }
    }

    #[test]
    fn a_string_readers_cut_differently_is_refused_end_to_end() {
        // #152 round 3: PDFium keeps a string's first 32767 decoded bytes. A 40,000-byte run
        // at a tiny size, then `SECRET` risen onto the region: PDFium placed it inside the
        // region, this walk after 40,000 advances, outside it -- `Ok` over the secret. In `Tj`
        // and in `TJ`.
        let run = "A".repeat(40_000);
        for content in [
            format!("BT /F1 0.02 Tf 100 400 Td ({run}) Tj /F1 24 Tf 300 Ts (SECRET) Tj ET\n"),
            format!("BT /F1 0.02 Tf 100 400 Td [({run})] TJ /F1 24 Tf 300 Ts (SECRET) Tj ET\n"),
        ] {
            let bytes = two_pages(
                "<< /Font << /F1 6 0 R >> >>",
                &content,
                "<< >>",
                &[helvetica("/Encoding /WinAnsiEncoding")],
            );
            match redact_bytes(&bytes, &[0]) {
                Err(error) => assert!(
                    format!("{error:?}").contains("string longer than readers agree"),
                    "refused, but not for the string's length: {error:?}"
                ),
                Ok(()) => panic!("a string PDFium cuts short was redacted"),
            }
        }
    }

    #[test]
    fn a_gs_whose_operand_is_a_string_is_refused() {
        // PDFium resolves `(GS0) gs` as `/GS0 gs`; walking past it was an `Ok` over the secret
        // (#152's code review). Refused whatever the state holds.
        let bytes = two_pages(
            "<< /Font << /F1 6 0 R >> /ExtGState << /GS0 7 0 R >> >>",
            "BT /F1 24 Tf (GS0) gs 72 700 Td (AAASECRET) Tj ET\n",
            "<< >>",
            &[
                helvetica("/Encoding /WinAnsiEncoding"),
                "<< /Type /ExtGState /CA 0.5 >>".to_owned(),
            ],
        );
        match redact_bytes(&bytes, &[0]) {
            Err(error) => assert!(
                format!("{error:?}").contains("[gs-operand-not-a-name]"),
                "refused, but not by [gs-operand-not-a-name]: {error:?}"
            ),
            Ok(()) => panic!("a gs with a string operand was redacted"),
        }
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

// #206: the second reading, wired into the operation that ships natively. The rules themselves are
// held by a lying fake in `redact_read_back::tests`; these hold that `redact_page` asks PDFium at
// all, that a disagreement it reports reaches the caller as a refusal, and that an honest document
// still redacts.
#[cfg(all(
    test,
    feature = "native-engines",
    burrow_native_engines,
    target_os = "linux"
))]
mod second_reading {
    use super::super::Qpdf;
    use crate::PageRedactor;
    use crate::pdfsyntax::region::Region;
    use crate::redact::hooks::{
        READER_DROPS_A_PAGE, SECOND_READING_FAILS, SECOND_READING_PAGE, SECOND_READINGS,
        WIDTHS_MISREAD, WIDTHS_MISREAD_COUNT, WRITER_DROPS_A_PAGE,
    };
    use burrow_types::{Error, Limits, SystemClock};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    /// A two-page document. Page 0 draws `content`; page 1 is blank. Plain, visible text in a
    /// standard font: what each test plants is in the walk or the engine, never in the file.
    fn document(content: &str) -> Vec<u8> {
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
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

    /// Two glyphs at 100 pt from x = 10. The first is outside the region on any reading; the
    /// second's origin is at 10 + 55.6 = 65.6, inside it, carried there by the first's width.
    const CARRIED_BY_A_WIDTH: &str = "BT /F1 100 Tf 10 700 Td (KK) Tj ET\n";

    /// Both glyphs where the content stream says, one inside the region and one outside.
    const PLAIN: &str = "BT /F1 24 Tf 72 700 Td (K) Tj ET\nBT /F1 24 Tf 72 100 Td (S) Tj ET\n";

    /// The region `[40, 540] x [632, 752]` in content space.
    fn redact(content: &str) -> burrow_types::Result<Vec<u8>> {
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let covered: BTreeSet<usize> = [0].into_iter().collect();
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        Qpdf.redact_page(&document(content), 0, &covered, region, &options)
            .map(|(bytes, _)| bytes)
    }

    /// Every hook off, and the counters zeroed, so one test's plant cannot reach the next.
    fn reset() {
        WIDTHS_MISREAD.with(|flag| flag.set(false));
        WIDTHS_MISREAD_COUNT.with(|n| n.set(0));
        SECOND_READING_FAILS.with(|flag| flag.set(false));
        SECOND_READING_PAGE.with(|page| page.set(None));
        SECOND_READINGS.with(|n| n.set(0));
        READER_DROPS_A_PAGE.with(|flag| flag.set(false));
        WRITER_DROPS_A_PAGE.with(|flag| flag.set(false));
    }

    fn readings() -> usize {
        SECOND_READINGS.with(std::cell::Cell::get)
    }

    /// The bracketed rule a rejection names.
    fn rule_of(result: burrow_types::Result<Vec<u8>>) -> String {
        match result {
            Ok(_) => "Ok".to_owned(),
            Err(Error::OutputRejected(message)) => message
                .rsplit('[')
                .next()
                .and_then(|rest| rest.split(']').next())
                .map_or_else(|| format!("no rule in {message:?}"), str::to_owned),
            Err(other) => format!("not a rejection: {other}"),
        }
    }

    #[test]
    fn an_honest_document_redacts_and_the_renderer_was_asked() {
        reset();
        let result = redact(PLAIN);
        let asked = readings();
        reset();
        assert_eq!(rule_of(result), "Ok");
        assert_eq!(
            asked, 1,
            "the second reading must run on every native redaction"
        );
    }

    #[test]
    fn r1_a_placement_defect_burrow_makes_twice_is_refused_by_the_renderer() {
        // THE CASE THE SECOND READING EXISTS FOR. The font reader misreads `/Widths`, in both of
        // burrow's passes, as a real placement defect would: the operation places the second
        // glyph at x = 15.6, outside the region, removes nothing, and burrow's own verification
        // agrees. Only PDFium puts it inside.
        //
        // TWIN FIRST: the same document with the reader correct redacts, so the refusal below is
        // the defect's and not the document's.
        reset();
        let twin = redact(CARRIED_BY_A_WIDTH);
        reset();
        assert_eq!(rule_of(twin), "Ok", "the twin, with the reader correct");

        WIDTHS_MISREAD.with(|flag| flag.set(true));
        let planted = redact(CARRIED_BY_A_WIDTH);
        let misread = WIDTHS_MISREAD_COUNT.with(std::cell::Cell::get);
        let asked = readings();
        reset();
        assert!(
            misread > 0,
            "the planted defect did not apply: no font's widths were read"
        );
        assert_eq!(asked, 1, "the second reading ran");
        assert_eq!(rule_of(planted), "second-reading-glyph-in-region");
    }

    /// As [`document`], with `fonts` added to the page's `/Font` (beside `/F1`), `xobjects` as its
    /// `/XObject`, and `extra` as objects 7 onwards. Every fixture below draws plain visible
    /// text; what varies is how its marked content is nested, and where.
    fn document_with(content: &str, fonts: &str, xobjects: &str, extra: &[String]) -> Vec<u8> {
        let widths = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [3 0 R 6 0 R] >>".to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 5 0 R {fonts}>> /XObject << {xobjects}>> >> \
                 /Contents 4 0 R >>"
            ),
            stream("", content),
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding \
                 /WinAnsiEncoding /FirstChar 32 /LastChar 94 /Widths {widths} >>"
            ),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_owned(),
        ];
        objects.extend(extra.iter().cloned());
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
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

    /// A stream object holding `data`, with `dictionary`'s entries before its `/Length`.
    fn stream(dictionary: &str, data: &str) -> String {
        format!(
            "<< {dictionary} /Length {} >>\nstream\n{data}endstream",
            data.len()
        )
    }

    /// As [`redact`], over [`document_with`]; and how many times PDFium loaded the page.
    fn redact_with(
        content: &str,
        fonts: &str,
        xobjects: &str,
        extra: &[String],
    ) -> (String, usize) {
        reset();
        let options = crate::OpenOptions::new(Limits::default(), Arc::new(SystemClock::new()));
        let covered: BTreeSet<usize> = [0].into_iter().collect();
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        let pdf = document_with(content, fonts, xobjects, extra);
        let result = Qpdf
            .redact_page(&pdf, 0, &covered, region, &options)
            .map(|(bytes, _)| bytes);
        let read = readings();
        reset();
        (rule_of(result), read)
    }

    /// `depth` nested `BMC` sequences around `inner`.
    fn wrapped_in(depth: usize, inner: &str) -> String {
        format!(
            "{}{inner}{}",
            "/Span BMC\n".repeat(depth),
            "EMC\n".repeat(depth)
        )
    }

    /// Visible text outside the region, as a form draws it.
    const FORM_TEXT: &str = "BT /F1 12 Tf 300 300 Td (K) Tj ET\n";

    /// A Form XObject over the whole page drawing `data`, as object 7, which the page names `/Fm0`.
    const FORM_RESOURCES: &str = "/Fm0 7 0 R ";
    fn form(data: &str) -> Vec<String> {
        vec![stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792]",
            data,
        )]
    }

    /// The outcome of redacting `content` alone, and whether PDFium loaded the page.
    fn verdict(content: &str) -> (String, usize) {
        redact_with(content, "", "", &[])
    }

    #[test]
    fn a_page_nested_past_the_ceiling_is_refused_before_pdfium_loads_it() {
        // THE TWIN: the plain page nested 300 deep, about 90,000 of marked-content work against
        // the ceiling's 1,000,000, is read and redacted.
        assert_eq!(
            verdict(&wrapped_in(300, PLAIN)),
            ("Ok".to_owned(), 1),
            "nested 300 deep, under the ceiling"
        );
        // 1,500 deep is about 2,250,000, past it.
        assert_eq!(
            verdict(&wrapped_in(1_500, PLAIN)),
            ("second-reading-nesting".to_owned(), 0),
            "the page past the ceiling must never reach PDFium's page load"
        );
    }

    #[test]
    fn a_form_cannot_close_the_marks_open_where_it_is_drawn() {
        // THE PER-STREAM FLOOR. The page opens 500 marks and draws a form whose first act is to
        // close 500. PDFium keeps a mark stack per stream, so the form cannot close the page's,
        // and every operation after its `EMC`s is still read 500 deep. A count the form could
        // close read them at depth zero.
        let page = format!("{PLAIN}{}", wrapped_in(500, "/Fm0 Do\n"));
        let closing = |shows: usize| {
            form(&format!(
                "{}{}",
                "EMC\n".repeat(500),
                FORM_TEXT.repeat(shows)
            ))
        };
        // THE TWIN: 100 shows after the `EMC`s, 500 operations at depth 500 -- about 750,000 in
        // all -- is read and redacted.
        assert_eq!(
            redact_with(&page, "", FORM_RESOURCES, &closing(100)),
            ("Ok".to_owned(), 1),
            "100 shows"
        );
        // 600 shows is 3,000 operations at depth 500 -- about 2,000,000, past the ceiling. With
        // the form able to close the page's marks, the same page counts about 375,000.
        assert_eq!(
            redact_with(&page, "", FORM_RESOURCES, &closing(600)),
            ("second-reading-nesting".to_owned(), 0),
            "600 shows after the form's EMCs must be counted at the page's depth"
        );
    }

    #[test]
    fn every_draw_of_a_form_is_counted() {
        // TOTAL WORK ACROSS DRAWS. PDFium parses a form again at every `Do`, so the work of a
        // form nested 300 deep -- about 91,500 -- is paid once per draw. A count of the form's
        // first draw only, or of its deepest point, sees one draw however many there are.
        let nested = form(&wrapped_in(300, FORM_TEXT));
        let page = |draws: usize| format!("{PLAIN}{}", "/Fm0 Do\n".repeat(draws));
        assert_eq!(
            redact_with(&page(5), "", FORM_RESOURCES, &nested),
            ("Ok".to_owned(), 1),
            "5 draws, about 457,500"
        );
        assert_eq!(
            redact_with(&page(20), "", FORM_RESOURCES, &nested),
            ("second-reading-nesting".to_owned(), 0),
            "20 draws, about 1,830,000, must be counted as 20"
        );
    }

    /// A Type 3 font as object `at`, whose codes `first..first + codes` are the names `/g0..`,
    /// each glyph 1000 units wide, with `/CharProcs` the given dictionary (direct or a reference).
    fn type_three_font(first: usize, codes: usize, char_procs: &str) -> String {
        let names: String = (0..codes).map(|i| format!("/g{i} ")).collect();
        let widths = "1000 ".repeat(codes);
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs {char_procs} \
             /Encoding << /Type /Encoding /Differences [{first} {names}] >> \
             /FirstChar {first} /LastChar {} /Widths [{widths}] >>",
            first + codes - 1
        )
    }

    /// Visible Type 3 text outside the region: font `/T{font}` at 1 pt, showing `codes` codes from 1.
    fn shows(font: usize, codes: usize) -> String {
        let hex: String = (1..=codes).map(|code| format!("{code:02X}")).collect();
        format!("BT /T{font} 1 Tf 300 {} Td <{hex}> Tj ET\n", 300 + font)
    }

    #[test]
    fn type_three_procedures_are_counted_for_every_code_drawn() {
        // TYPE 3 PROCEDURES. PDFium parses a Type 3 glyph's procedure when it loads the page, and
        // the walk never enters one. One font with `codes` codes, each its own procedure nested
        // 150 deep -- about 22,500 apiece -- every code drawn, outside the region, beside the
        // plain page's text.
        let run = |codes: usize| {
            let procs: String = (0..codes)
                .map(|i| format!("/g{i} {} 0 R ", 8 + i))
                .collect();
            let mut objects = vec![type_three_font(1, codes, &format!("<< {procs}>>"))];
            for _ in 0..codes {
                objects.push(stream("", &format!("1000 0 d0\n{}", wrapped_in(150, ""))));
            }
            redact_with(
                &format!("{PLAIN}{}", shows(0, codes)),
                "/T0 7 0 R ",
                "",
                &objects,
            )
        };
        assert_eq!(run(10), ("Ok".to_owned(), 1), "10 codes, about 225,000");
        assert_eq!(
            run(60),
            ("second-reading-nesting".to_owned(), 0),
            "60 codes, about 1,350,000, must each be counted"
        );
    }

    #[test]
    fn a_procedure_shared_by_codes_and_fonts_is_counted_for_each_of_them() {
        // ONE PROCEDURE, MANY PARSES. PDFium keeps a Type 3 font's glyphs per font object and code,
        // so a procedure named under 255 codes, in each of 8 font objects sharing one `/CharProcs`
        // dictionary, is parsed 2,040 times at the load. Counted once per stream it read as one
        // procedure -- under the ceiling -- and the uninterruptible load ran for seconds (#206's
        // second review). The procedure is nested 600 deep, about 360,000: one parse is under the
        // ceiling, 2,040 are far past it.
        let procedure = stream("", &format!("1 0 d0\n{}", wrapped_in(600, "")));
        let run = |fonts: usize, codes: usize| {
            // Object 7 the shared `/CharProcs`, 8 the procedure, 9.. the fonts.
            let names: String = (0..codes).map(|i| format!("/g{i} 8 0 R ")).collect();
            let mut objects = vec![format!("<< {names}>>"), procedure.clone()];
            let mut resources = String::new();
            let mut page = PLAIN.to_owned();
            for font in 0..fonts {
                objects.push(type_three_font(1, codes, "7 0 R"));
                resources.push_str(&format!("/T{font} {} 0 R ", 9 + font));
                page.push_str(&shows(font, codes));
            }
            let started = std::time::Instant::now();
            let outcome = redact_with(&page, &resources, "", &objects);
            (outcome, started.elapsed())
        };
        // THE TWIN: one font, one code, one parse -- read and redacted.
        let (twin, _) = run(1, 1);
        assert_eq!(twin, ("Ok".to_owned(), 1), "one font drawing one code");
        // PAST IT: refused before PDFium loads the page. `read == 0` is the check; the time is
        // reported, not asserted. Run alone it is milliseconds (39 ms in a debug build on
        // 2026-10-15, against about 4 s for the load it prevents). Inside the parallel suite the
        // wall clock also holds the wait for PDFium's one engine thread, which every test
        // shares and the operation's page-size read still uses: 1.8 s was measured there, none of
        // it this refusal's.
        let (past, took) = run(8, 255);
        assert_eq!(
            past,
            ("second-reading-nesting".to_owned(), 0),
            "8 fonts x 255 codes of one procedure must be counted 2,040 times"
        );
        println!("refused in {took:?}");
    }

    #[test]
    fn the_second_reading_spends_the_operations_budget() {
        // THE CLOCK PASSES THE BUDGET AS THE SECOND READING BEGINS, and not before: the redaction
        // and burrow's own verification run inside it. The reading must spend the OPERATION's
        // deadline, so the outcome is the operation's -- `LimitExceeded` on `max_duration_ms`,
        // never a verdict on the page -- and it must come at the reading's first open, before any
        // page is asked for. A reading that started a fresh deadline of its own would open the
        // input, count it, and go on to read the page.
        use crate::redact::hooks::ON_SECOND_READING;
        reset();
        let clock = Arc::new(burrow_types::ManualClock::new(0));
        let limits = Limits::with(|l| l.max_duration_ms = 60_000);
        let options =
            crate::OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn burrow_types::Clock>);
        let advanced = Arc::clone(&clock);
        ON_SECOND_READING.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || advanced.advance(61_000)));
        });
        let covered: BTreeSet<usize> = [0].into_iter().collect();
        let region = Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        };
        let outcome = Qpdf
            .redact_page(&document(PLAIN), 0, &covered, region, &options)
            .map(|(bytes, _)| bytes);
        ON_SECOND_READING.with(|slot| *slot.borrow_mut() = None);
        let read = readings();
        let now = burrow_types::Clock::now_ms(clock.as_ref());
        reset();
        assert_eq!(
            now, 61_000,
            "the clock was advanced by the second reading, and only there"
        );
        match outcome {
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            }) => {}
            other => panic!(
                "past the budget during the second reading must be the operation's time limit: \
                 {:?}",
                other.map(|_| ())
            ),
        }
        assert_eq!(
            read, 0,
            "the page was asked for after the budget had run out"
        );
    }

    #[test]
    fn r4_an_output_short_of_a_page_is_refused() {
        reset();
        WRITER_DROPS_A_PAGE.with(|flag| flag.set(true));
        let result = redact(PLAIN);
        let applied = !WRITER_DROPS_A_PAGE.with(std::cell::Cell::get);
        reset();
        assert!(applied, "the writer never dropped the page");
        assert_eq!(rule_of(result), "second-reading-page-count");
    }

    #[test]
    fn r5_an_input_qpdf_reads_short_of_a_page_is_refused() {
        // #111: qpdf loses a page as it reads, so the promise and qpdf's own reading of the output
        // agree with each other. Only another engine's count of the input can refuse.
        reset();
        READER_DROPS_A_PAGE.with(|flag| flag.set(true));
        let result = redact(PLAIN);
        let applied = !READER_DROPS_A_PAGE.with(std::cell::Cell::get);
        reset();
        assert!(applied, "the reader never dropped the page");
        assert_eq!(rule_of(result), "second-reading-input-page-count");
    }

    #[test]
    fn a_reading_of_another_page_is_refused() {
        reset();
        SECOND_READING_PAGE.with(|page| page.set(Some(1)));
        let result = redact(PLAIN);
        let asked = readings();
        reset();
        assert_eq!(asked, 1);
        assert_eq!(rule_of(result), "second-reading-wrong-page");
    }

    #[test]
    fn a_failing_second_reading_stops_the_operation_returning_bytes() {
        // KILLS: discarding `region_is_cleared_twice`'s result, or calling `region_is_cleared`
        // alone. The honest run above is this test's twin.
        reset();
        SECOND_READING_FAILS.with(|flag| flag.set(true));
        let result = redact(PLAIN);
        reset();
        assert_eq!(rule_of(result), "second-reading-unavailable");
    }
}
