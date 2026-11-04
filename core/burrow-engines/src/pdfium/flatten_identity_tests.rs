//! ADR 0031 *Fixtures*, §5 `[page-identity-mismatch]` and "frames as floats", on the real engines:
//! qpdf's `page_frames` against PDFium's count and no-load sizes. Page *N*'s rotation is in
//! `flatten_render_tests.rs`, because it is read inside the render.
//!
//! Each test names the mutation that turns it red. Measurements of the pinned PDFium that the
//! comparison relies on are pinned here too, by name.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Limits, ManualClock};

use super::{check_pages, survey};
use crate::pdfium::{Pdfium, PdfiumDocument};
use crate::qpdf::Qpdf;
use crate::redact::flatten::identity::{Disagreement, Survey, is_page_identity_mismatch};
use crate::redact::frames::{PageFrameReading, PageFrames};
use crate::{DocumentEngine, OpenOptions, PageFrameReader, PageRenderer};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// A document of numbered objects, the catalogue first.
fn objects(bodies: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            bodies.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// One `/Pages` root carrying `root` (its `/Count` among it), over one page per entry of `pages`.
fn tree(root: &str, pages: &[&str]) -> Vec<u8> {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut bodies = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Kids [{}] {root} >>", kids.join(" ")),
    ];
    for page in pages {
        bodies.push(format!(
            "<< /Type /Page /Parent 2 0 R /Resources << >> {page} >>"
        ));
    }
    objects(&bodies)
}

/// `pages` under a root whose `/Count` is borne out.
fn honest(pages: &[&str]) -> Vec<u8> {
    tree(&format!("/Count {}", pages.len()), pages)
}

fn frames(bytes: &[u8]) -> PageFrames {
    Qpdf.page_frames(bytes, &options()).unwrap()
}

fn open(bytes: &[u8]) -> PdfiumDocument {
    Pdfium::new()
        .open(bytes.to_vec().into_boxed_slice(), &options())
        .unwrap()
}

/// Both engines over `bytes`: the survey, and whether `check_pages` refused it as a mismatch.
fn compare(bytes: &[u8]) -> (Survey, bool) {
    let read = frames(bytes);
    let document = open(bytes);
    let options = options();
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    let surveyed = survey(&read, &document, &options, &deadline).unwrap();
    let checked = check_pages(&read, &document, &options, &deadline);
    match &checked {
        Ok(agreed) => assert_eq!(agreed, &surveyed),
        Err(error) => assert!(is_page_identity_mismatch(error), "{error:?}"),
    }
    (surveyed, checked.is_err())
}

const PAGE: &str = "/MediaBox [0 0 200 100]";

/// MEASURED, AND RELIED ON: the no-load size is the display size, with width and height swapped
/// at `/Rotate` 90 and 270 and not at 180, an inherited `/Rotate` included. If this went red, the
/// comparison would have to swap PDFium's side too (ADR 0031 §5).
#[test]
fn the_no_load_size_reflects_rotate_on_the_pinned_pdfium() {
    let cases = [
        ("", (200.0, 100.0)),
        ("/Rotate 90", (100.0, 200.0)),
        ("/Rotate 180", (200.0, 100.0)),
        ("/Rotate 270", (100.0, 200.0)),
        ("/Rotate -90", (100.0, 200.0)),
    ];
    for (rotate, expected) in cases {
        let document = open(&honest(&[&format!("{PAGE} {rotate}")]));
        assert_eq!(
            PageRenderer::page_size(&Pdfium, &document, 0).unwrap(),
            expected,
            "{rotate}"
        );
    }
    let inherited = tree("/Count 1 /Rotate 90", &[PAGE]);
    assert_eq!(
        PageRenderer::page_size(&Pdfium, &open(&inherited), 0).unwrap(),
        (100.0, 200.0),
        "an inherited /Rotate"
    );
}

/// The twin every fixture below differs from, with the quarter turns that agree only through the
/// swap in qpdf's display frame.
///
/// KILLS: MI4, the swap removed from `PageFrame::displayed_size`.
#[test]
fn engines_reading_the_same_pages_agree_at_every_rotation() {
    let bytes = honest(&[
        PAGE,
        &format!("{PAGE} /Rotate 90"),
        &format!("{PAGE} /Rotate 180"),
        &format!("{PAGE} /Rotate 270"),
        "/MediaBox [0 0 612 792] /CropBox [10.5 20.25 500.75 700.125] /Rotate 90",
    ]);
    let (surveyed, refused) = compare(&bytes);
    assert!(!refused, "{surveyed:?}");
    assert_eq!(surveyed.examined, 5);
    assert!(surveyed.disagreements.is_empty());
}

/// ADR row: engines disagreeing on the count. PDFium counts the root's `/Count`, qpdf the leaves,
/// and qpdf repairs nothing (measured), so this is a real file.
///
/// KILLS: MI1, the count comparison deleted.
#[test]
fn a_root_count_the_leaves_do_not_bear_out_refuses_both_ways() {
    for (count, pages, pdfium) in [(2_u64, 1_usize, 2_u64), (1, 2, 1)] {
        let bytes = tree(&format!("/Count {count}"), &vec![PAGE; pages]);
        let (surveyed, refused) = compare(&bytes);
        assert!(refused, "/Count {count} over {pages}: {surveyed:?}");
        assert_eq!(
            surveyed.disagreements,
            vec![Disagreement::Count {
                qpdf: pages as u64,
                pdfium
            }]
        );
    }
    // THE TWIN: the same tree with its count borne out.
    assert!(!compare(&honest(&[PAGE, PAGE])).1);
}

