//! An annotation the redaction keeps must draw inside its `/Rect`, on both engines (#229).
//!
//! The page is 300 x 400 with `SECRET` at (20, 350) in the band the region covers; the annotation's
//! `/Rect` is `[20 50 120 70]` on an upright page and `[200 100 300 120]` on a turned one, clear of
//! the band either way, so it is kept and the two new checks are what decide.

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::web_differential_tests::NativeBridge;
use crate::pdfsyntax::region::Region;
use crate::{OpenOptions, PageRedactor};

const UPPER_BAND: Region = Region {
    left: 0.0,
    top: 30.0,
    width: 300.0,
    height: 40.0,
};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// A file of `objects`, numbered from 1, with a classic cross-reference.
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

fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// One page carrying one annotation. `page_extra` and `pages_extra` go on the page and its
/// `/Pages` node; `annotation` is the annotation's own entries beside `/Rect` and `/AP`, and `ap`
/// its `/AP` dictionary, whose streams are objects 7 onwards (`streams`).
fn annotated(
    page_extra: &str,
    pages_extra: &str,
    rect: &str,
    annotation: &str,
    ap: &str,
    streams: &[String],
) -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\nBT /F1 12 Tf 20 20 Td (KEEP) Tj ET";
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Count 1 /Kids [3 0 R] {pages_extra} >>"),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] {page_extra} >>"
        ),
        stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
        // A STAMP unless the caller names a subtype: a second `/Subtype` would be a duplicate key,
        // which qpdf repairs with a warning, and the document would refuse for that instead.
        format!(
            "<< /Type /Annot {} /Rect {rect} {annotation} /AP {ap} >>",
            if annotation.contains("/Subtype") {
                ""
            } else {
                "/Subtype /Stamp"
            }
        ),
    ];
    objects.extend(streams.iter().cloned());
    pdf(&objects)
}

/// An appearance stream drawing a block, with `bbox` written into its dictionary as given.
fn appearance(bbox: &str) -> String {
    stream(
        &format!("/Type /XObject /Subtype /Form {bbox}"),
        "0 g 0 0 100 20 re f",
    )
}

const UPRIGHT: &str = "[20 50 120 70]";
const TURNED: &str = "[200 100 300 120]";

