//! The probe behind `support::char_box_oracle::canonical_unicode`.
//!
//! **Its own binary, deliberately.** `support` is included by every integration test here, so a
//! `#[cfg(test)] mod` inside the oracle compiled into seven of them and ran three PDFium
//! documents in each — which flaked under the parallel workspace run and measured nothing the
//! first copy had not. One binary asks the question once.
//!
//! A canonicalisation nobody can see the effect of is indistinguishable from one that never
//! fires, so this asserts the condition it exists for **on every run**: the positive shape (a
//! line-final hyphen with a line after it) must come back from PDFium as `0x0002` and be
//! canonicalised to `0x002D`, and the near-misses must come back untouched. If PDFium ever
//! stops re-labelling, the first assertion fails and `canonical_unicode` can be deleted rather
//! than left inert.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test building its own fixtures; the workspace lints are for attacker-controlled \
              input in library code, where they stay denied"
)]

mod support;

use support::char_box_oracle::chars_on_page;
use support::pdf_builder::{Builder, helvetica_with_widths};

const PDFIUM_LINE_FINAL_HYPHEN: u32 = 0x0002;

/// A one-page document drawing `first` at y=100 and, if given, `second` at y=40.
fn two_lines(first: &str, second: Option<&str>) -> Vec<u8> {
    let mut content = format!("BT /Helv 14 Tf 40 100 Td ({first}) Tj ET\n");
    if let Some(second) = second {
        content.push_str(&format!("BT /Helv 14 Tf 40 40 Td ({second}) Tj ET\n"));
    }
    let mut builder = Builder::new();
    let page = builder.reserve();
    let font = builder.add(&helvetica_with_widths());
    let stream = builder.stream("", &content);
    let pages = builder.reserve();
    builder.put(
        page,
        &format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 300 150] \
             /Contents {stream} 0 R /Resources << /Font << /Helv {font} 0 R >> >> >>"
        ),
    );
    builder.put(
        pages,
        &format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    let root = builder.add(&format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    builder.build(root)
}

/// What PDFium said, and what the oracle canonicalised it to, for the top line's last
/// character read from the file.
fn last_on_the_top_line(pdf: &[u8]) -> (u32, u32) {
    let chars = chars_on_page(pdf, 0);
    let last = chars
        .iter()
        .rfind(|char| !char.generated && char.origin.1 > 80.0)
        .expect("the top line draws at least one character");
    (last.raw_unicode, last.unicode)
}

#[test]
fn a_line_final_hyphen_is_relabelled_by_pdfium() {
    let (raw, canonical) = last_on_the_top_line(&two_lines("ABC-", Some("DEF")));
    assert_eq!(
        raw, PDFIUM_LINE_FINAL_HYPHEN,
        "PDFium no longer re-labels a line-final hyphen; canonical_unicode is now inert \
         and should be deleted rather than left in"
    );
    assert_eq!(
        canonical,
        u32::from('-'),
        "the canonicalisation must undo it"
    );
}

#[test]
fn a_hyphen_that_is_not_line_final_is_left_alone() {
    let (raw, canonical) = last_on_the_top_line(&two_lines("ABC-D", Some("DEF")));
    assert_eq!(raw, u32::from('D'), "the near-miss must not be re-labelled");
    assert_eq!(canonical, u32::from('D'));
}

#[test]
fn a_line_final_hyphen_with_nothing_after_it_is_left_alone() {
    let (raw, canonical) = last_on_the_top_line(&two_lines("ABC-", None));
    assert_eq!(
        raw,
        u32::from('-'),
        "the re-labelling is conditional on a following line, not on the hyphen"
    );
    assert_eq!(canonical, u32::from('-'));
}