/// MEASURED: an intermediate node's `/Count` lie is not trusted by the pinned PDFium, so both
/// engines enumerate the same leaves and the pass sees nothing. `[page-tree-ambiguous]` (S3, a
/// count the tree does not bear out) owns this shape; this test says S9b does not.
#[test]
fn an_intermediate_count_lie_is_not_seen_by_this_pass() {
    let bodies: Vec<String> = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 3 >>",
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 3 0 R /Resources << >> /MediaBox [0 0 100 100] >>",
        "<< /Type /Page /Parent 3 0 R /Resources << >> /MediaBox [0 0 200 100] >>",
        "<< /Type /Page /Parent 2 0 R /Resources << >> /MediaBox [0 0 300 100] >>",
    ]
    .map(str::to_owned)
    .to_vec();
    let (surveyed, refused) = compare(&objects(&bodies));
    assert!(!refused);
    assert_eq!(surveyed.examined, 3);
}

/// ADR row: one page's display size. Only page 0 differs, and it is not the page the integration
/// slice would render (page 1). PDFium subtracts in 32 bits after rounding each coordinate, qpdf
/// in 64 and is rounded once: one ulp apart on this box, although both read the same numbers
/// (ROUND0 finding 3; a fail-closed false refusal the census counts).
///
/// KILLS: MI2, only page N's size compared.
#[test]
fn one_other_page_s_size_one_ulp_apart_refuses() {
    let odd = "/MediaBox [0.1 0.2 612.123456789 792.987654321]";
    let bytes = honest(&[odd, PAGE]);
    let (surveyed, refused) = compare(&bytes);
    assert!(refused, "{surveyed:?}");
    let [Disagreement::Size { page, qpdf, pdfium }] = surveyed.disagreements[..] else {
        panic!("{surveyed:?}");
    };
    assert_eq!(page, 0);
    assert_eq!(qpdf.1, 792.787_654_321);
    assert_eq!(pdfium.1, 792.787_65_f32, "the height agrees");
    assert_ne!(f64::from(pdfium.0), qpdf.0);
    // THE TWIN: fractional coordinates whose two roundings land on the same 32-bit value.
    let agreeing = honest(&["/MediaBox [10.3 20.7 600.1 780.9]", PAGE]);
    assert!(!compare(&agreeing).1);
}

/// ADR row "frames as floats": 612.1 is read as two different numbers by the two engines, equal
/// only once qpdf's is rounded to 32 bits.
///
/// KILLS: MI3, the comparison made without rounding.
#[test]
fn a_width_whose_64_and_32_bit_readings_differ_agrees_on_real_engines() {
    let bytes = honest(&["/MediaBox [0 0 612.1 792]"]);
    let read = frames(&bytes);
    let PageFrameReading::Frame { width, .. } = read.readings[0] else {
        panic!("{read:?}");
    };
    let pdfium = PageRenderer::page_size(&Pdfium, &open(&bytes), 0).unwrap();
    assert_ne!(
        f64::from(pdfium.0),
        width,
        "the two readings differ in 64 bits"
    );
    assert!(!compare(&bytes).1);
}

/// ADR fixture: a page tree with `/Rotate` inherited vs on the page. On a page other than *N*,
/// qpdf refuses to frame an inherited `/Rotate` (`[page-attribute-inherited]`), so nothing vouches
/// for that page's identity and the pass fails closed. The twin carries the same `/Rotate` on the
/// page's own dictionary.
///
/// KILLS: MI7, `Unframed` readings skipped.
#[test]
fn an_inherited_rotate_on_another_page_refuses_and_the_same_rotate_on_the_page_does_not() {
    let bodies: Vec<String> = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 /Rotate 90 >>",
        "<< /Type /Page /Parent 2 0 R /Resources << >> /MediaBox [0 0 200 100] >>",
        "<< /Type /Page /Parent 3 0 R /Resources << >> /MediaBox [0 0 200 100] >>",
    ]
    .map(str::to_owned)
    .to_vec();
    let inherited = objects(&bodies);
    let (surveyed, refused) = compare(&inherited);
    assert!(refused);
    assert_eq!(
        surveyed.disagreements,
        vec![Disagreement::Unframed { page: 0 }]
    );

    let mut on_the_page = bodies;
    on_the_page[2] = "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R] /Count 1 >>".to_owned();
    on_the_page[4] =
        "<< /Type /Page /Parent 3 0 R /Resources << >> /MediaBox [0 0 200 100] /Rotate 90 >>"
            .to_owned();
    let (surveyed, refused) = compare(&objects(&on_the_page));
    assert!(!refused, "{surveyed:?}");
    assert_eq!(surveyed.examined, 2);
}

/// The pass spends the operation's deadline: a spent one refuses before any page is read.
#[test]
fn a_spent_deadline_refuses_the_pass() {
    let bytes = honest(&[PAGE, PAGE]);
    let read = frames(&bytes);
    let document = open(&bytes);
    let clock = Arc::new(ManualClock::new(0));
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = 10;
    let options = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    let deadline = Deadline::start(clock.as_ref(), &limits);
    clock.advance(11);
    let got = check_pages(&read, &document, &options, &deadline);
    assert!(
        matches!(got, Err(burrow_types::Error::LimitExceeded { .. })),
        "{got:?}"
    );
}