fn on_both(bytes: &[u8]) -> [(&'static str, burrow_types::Result<Vec<u8>>); 2] {
    let covered = BTreeSet::from([0]);
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    [
        (
            "natively",
            super::Qpdf
                .redact_page(bytes, 0, &covered, UPPER_BAND, &options())
                .map(|(output, _)| output),
        ),
        (
            "on the web",
            web.redact_page(bytes, 0, &covered, UPPER_BAND, &options())
                .map(|(output, _)| output),
        ),
    ]
}

fn refused_on_both(bytes: &[u8], rule: &str, what: &str) {
    for (engine, outcome) in on_both(bytes) {
        match outcome {
            Err(error) => assert!(
                format!("{error:?}").contains(&format!("[{rule}]")),
                "{what}, {engine}: refused, but not by [{rule}]: {error:?}"
            ),
            Ok(_) => panic!("{what}, {engine}: redacted, where [{rule}] must refuse"),
        }
    }
}

fn redacts_on_both(bytes: &[u8], what: &str) {
    let [(_, natively), (_, on_the_web)] = on_both(bytes);
    let natively = natively.unwrap_or_else(|error| panic!("{what}, natively: {error:?}"));
    let on_the_web = on_the_web.unwrap_or_else(|error| panic!("{what}, on the web: {error:?}"));
    assert_eq!(
        natively, on_the_web,
        "{what}: the two engines wrote different bytes"
    );
}

#[test]
fn an_appearance_with_no_bounding_box_is_refused() {
    // MEASURED (#229): with no `/BBox` PDFium only moves the appearance to the `/Rect`'s corner,
    // and an appearance drawing 290 points up left 654 dark pixels in the region after an `Ok`.
    let bytes = annotated("", "", UPRIGHT, "", "<< /N 7 0 R >>", &[appearance("")]);
    refused_on_both(&bytes, "annotation-appearance-unbounded", "no /BBox");
    for (bbox, what) in [
        ("/BBox 5", "a /BBox that is not an array"),
        ("/BBox [0 0 100]", "a /BBox of three numbers"),
        ("/BBox [0 0 0 20]", "a /BBox with no width"),
        (
            "/BBox [0 0 100 (x)]",
            "a /BBox with an item that is not a number",
        ),
    ] {
        let bytes = annotated("", "", UPRIGHT, "", "<< /N 7 0 R >>", &[appearance(bbox)]);
        refused_on_both(&bytes, "annotation-appearance-unbounded", what);
    }
    // THE NEAR-MISS: the same appearance, bounded, redacts.
    let bytes = annotated(
        "",
        "",
        UPRIGHT,
        "",
        "<< /N 7 0 R >>",
        &[appearance("/BBox [0 0 100 20]")],
    );
    redacts_on_both(&bytes, "a bounded appearance");
}

#[test]
fn every_appearance_state_is_checked_not_only_the_normal_one() {
    // `/N` bounded; the only unbounded appearance is the `/On` state of `/D`, which a viewer draws
    // while the annotation is pressed. And the same for `/R`, drawn under the pointer.
    for kind in ["/D", "/R"] {
        let bytes = annotated(
            "",
            "",
            UPRIGHT,
            "/AS /Off",
            &format!("<< /N 7 0 R {kind} << /Off 7 0 R /On 8 0 R >> >>"),
            &[appearance("/BBox [0 0 100 20]"), appearance("")],
        );
        refused_on_both(
            &bytes,
            "annotation-appearance-unbounded",
            &format!("an unbounded {kind} state"),
        );
    }
}

#[test]
fn a_no_rotate_annotation_on_a_turned_page_is_refused() {
    // MEASURED (#229): PDFium turns a NoRotate appearance about the `/Rect`'s corner; on a page
    // turned 90 degrees it put 1,800 dark pixels in a region beside the `/Rect` that the `/Rect`
    // does not meet, after an `Ok`.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    let bytes = annotated(
        "/Rotate 90",
        "",
        TURNED,
        "/F 16",
        "<< /N 7 0 R >>",
        &bounded,
    );
    refused_on_both(&bytes, "annotation-no-rotate", "NoRotate on a turned page");
    // With other flags beside it, and an `/F` that is not an integer.
    for flags in ["/F 20", "/F 16.0"] {
        let bytes = annotated("/Rotate 90", "", TURNED, flags, "<< /N 7 0 R >>", &bounded);
        refused_on_both(&bytes, "annotation-no-rotate", flags);
    }
    // THE TWINS: NoRotate on an upright page, and a turned page without it, both redact.
    let bytes = annotated("", "", UPRIGHT, "/F 16", "<< /N 7 0 R >>", &bounded);
    redacts_on_both(&bytes, "NoRotate on an upright page");
    let bytes = annotated("/Rotate 90", "", TURNED, "/F 4", "<< /N 7 0 R >>", &bounded);
    redacts_on_both(&bytes, "a bounded appearance on a turned page");
}

#[test]
fn an_inherited_rotate_is_refused_before_the_annotation_is_read() {
    // THE CASE A CHECK ON THE PAGE DICTIONARY ALONE WOULD MISS, and why this check does not: it
    // takes the frame's `/Rotate`, which refuses one inherited from the page tree first (#224).
    // So this pins the order: the page never reaches the annotation step.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    let bytes = annotated(
        "",
        "/Rotate 90",
        TURNED,
        "/F 16",
        "<< /N 7 0 R >>",
        &bounded,
    );
    refused_on_both(
        &bytes,
        "page-attribute-inherited",
        "NoRotate under an inherited /Rotate",
    );
}

#[test]
fn every_turned_angle_refuses_no_rotate_and_a_full_turn_does_not() {
    // THE CODE REVIEW OF #229: only 90 was tested, so a rule firing at 90 alone passed. The
    // `/Rect` sits clear of the band at every angle here; `TURNED` meets it at 270, where the
    // annotation is rightly removed and the redaction is `Ok`.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    for rotate in [
        "/Rotate 90",
        "/Rotate 180",
        "/Rotate 270",
        "/Rotate -90",
        "/Rotate 450",
    ] {
        let bytes = annotated(
            rotate,
            "",
            "[100 100 200 120]",
            "/F 16",
            "<< /N 7 0 R >>",
            &bounded,
        );
        refused_on_both(&bytes, "annotation-no-rotate", rotate);
    }
    // A FULL TURN IS UPRIGHT: the frame normalises 360 and -360 to 0, so NoRotate changes
    // nothing and the page redacts. A rule reading the raw `/Rotate` would refuse these.
    for rotate in ["/Rotate 360", "/Rotate -360"] {
        let bytes = annotated(rotate, "", UPRIGHT, "/F 16", "<< /N 7 0 R >>", &bounded);
        redacts_on_both(&bytes, rotate);
    }
    // AND NO `/F` AT ALL on a turned page is not NoRotate: an ordinary annotation redacts.
    let bytes = annotated("/Rotate 90", "", TURNED, "", "<< /N 7 0 R >>", &bounded);
    redacts_on_both(&bytes, "a turned page, an annotation with no /F");
}

#[test]
fn every_way_a_bounding_box_fails_to_bound_is_refused() {
    // The code review of #229 found the height check and the out-of-range arm unwitnessed.
    for (bbox, what) in [
        ("/BBox [0 0 100 0]", "a /BBox with no height"),
        (
            "/BBox [0 0 4294967396 20]",
            "a /BBox past what readers agree on",
        ),
    ] {
        let bytes = annotated("", "", UPRIGHT, "", "<< /N 7 0 R >>", &[appearance(bbox)]);
        refused_on_both(&bytes, "annotation-appearance-unbounded", what);
    }
}

#[test]
fn a_rect_written_with_its_corners_reversed_is_still_over_the_region() {
    // NORMALISED BY THE SHARED BOX READER. `[300 70 0 30]` names the band with its corners
    // swapped; un-normalised it would compare as empty and the annotation over the secret would
    // be kept. Removed, so the redaction is `Ok` and the annotation is gone from the output.
    let bytes = annotated(
        "",
        "",
        "[300 400 0 340]",
        "",
        "<< /N 7 0 R >>",
        &[appearance("/BBox [0 0 100 20]")],
    );
    let [(_, natively), (_, on_the_web)] = on_both(&bytes);
    for (engine, output) in [("natively", natively), ("on the web", on_the_web)] {
        let output = output.unwrap_or_else(|error| panic!("{engine}: {error:?}"));
        assert!(
            !output
                .windows(b"/Subtype /Stamp".len())
                .any(|w| w == b"/Subtype /Stamp"),
            "{engine}: the annotation over the region was kept"
        );
    }
}

#[test]
fn a_markup_annotation_whose_quadrilaterals_leave_its_rect_is_refused() {
    // THE SPECIFICATION REVIEW OF #229 (C1): PDFium can draw a markup annotation where its
    // `/QuadPoints` are -- measured with `/PDFIUM_HasGeneratedAP`, 1,304 dark pixels in the region
    // from a Highlight whose `/Rect` missed it. Keyed on the declared structure, not on the key.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    for subtype in ["/Highlight", "/Underline", "/Squiggly", "/StrikeOut"] {
        let outside = format!(
            "/Subtype {subtype} /QuadPoints [0 370 300 370 0 330 300 330] /PDFIUM_HasGeneratedAP true"
        );
        let bytes = annotated("", "", UPRIGHT, &outside, "<< /N 7 0 R >>", &bounded);
        refused_on_both(&bytes, "annotation-quads-outside-rect", subtype);
    }
    for (quads, what) in [
        ("/QuadPoints 5", "a /QuadPoints that is not an array"),
        (
            "/QuadPoints [20 70 (x) 70 20 50 120 50]",
            "a /QuadPoints item that is not a number",
        ),
    ] {
        let bytes = annotated(
            "",
            "",
            UPRIGHT,
            &format!("/Subtype /Highlight {quads}"),
            "<< /N 7 0 R >>",
            &bounded,
        );
        refused_on_both(&bytes, "annotation-quads-outside-rect", what);
    }
    // THE TWIN: the same Highlight with its quadrilateral inside its `/Rect` redacts, and so does
    // a Stamp carrying quadrilaterals outside its `/Rect` -- not a markup annotation.
    let inside =
        "/Subtype /Highlight /QuadPoints [20 70 120 70 20 50 120 50] /PDFIUM_HasGeneratedAP true";
    redacts_on_both(
        &annotated("", "", UPRIGHT, inside, "<< /N 7 0 R >>", &bounded),
        "quadrilaterals inside the /Rect",
    );
    redacts_on_both(
        &annotated(
            "",
            "",
            UPRIGHT,
            "/QuadPoints [0 370 300 370 0 330 300 330]",
            "<< /N 7 0 R >>",
            &bounded,
        ),
        "a Stamp's quadrilaterals, which nothing draws from",
    );
}

/// `count` annotations, clear of the region, all naming one `/AP` whose `/N` is a dictionary of
/// `states` bounded states.
fn many_sharing_one_appearance(count: usize, states: usize) -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET";
    let first_annotation = 9;
    let kids: String = (0..count)
        .map(|i| format!("{} 0 R ", first_annotation + i))
        .collect();
    let state_entries: String = (0..states).map(|i| format!("/S{i} 7 0 R ")).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> /Annots [{kids}] >>"
        ),
        stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
        format!("<< /N << {state_entries} >> >>"),
        appearance("/BBox [0 0 100 20]"),
        "null".to_owned(),
    ];
    for _ in 0..count {
        objects.push(format!(
            "<< /Type /Annot /Subtype /Stamp /Rect {UPRIGHT} /AP 6 0 R >>"
        ));
    }
    pdf(&objects)
}

#[test]
fn a_shared_appearance_dictionary_is_walked_once() {
    // THE CODE REVIEW OF #229: every annotation re-walked a shared `/AP`, and nothing read the
    // deadline -- 182.6 s on a 616 KB file. Visited once, by object identity, whatever the count.
    let bytes = many_sharing_one_appearance(200, 50);
    let walked = walked_natively(&bytes);
    assert_eq!(
        walked[0], 1,
        "200 annotations naming one /AP must walk it once"
    );
}

#[test]
fn the_annotation_walk_reads_the_deadline_while_it_works() {
    // AND A DEADLINE IS A DEADLINE. A clock that moves a second per reading, against a ten-second
    // deadline: the walk over 5,000 annotations must stop at it, as a limit, rather than run on.
    struct Ticking(std::sync::atomic::AtomicU64);
    impl burrow_types::Clock for Ticking {
        fn now_ms(&self) -> u64 {
            self.0
                .fetch_add(1_000, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let bytes = many_sharing_one_appearance(5_000, 1);
    let limits = Limits::with(|limits| limits.max_duration_ms = 10_000);
    let options = OpenOptions::new(limits, Arc::new(Ticking(0.into())) as Arc<dyn Clock>);
    let outcome = super::Qpdf.redact_page(&bytes, 0, &BTreeSet::from([0]), UPPER_BAND, &options);
    assert!(
        matches!(outcome, Err(burrow_types::Error::LimitExceeded { .. })),
        "5,000 annotations against a deadline that passes after ten readings: {outcome:?}"
    );
}

/// How far past its deadline the 616 KB file below may run, registered before measuring and
/// generous for a slow machine: after #229's fix it stopped 1 ms past a 1 s deadline in a release
/// build, where before an earlier step ran 9.3 s without reading it at all.
const REGISTERED_OVERSHOOT_MS: u128 = 2_000;

#[test]
fn the_616_kb_file_of_5000_annotations_sharing_one_appearance_stops_at_its_deadline() {
    // THE CODE REVIEW OF #229's FILE, rebuilt: 5,000 annotations naming one `/AP` whose `/N` has
    // 4,096 states. Measured before the fix at 182.6 s against a 60 s deadline (the review's run)
    // and 57.95 s to an `Ok` (this branch's); a real clock, a 1 s deadline.
    let bytes = many_sharing_one_appearance(5_000, 4_096);
    assert!(
        (600_000..640_000).contains(&bytes.len()),
        "the file is the review's size: {}",
        bytes.len()
    );
    let limits = Limits::with(|limits| limits.max_duration_ms = 1_000);
    let options = OpenOptions::new(
        limits,
        Arc::new(burrow_types::SystemClock::new()) as Arc<dyn Clock>,
    );
    let started = std::time::Instant::now();
    let outcome = super::Qpdf.redact_page(&bytes, 0, &BTreeSet::from([0]), UPPER_BAND, &options);
    let elapsed = started.elapsed().as_millis();
    assert!(
        matches!(outcome, Err(burrow_types::Error::LimitExceeded { .. })),
        "{outcome:?}"
    );
    assert!(
        elapsed < 1_000 + REGISTERED_OVERSHOOT_MS,
        "stopped {elapsed} ms after starting, against a 1,000 ms deadline and a registered \
         overshoot of {REGISTERED_OVERSHOOT_MS} ms"
    );
}

/// One page whose `/Annots` are `annotations`, in that order, as objects after `objects`, which
/// are numbered from 6. Each annotation is at `UPRIGHT` unless its entries name a `/Rect`.
fn page_of(objects: &[String], annotations: &[String]) -> Vec<u8> {
    let first = 6 + objects.len();
    let kids: String = (0..annotations.len())
        .map(|i| format!("{} 0 R ", first + i))
        .collect();
    let mut all = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> /Annots [{kids}] >>"
        ),
        stream("", "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
    all.extend(objects.iter().cloned());
    all.extend(annotations.iter().map(|entries| {
        // AT `UPRIGHT` unless the entries name their own `/Rect`.
        let rect = if entries.contains("/Rect") {
            String::new()
        } else {
            format!("/Rect {UPRIGHT}")
        };
        format!("<< /Type /Annot {rect} {entries} >>")
    }));
    pdf(&all)
}

/// How many appearance dictionaries, state dictionaries, streams and `/QuadPoints` arrays one
/// native redaction of `bytes` read, asserting it redacted.
fn walked_natively(bytes: &[u8]) -> [usize; 4] {
    let before = crate::redact::steps::appearance_objects_walked();
    let natively = super::Qpdf.redact_page(bytes, 0, &BTreeSet::from([0]), UPPER_BAND, &options());
    assert!(natively.is_ok(), "{natively:?}");
    let after = crate::redact::steps::appearance_objects_walked();
    core::array::from_fn(|i| after[i] - before[i])
}

#[test]
fn an_object_checked_in_one_role_is_still_checked_in_another() {
    // BOTH REVIEWS OF #229's MEMO: one set for every role let an object read as an `/AP` -- for
    // `/N`, `/D` and `/R` only -- be skipped when reached as a state dictionary, so its other
    // states were never read. Measured `Ok` with 654 dark pixels of ink in the region.
    let unbounded = appearance("");
    let bounded = appearance("/BBox [0 0 100 20]");

    // An `/AP` naming itself as its `/N`: PDFium reads it again as the state dictionary.
    refused_on_both(
        &page_of(
            &["<< /N 6 0 R /Off 7 0 R >>".to_owned(), unbounded.clone()],
            &["/Subtype /Stamp /AS /Off /AP 6 0 R".to_owned()],
        ),
        "annotation-appearance-unbounded",
        "an /AP that is its own /N",
    );
    // One annotation's `/AP` that is another's state dictionary, in both orders: the walk runs
    // backwards, so the second order checks it as an `/AP` first.
    let shared = ["<< /N 8 0 R /On 7 0 R >>".to_owned(), unbounded, bounded];
    let as_states = "/Subtype /Stamp /AS /On /AP << /N 6 0 R >>".to_owned();
    let as_appearance = "/Subtype /Stamp /AP 6 0 R".to_owned();
    for (order, annotations) in [
        (
            "as a state dictionary first",
            [as_appearance.clone(), as_states.clone()],
        ),
        ("as an /AP first", [as_states, as_appearance]),
    ] {
        refused_on_both(
            &page_of(&shared, &annotations),
            "annotation-appearance-unbounded",
            &format!("an /AP shared as another's state dictionary, read {order}"),
        );
    }
}

#[test]
fn each_memo_reads_a_shared_object_once() {
    // THE OTHER TWO ROLES, and the `/QuadPoints` extent, each shared by 50 annotations whose own
    // `/AP` is direct -- so the dictionary memo cannot be what saves the work.
    let count = 50;
    let states = page_of(
        &[
            "<< /S0 7 0 R /S1 7 0 R >>".to_owned(),
            appearance("/BBox [0 0 100 20]"),
        ],
        &vec!["/Subtype /Stamp /AP << /N 6 0 R >>".to_owned(); count],
    );
    let walked = walked_natively(&states);
    assert_eq!(walked[0], count, "each direct /AP is read: {walked:?}");
    assert_eq!(
        walked[1], 1,
        "one shared state dictionary is read once: {walked:?}"
    );
    assert_eq!(walked[2], 1, "one shared stream is read once: {walked:?}");

    let quads = page_of(
        &[
            "[20 70 120 70 20 50 120 50]".to_owned(),
            appearance("/BBox [0 0 100 20]"),
        ],
        &vec!["/Subtype /Highlight /QuadPoints 6 0 R /AP << /N 7 0 R >>".to_owned(); count],
    );
    let walked = walked_natively(&quads);
    assert_eq!(
        walked[3], 1,
        "one shared /QuadPoints is read once: {walked:?}"
    );
}

#[test]
fn a_subtype_that_is_not_a_name_is_checked_as_markup() {
    // #229's REVIEWS: PDFium reads `/Subtype` as a byte string, so `(Highlight)` is a Highlight to
    // it, drawn at its quadrilaterals -- `Ok` with 1,304 dark pixels in the region when only a
    // name was read. Anything but a name or nothing is checked, whatever it spells.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    for subtype in ["(Highlight)", "<486967686c69676874>", "(Text)", "5"] {
        refused_on_both(
            &page_of(
                &bounded,
                &[format!(
                    "/Subtype {subtype} /QuadPoints [0 370 300 370 0 330 300 330] \
                     /AP << /N 6 0 R >>"
                )],
            ),
            "annotation-quads-outside-rect",
            &format!("a /Subtype of {subtype}"),
        );
    }
    // THE NEAR-MISS: no `/Subtype` at all is no markup type PDFium knows.
    redacts_on_both(
        &page_of(
            &bounded,
            &["/QuadPoints [0 370 300 370 0 330 300 330] /AP << /N 6 0 R >>".to_owned()],
        ),
        "an annotation with no /Subtype",
    );
}

#[test]
fn each_edge_of_the_quadrilaterals_is_checked_on_its_own() {
    // THE CODE REVIEW: the one outside fixture left the `/Rect` on every side at once, so either
    // half of the range check could go. Each of these leaves it on one side, and is a real leak:
    // 462 and 351 dark pixels in the region, measured.
    let bounded = [appearance("/BBox [0 0 100 20]")];
    for (rect, quads, side) in [
        (
            UPRIGHT,
            "[20 370 120 370 20 330 120 330]",
            "above its /Rect only",
        ),
        (
            "[20 375 120 395]",
            "[20 365 120 365 20 335 120 335]",
            "below its /Rect only",
        ),
        (
            UPRIGHT,
            "[0 70 120 70 0 50 120 50]",
            "left of its /Rect only",
        ),
        (
            UPRIGHT,
            "[20 70 140 70 20 50 140 50]",
            "right of its /Rect only",
        ),
    ] {
        let bytes = page_of(
            &bounded,
            &[format!(
                "/Subtype /Highlight /Rect {rect} /QuadPoints {quads} /AP << /N 6 0 R >>"
            )],
        );
        refused_on_both(
            &bytes,
            "annotation-quads-outside-rect",
            &format!("quadrilaterals {side}"),
        );
    }
}

#[test]
fn quadrilaterals_past_the_ceiling_or_absent() {
    let bounded = [appearance("/BBox [0 0 100 20]")];
    // PAST THE CEILING, every number inside the `/Rect`: refused on the count alone.
    let many = "30 60 ".repeat(32_769);
    refused_on_both(
        &page_of(
            &bounded,
            &[format!(
                "/Subtype /Highlight /QuadPoints [{many}] /AP << /N 6 0 R >>"
            )],
        ),
        "annotation-quads-outside-rect",
        "65,538 numbers",
    );
    // AND NONE: a Highlight with no `/QuadPoints` is drawn at its `/Rect`.
    redacts_on_both(
        &page_of(
            &bounded,
            &["/Subtype /Highlight /AP << /N 6 0 R >>".to_owned()],
        ),
        "a Highlight with no /QuadPoints",
    );
}

#[test]
fn the_quadrilateral_memo_holds_extents_by_identity_and_never_verdicts() {
    let bounded = [appearance("/BBox [0 0 100 20]")];
    // DIRECT ARRAYS HAVE NO IDENTITY (#229's third code review): every inline `/QuadPoints` is
    // `(0, 0)`, so a memo without its guard let the first one read -- here the later annotation,
    // inside its `/Rect`, since the walk runs backwards -- answer for the other, over the region.
    refused_on_both(
        &page_of(
            &bounded,
            &[
                "/Subtype /Highlight /QuadPoints [0 370 300 370 0 330 300 330] \
                 /PDFIUM_HasGeneratedAP true /AP << /N 6 0 R >>"
                    .to_owned(),
                "/Subtype /Highlight /QuadPoints [20 70 120 70 20 50 120 50] /AP << /N 6 0 R >>"
                    .to_owned(),
            ],
        ),
        "annotation-quads-outside-rect",
        "two inline /QuadPoints, the later one inside its /Rect",
    );
    // AND ONE SHARED ARRAY UNDER TWO `/Rect`s: inside the later annotation's, outside the
    // earlier's. The extent is memoised; whether it fits is asked of each `/Rect`.
    refused_on_both(
        &page_of(
            &[
                "[20 70 120 70 20 50 120 50]".to_owned(),
                appearance("/BBox [0 0 100 20]"),
            ],
            &[
                "/Subtype /Highlight /Rect [130 50 230 70] /QuadPoints 6 0 R /AP << /N 7 0 R >>"
                    .to_owned(),
                "/Subtype /Highlight /QuadPoints 6 0 R /AP << /N 7 0 R >>".to_owned(),
            ],
        ),
        "annotation-quads-outside-rect",
        "one shared /QuadPoints, inside one /Rect and outside the other",
    );
}
