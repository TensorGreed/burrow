//! The flatten write on the real qpdf: fixtures, near-miss twins, and a lying write per read-back
//! check (ADR 0031 §4's fake list for V1 to V3, and *Fixtures and census*' rows this slice owns).
//!
//! Every lie is planted between the honest edit and the write (`super::lie`), so the write is the
//! engine's own and only the read-back stands between the lie and a returned document. Each test
//! names the check whose deletion turns it red.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use core::cell::Cell;
use core::ffi::c_int;
use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::super::Picture;
use super::lie::{Lie, plant};
use super::{CONTENTS_KEYS, IMAGE_KEYS, PAGE_KEYS, Reading, hooks};
use crate::qpdf::Qpdf;
use crate::redact::frames::PageFrameReading;
use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject, ReportsResolution};
use crate::{OpenOptions, PageFlattener, PageFrameReader};

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

/// A document of `pages`, each a page dictionary's extra entries, with `catalog` added to the
/// catalogue. Each page draws one line, so its content is not empty.
fn document(pages: &[&str], catalog: &str) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 3 + 2 * i))
        .collect();
    objects.push(format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"));
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    for (i, page) in pages.iter().enumerate() {
        // A PAGE WITH NO `/Resources` IS REFUSED by the frame reading this slice reuses (qpdf
        // warns, and a warning is a repair), so every page gets an empty one unless it names its
        // own.
        let resources = if page.contains("/Resources") {
            ""
        } else {
            "/Resources << >>"
        };
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R {page} {resources} /Contents {} 0 R >>",
            4 + 2 * i
        ));
        let line = "0 0 m 10 10 l S";
        objects.push(format!(
            "<< /Length {} >>\nstream\n{line}\nendstream",
            line.len() + 1
        ));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Page `page`'s display frame, read as the flatten reads it.
fn frame(bytes: &[u8], page: usize) -> (f64, f64) {
    let frames = Qpdf.page_frames(bytes, &options(Limits::DEFAULT)).unwrap();
    match &frames.readings[page] {
        PageFrameReading::Frame { width, height } => (*width, *height),
        PageFrameReading::Refused { error, .. } => panic!("page {page} has no frame: {error}"),
    }
}

/// A recognisable picture for `bytes`' page `page`: every pixel differs from its neighbours.
fn pixels_for_page(bytes: &[u8], page: usize) -> (u32, u32, Vec<u8>) {
    let (width, height) = frame(bytes, page);
    let wide = super::pixels_for(width).unwrap();
    let high = super::pixels_for(height).unwrap();
    let length = usize::try_from(u64::from(wide) * u64::from(high) * 3).unwrap();
    let rgb = (0..length).map(|i| (i % 251) as u8).collect();
    (wide, high, rgb)
}

/// Flatten under `limits`, with the operation's deadline started on the same clock.
fn flatten_with(
    bytes: &[u8],
    page: usize,
    picture: &Picture<'_>,
    limits: Limits,
) -> Result<Vec<u8>> {
    let options = options(limits);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    Qpdf.flatten_page(bytes, page, picture, &options, &deadline)
}

fn flatten(bytes: &[u8], page: usize) -> Result<Vec<u8>> {
    let (wide, high, rgb) = pixels_for_page(bytes, page);
    flatten_with(
        bytes,
        page,
        &Picture::new(wide, high, &rgb),
        Limits::DEFAULT,
    )
}

/// The output's page `page` dictionary keys, sorted, through a fresh open.
fn page_keys(output: &[u8], page: usize) -> Vec<Vec<u8>> {
    let (document, _) = Qpdf
        .open_for_redaction(output, &options(Limits::DEFAULT))
        .unwrap();
    let mut keys =
        crate::pdfsyntax::dict::top_level_keys(&document.page(page).unwrap().unparse()).unwrap();
    keys.sort();
    keys
}

/// The web's `max_pixels` (ADR 0027 §1), on the default limits.
fn web_limits() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.max_pixels = 4_194_304;
    limits
}

const LETTER: &str = "/MediaBox [0 0 612 792]";

/// The transform matrix's write half (ADR 0031 *Fixtures*, §4 transform matrix): every rotation
/// by box origin by crop offset flattens, and the written page holds the input's display frame
/// under `/Rotate 0`, with width and height swapped for 90 and 270.
#[test]
fn every_rotation_origin_and_crop_flattens_to_its_display_frame() {
    let mut examined = 0;
    for rotate in [0, 90, 180, 270] {
        for media in ["[0 0 300 500]", "[100 50 400 550]", "[-200 -100 100 400]"] {
            for crop in ["", "/CropBox [10 20 210 420]"] {
                // A crop offset on a box with a negative origin: shifted into it.
                let crop = if media.starts_with("[-") && !crop.is_empty() {
                    "/CropBox [-190 -80 10 320]"
                } else if media.starts_with("[100") && !crop.is_empty() {
                    "/CropBox [110 70 310 470]"
                } else {
                    crop
                };
                let page = format!("/MediaBox {media} {crop} /Rotate {rotate}");
                let input = document(&[&page, LETTER], "");
                let (width, height) = frame(&input, 0);
                let output = flatten(&input, 0).unwrap_or_else(|error| panic!("{page}: {error}"));
                assert_eq!(frame(&output, 0), (width, height), "{page}");
                assert_eq!(frame(&output, 1), (612.0, 792.0), "{page}: page 2 moved");
                assert_eq!(
                    page_keys(&output, 0),
                    PAGE_KEYS.map(<[u8]>::to_vec),
                    "{page}"
                );
                if media == "[0 0 300 500]" && crop.is_empty() {
                    // THE SWAP, stated: a quarter-turn page's frame is its box turned.
                    let expected = if rotate % 180 == 90 {
                        (500.0, 300.0)
                    } else {
                        (300.0, 500.0)
                    };
                    assert_eq!((width, height), expected, "{page}");
                }
                examined += 1;
            }
        }
    }
    assert_eq!(
        examined, 24,
        "the matrix is 4 rotations x 3 origins x 2 crops"
    );
}

/// §3's allowlist: every other key goes, including ones nobody listed. Twin: a minimal page.
/// KILLS: keeping any key outside the seven (V2's key check fails first, inside the engine).
#[test]
fn every_key_outside_the_seven_is_removed() {
    let crowded = format!(
        "{LETTER} /Annots [] /AA << >> /B [] /StructParents 0 /PieceInfo << >> \
         /Group << /S /Transparency >> /Tabs /S /ArtBox [0 0 1 1] /BleedBox [0 0 1 1] \
         /TrimBox [0 0 1 1] /UserUnit 1 /BurrowNobodyListed 7 /Resources << >>"
    );
    for page in [crowded.as_str(), LETTER] {
        let output = flatten(&document(&[page], ""), 0).unwrap();
        assert_eq!(
            page_keys(&output, 0),
            PAGE_KEYS.map(<[u8]>::to_vec),
            "{page}"
        );
    }
}

/// §3's `/CropBox`: an inherited one is still refused by the frame reading this slice reuses
/// (`[page-attribute-inherited]`); lifting that is an acceptance for the census, not this slice.
/// Twin: a declared crop smaller than the box flattens to the crop.
#[test]
fn an_inherited_crop_box_is_refused_and_a_declared_one_flattens_to_the_crop() {
    let declared = document(&["/MediaBox [0 0 600 800] /CropBox [0 0 300 400]"], "");
    let output = flatten(&declared, 0).unwrap();
    assert_eq!(frame(&output, 0), (300.0, 400.0));

    let tree = tree_with_inherited_crop();
    let error = flatten_unframed(&tree, 0).expect_err("an inherited crop is not flattened yet");
    assert!(
        error.to_string().contains("[page-attribute-inherited]"),
        "{error}"
    );
}

/// A one-page document whose `/CropBox` is on the page tree node.
fn tree_with_inherited_crop() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /CropBox [0 0 300 400] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << >> /Contents 4 0 R >>"
            .to_owned(),
        "<< /Length 16 >>\nstream\n0 0 m 10 10 l S\nendstream".to_owned(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// Flatten with a picture of any size: for inputs whose frame is refused before the size matters.
fn flatten_unframed(bytes: &[u8], page: usize) -> Result<Vec<u8>> {
    flatten_with(bytes, page, &Picture::new(1, 1, &[0; 3]), Limits::DEFAULT)
}

/// §2 rule 1's drop. Twin: an untagged document. KILLS: keeping `/MarkInfo` or
/// `/StructTreeRoot` (V4's catalogue half refuses).
#[test]
fn a_tagged_document_loses_its_tags_and_an_untagged_one_is_unchanged() {
    for catalog in [
        "/StructTreeRoot << /Type /StructTreeRoot >> /MarkInfo << /Marked true >>",
        "",
    ] {
        let output = flatten(&document(&[LETTER, LETTER], catalog), 0).unwrap();
        let (document, _) = Qpdf
            .open_for_redaction(&output, &options(Limits::DEFAULT))
            .unwrap();
        let root = document
            .trailer()
            .unwrap()
            .key(&crate::name::Name::literal(b"/Root\0"));
        let keys = crate::pdfsyntax::dict::top_level_keys(&root.unparse()).unwrap();
        assert!(
            !keys
                .iter()
                .any(|k| k == b"StructTreeRoot" || k == b"MarkInfo"),
            "{catalog}: {keys:?}"
        );
    }
}

/// Rule 6's ceilings, on the product, before the picture's size or length is looked at: a page
/// exactly at a ceiling passes the ceiling (and fails only on the buffer handed in, which is
/// empty), and one row more is refused by it. KILLS: checking after allocation; dropping either
/// ceiling.
#[test]
fn the_pixel_ceilings_are_exact() {
    let at_web = document(&["/MediaBox [0 0 983 983]"], "");
    let over_web = document(&["/MediaBox [0 0 983 983.5]"], "");
    let web = web_limits();
    let empty = |bytes: &[u8], wide, high, limits| {
        flatten_with(bytes, 0, &Picture::new(wide, high, &[]), limits)
    };
    assert!(matches!(
        empty(&at_web, 2048, 2048, web),
        Err(Error::Internal(ref what)) if what.contains("three bytes per pixel")
    ));
    assert!(matches!(
        empty(&over_web, 2048, 2049, web),
        Err(Error::LimitExceeded {
            limit: "max_pixels",
            requested: 4_196_352,
            ..
        })
    ));

    let at_native = document(&["/MediaBox [0 0 2992.5 6888.25]"], "");
    let over_native = document(&["/MediaBox [0 0 2992.5 6888.75]"], "");
    assert!(matches!(
        empty(&at_native, 6235, 14351, Limits::DEFAULT),
        Err(Error::Internal(ref what)) if what.contains("three bytes per pixel")
    ));
    assert!(matches!(
        empty(&over_native, 6235, 14352, Limits::DEFAULT),
        Err(Error::LimitExceeded {
            limit: "redaction_flatten_pixels",
            ..
        })
    ));
}

/// The web's ceiling, honoured end to end: a 983 × 983 page flattens under it.
#[test]
fn a_page_at_the_web_ceiling_flattens() {
    let bytes = document(&["/MediaBox [0 0 983 983]"], "");
    let (wide, high, rgb) = pixels_for_page(&bytes, 0);
    assert_eq!((wide, high), (2048, 2048));
    let limits = web_limits();
    flatten_with(&bytes, 0, &Picture::new(wide, high, &rgb), limits).unwrap();
}

/// §1 step 6's size and length: the drawing side's size is never trusted. KILLS: trusting the
/// reported size; deleting the length check.
#[test]
fn a_picture_of_the_wrong_size_or_length_is_refused() {
    let bytes = document(&[LETTER], "");
    let (wide, high, rgb) = pixels_for_page(&bytes, 0);
    let run = |wide, high, rgb: &[u8]| {
        flatten_with(&bytes, 0, &Picture::new(wide, high, rgb), Limits::DEFAULT)
    };
    let mut long = rgb.clone();
    long.push(0);
    for (case, result) in [
        ("W + 1", run(wide + 1, high, &rgb)),
        ("H + 1", run(wide, high + 1, &rgb)),
        ("one byte short", run(wide, high, &rgb[1..])),
        ("one byte long", run(wide, high, &long)),
    ] {
        assert!(
            matches!(result, Err(Error::Internal(_))),
            "{case}: {result:?}"
        );
    }
    run(wide, high, &rgb).expect("the exact size and length flatten");
}

/// Each read-back check, against a write that lies (ADR 0031 §4's fakes for V1 to V3, and V4's
/// catalogue half). Twin: the honest write, which every other test here is. KILLS: deleting the
/// check each lie names.
#[test]
fn every_lie_in_the_write_is_rejected_by_the_read_back() {
    let rotated = document(&["/MediaBox [0 0 300 500] /Rotate 90", LETTER], "");
    let plain = document(&[LETTER, "/MediaBox [0 0 300 500]"], "");
    let cases: [(Lie, &[u8], &str); 24] = [
        (
            Lie::ExtraPageKey,
            &plain,
            "the flattened page does not have exactly the keys",
        ),
        (Lie::CropBoxUnequal, &plain, "/CropBox is not [0 0 dw dh]"),
        (Lie::RotateNinety, &plain, "display frame"),
        (Lie::MediaBoxUnswapped, &rotated, "display frame"),
        (
            Lie::SecondResource,
            &plain,
            "/Resources does not have exactly the keys",
        ),
        (
            Lie::ContentByteOff,
            &plain,
            "the content line is not the one written",
        ),
        (
            Lie::ImageRowShort,
            &plain,
            "the image is not W x H x 3 bytes",
        ),
        (
            Lie::ImageByteLong,
            &plain,
            "the image is not W x H x 3 bytes",
        ),
        (
            Lie::PixelUnpainted,
            &plain,
            "the image's pixels are not the picture's",
        ),
        (
            Lie::ImageExtraKey,
            &plain,
            "the image does not have exactly the keys",
        ),
        (Lie::WidthOff, &plain, "/Width is not the integer written"),
        (Lie::OtherPageFrame, &plain, "page 2 of the output"),
        (Lie::KeepMarkInfo, &plain, "structure tree or /MarkInfo"),
        (
            Lie::ParentReplaced,
            &plain,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::ContentsExtraKey,
            &plain,
            "the content stream does not have exactly the keys",
        ),
        (
            Lie::SecondImageName,
            &plain,
            "/Resources /XObject does not have exactly the keys",
        ),
        (Lie::HeightOff, &plain, "/Height is not the integer written"),
        (
            Lie::ColorSpaceOther,
            &plain,
            "/ColorSpace is not the name written",
        ),
        (Lie::FilterOther, &plain, "/Filter is not the name written"),
        (
            Lie::BitsPerComponentOff,
            &plain,
            "/BitsPerComponent is not the integer written",
        ),
        // QPDF'S OWN PAGE-TREE READ rewrites a page whose `/Type` is not `/Page`, and a repair is
        // refused: V1's read refuses this lie before V2's `/Type` check sees it.
        (Lie::PageTypeOther, &plain, "its frames cannot be read"),
        (
            Lie::ImageTypeOther,
            &plain,
            "the image's /Type is not the name written",
        ),
        (
            Lie::ImageSubtypeOther,
            &plain,
            "the image's /Subtype is not the name written",
        ),
        // qpdf DECODES EVERY `/FlateDecode` it is handed, or fails: its read is an error here, and
        // the `None` arm (a filter qpdf does not decode) is unreachable past the `/Filter` check.
        (Lie::ImageUndecodable, &plain, "its image cannot be decoded"),
    ];
    // `/MediaBox` HAS NO LIE OF ITS OWN: a box that differs is a frame that differs, and V1's
    // frame check refuses it first (`Lie::MediaBoxUnswapped`, `Lie::RotateNinety`).
    for (lie, bytes, says) in cases {
        plant(lie);
        let error = flatten(bytes, 0).expect_err("a lying write must not come back");
        assert!(
            matches!(error, Error::OutputRejected(ref what) if what.contains(says)),
            "{lie:?}: expected a rejection saying {says:?}, got {error:?}"
        );
    }
    // AND THE LIE IS SPENT: the next flatten is honest.
    flatten(&plain, 0).expect("an unplanted flatten is honest");
}

/// The `/Parent` hold, on a nested tree: page *N* two levels down flattens, and a write that hands
/// it a different `/Parent` is refused. KILLS: deleting V2's `/Parent` place check.
#[test]
fn page_n_keeps_its_place_in_a_nested_tree() {
    let nested = nested_tree();
    flatten(&nested, 1).expect("a page two levels down flattens");
    plant(Lie::ParentReplaced);
    let error = flatten(&nested, 1).expect_err("a write that replaces /Parent must not come back");
    assert!(
        matches!(error, Error::OutputRejected(ref what) if what.contains("/Parent")),
        "{error:?}"
    );
}

/// One lie per way the page-tree climb can end (S1 round 2, L-6), on the nested tree's page 1
/// (input place: steps `[1, 0]`, ending at the catalogue's `/Pages`). Each changes nothing qpdf
/// enumerates from the catalogue, so only V2's place comparison refuses it; between them they
/// pin the climb's ends one by one: `Unlisted` through the `/Kids` search at the first step and
/// one step up, `Detached` one step up, the same steps with a different end, and the same end with
/// a different step.
///
/// The depth bound (`TooDeep`) is NOT reachable this way, measured: a loop above page *N* is also
/// above every page sharing its `/Parent`, and the frame reading V1 runs climbs `/Parent` for
/// inherited attributes and refuses it first. [`Lie::AncestorLoops`] is kept, asserting that
/// shadowing, so a change that lets the loop past V1 is seen.
#[test]
fn every_way_the_climb_can_end_differently_is_rejected() {
    let nested = nested_tree();
    for (lie, says) in [
        (
            Lie::ParentUnlisting,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::AncestorDetached,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::AncestorUnlisted,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::AncestorEndsElsewhere,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::AncestorShifted,
            "/Parent is not the input's place in the page tree",
        ),
        (
            Lie::AncestorLoops,
            "does not have the input's display frame",
        ),
    ] {
        plant(lie);
        let error = flatten(&nested, 1).expect_err("a write that moves page N must not come back");
        assert!(
            matches!(error, Error::OutputRejected(ref what) if what.contains(says)),
            "{lie:?}: expected a rejection saying {says:?}, got {error:?}"
        );
    }
    flatten(&nested, 1).expect("an unplanted flatten is honest");
}

/// The `/Contents` stream's dictionary holds only what qpdf's write adds.
#[test]
fn the_content_stream_has_exactly_its_keys() {
    let output = flatten(&document(&[LETTER], ""), 0).unwrap();
    let (document, _) = Qpdf
        .open_for_redaction(&output, &options(Limits::DEFAULT))
        .unwrap();
    let contents = document
        .page(0)
        .unwrap()
        .key(&crate::name::Name::literal(b"/Contents\0"));
    let mut keys =
        crate::pdfsyntax::dict::top_level_keys(&contents.stream_dict().unparse()).unwrap();
    keys.sort();
    assert_eq!(keys, CONTENTS_KEYS.map(<[u8]>::to_vec));
}

/// The flatten runs on the operation's deadline, not on one of its own: a budget already spent
/// when it is called is refused. Twin: the same call on an unspent budget flattens. KILLS:
/// starting a fresh `Deadline` inside the flatten.
#[test]
fn the_flatten_spends_the_operation_s_deadline() {
    let bytes = document(&[LETTER], "");
    let (wide, high, rgb) = pixels_for_page(&bytes, 0);
    let picture = Picture::new(wide, high, &rgb);
    let clock = Arc::new(ManualClock::new(0));
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = 1_000;
    let options = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    let deadline = Deadline::start(clock.as_ref(), &options.limits);
    Qpdf.flatten_page(&bytes, 0, &picture, &options, &deadline)
        .expect("an unspent budget flattens");
    clock.advance(1_001);
    let error = Qpdf
        .flatten_page(&bytes, 0, &picture, &options, &deadline)
        .expect_err("a spent budget must not flatten");
    assert!(
        matches!(
            error,
            Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            }
        ),
        "{error:?}"
    );
}

/// Catalogue → `/Pages` → an intermediate node → two letter pages: page 1 is two levels down.
fn nested_tree() -> Vec<u8> {
    let line = "0 0 m 10 10 l S";
    let stream = format!(
        "<< /Length {} >>\nstream\n{line}\nendstream",
        line.len() + 1
    );
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>".to_owned(),
        format!("<< /Type /Page /Parent 3 0 R {LETTER} /Resources << >> /Contents 6 0 R >>"),
        format!("<< /Type /Page /Parent 3 0 R {LETTER} /Resources << >> /Contents 6 0 R >>"),
        stream,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// The rotated twins: V2's `/Rotate` check is not shadowed by V1's frames on a square page, where
/// a turned page has the same frame. KILLS: deleting V2's `/Rotate 0` check.
#[test]
fn a_turned_square_page_is_rejected_by_the_rotate_check() {
    let square = document(&["/MediaBox [0 0 400 400]"], "");
    plant(Lie::RotateNinety);
    let error = flatten(&square, 0).expect_err("a square page written /Rotate 90");
    assert!(
        matches!(error, Error::OutputRejected(ref what) if what.contains("/Rotate")),
        "{error:?}"
    );
}

/// The image's keys are exactly the eight V3 names, and its filter is qpdf's Flate.
#[test]
fn the_image_is_flate_rgb_with_exactly_its_keys() {
    let output = flatten(&document(&[LETTER], ""), 0).unwrap();
    let (document, _) = Qpdf
        .open_for_redaction(&output, &options(Limits::DEFAULT))
        .unwrap();
    let image = document
        .page(0)
        .unwrap()
        .key(&crate::name::Name::literal(b"/Resources\0"))
        .key(&crate::name::Name::literal(b"/XObject\0"))
        .key(&crate::name::Name::literal(b"/Im0\0"));
    let mut keys = crate::pdfsyntax::dict::top_level_keys(&image.stream_dict().unparse()).unwrap();
    keys.sort();
    assert_eq!(keys, IMAGE_KEYS.map(<[u8]>::to_vec));
}

/// Golden: the output is deterministic, and its page *N* holds the fixed line for its frame.
#[test]
fn the_output_is_deterministic_and_draws_the_fixed_line() {
    let bytes = document(&["/MediaBox [0 0 595.28 841.89]", LETTER], "");
    let first = flatten(&bytes, 0).unwrap();
    assert_eq!(first, flatten(&bytes, 0).unwrap(), "two runs differ");
    let (document, _) = Qpdf
        .open_for_redaction(&first, &options(Limits::DEFAULT))
        .unwrap();
    let line = document.page(0).unwrap().page_content().unwrap();
    assert_eq!(line, b"q 595.28 0 0 841.89 0 0 cm /Im0 Do Q\n");
}

/// A page with its own refusal is refused with it; a page past the end is an invalid argument.
#[test]
fn a_page_without_a_frame_or_past_the_end_is_refused() {
    let error = flatten_unframed(&document(&["/MediaBox [0 0 300 500] /UserUnit 2"], ""), 0)
        .expect_err("a /UserUnit page has no frame");
    assert!(error.to_string().contains("[user-unit-not-one]"), "{error}");
    let error = flatten_unframed(&document(&[LETTER], ""), 1).expect_err("there is no page 2");
    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
}

/// §1's resolution at open: an object nothing on the page tree reaches, whose cross-reference
/// offset is wrong, is resolved by the open's forced resolution -- inside the trap -- and the
/// repair refuses. Twin: the same file with the offset right. KILLS: skipping the forced
/// resolution -- measured in slice 2, the flag then reads clear at the open on the twin and the
/// broken file alike, so this test's twin is refused and it goes red on its own, with every
/// re-check in place.
#[test]
fn a_repair_at_the_open_s_resolution_is_refused() {
    let well_formed = with_unreferenced_object(false);
    flatten(&well_formed, 0).expect("the twin, with its offset right, flattens");
    let broken = with_unreferenced_object(true);
    let error = flatten(&broken, 0).expect_err("a repair must refuse");
    assert!(
        error.to_string().contains("[engine-repaired-input]"),
        "expected the repair refusal, got {error}"
    );
}

/// One letter page plus object 5, which nothing references; `broken` points its cross-reference
/// entry 4 bytes into the object rather than at it.
fn with_unreferenced_object(broken: bool) -> Vec<u8> {
    resolution_fixture(Repair::OffsetInside, broken)
}

/// The ways an object nothing references can be repaired when qpdf's whole-file resolution reaches
/// it, one per path through qpdf's object read (12.4.1, `QPDF_objects.cc`). Each is reached only by
/// the resolution: no page, no frame and no trailer key leads to object 5, so the open alone reads
/// none of them (`each_resolution_fixture_repairs_only_at_the_forced_resolution`).
#[derive(Debug, Clone, Copy)]
enum Repair {
    /// The cross-reference offset lands 4 bytes inside the object: qpdf **reconstructs the
    /// cross-reference table**, which also resets `fixed_dangling_refs` mid-resolution
    /// (`reconstruct_xref`, `QPDF_objects.cc:332`) -- the path where the flag is cleared and set
    /// again inside one forced resolution.
    OffsetInside,
    /// The cross-reference offset is 0: qpdf warns "object has offset 0" and reads the object as
    /// null, with no reconstruction (`readObjectAtOffset`).
    OffsetZero,
    /// Object 5 is a stream whose `/Length` runs past its data: qpdf warns and recovers the length
    /// while parsing it, with no reconstruction.
    StreamLength,
}

impl Repair {
    const ALL: [Self; 3] = [Self::OffsetInside, Self::OffsetZero, Self::StreamLength];
}

/// One letter page plus object 5, which nothing references, repaired as `repair` says when
/// `broken`. The twin (`broken` false) is the same file well formed: every byte but the one fault
/// is the same.
fn resolution_fixture(repair: Repair, broken: bool) -> Vec<u8> {
    let unreferenced = match repair {
        Repair::OffsetInside | Repair::OffsetZero => "<< /BurrowUnreferenced 1 >>".to_owned(),
        // `BurrowUnreferenced\n` is 19 bytes; 99 runs past `endstream`.
        Repair::StreamLength => format!(
            "<< /Length {} >>\nstream\nBurrowUnreferenced\nendstream",
            if broken { 99 } else { 19 }
        ),
    };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Contents 4 0 R >>"
            .to_owned(),
        "<< /Length 16 >>\nstream\n0 0 m 10 10 l S\nendstream".to_owned(),
        unreferenced,
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    match (repair, broken) {
        (Repair::OffsetInside, true) => offsets[4] += 4,
        (Repair::OffsetZero, true) => offsets[4] = 0,
        _ => {}
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// What makes the resolution fixtures fixtures for §1's resolution at open (slice 2's row): for
/// each [`Repair`], the redaction's open **alone** reports no repair, the forced resolution S1
/// routes through `qpdf_make_indirect_object` reports one on the broken file and none on its twin,
/// and the flatten refuses the one and returns the other. A fixture the open already repaired would
/// be refused before the resolution ran, and could not tell a skipped resolution from a run one.
#[test]
fn each_resolution_fixture_repairs_only_at_the_forced_resolution() {
    use crate::redact::graph::BuildsObjects;

    for repair in Repair::ALL {
        for broken in [true, false] {
            let bytes = resolution_fixture(repair, broken);
            let (document, _) = Qpdf
                .open_for_redaction(&bytes, &options(Limits::DEFAULT))
                .unwrap_or_else(|error| panic!("{repair:?}/{broken}: the open refused: {error}"));
            assert!(
                !document.repaired(),
                "{repair:?}/{broken}: the open alone must not reach object 5"
            );
            let trailer = document.trailer().unwrap();
            let _resolved = trailer.null_beside().made_indirect().unwrap();
            assert_eq!(
                document.repaired(),
                broken,
                "{repair:?}/{broken}: the forced resolution's repair"
            );
            drop((_resolved, trailer));
            drop(document);
            match flatten(&bytes, 0) {
                Ok(_) if !broken => {}
                Err(error) if broken => assert!(
                    error.to_string().contains("[engine-repaired-input]"),
                    "{repair:?}: expected the repair refusal, got {error}"
                ),
                other => panic!("{repair:?}/{broken}: {:?}", other.map(|bytes| bytes.len())),
            }
        }
    }
}

/// Every flag reading the last flatten on this thread made, in order; cleared.
fn flag_readings() -> Vec<(Reading, bool)> {
    hooks::FLAG_READINGS.with(|readings| core::mem::take(&mut *readings.borrow_mut()))
}

/// (89)(1): qpdf's resolved-once flag reads **clear** after the redaction's open and **set**
/// after the forced resolution, on each well-formed twin. That this is a false-to-true fixture is
/// what the first reading measures: the open did not resolve the table, so the forced resolution
/// is the call that set the flag. The broken files are read too, and stated: each reads clear
/// after the open (the open alone never reaches object 5) and set after the forced resolution,
/// including `OffsetInside`, whose reconstruction clears the flag partway and whose second pass
/// sets it again (`QPDF::fixDanglingReferences`, 12.4.1) -- which is why that file is not the
/// one this test rests on, and why the repair check, not the flag, refuses it.
#[test]
fn the_flag_reads_clear_before_the_forced_resolution_and_set_after() {
    use crate::redact::graph::BuildsObjects;

    let mut examined = 0;
    for repair in Repair::ALL {
        for broken in [false, true] {
            let bytes = resolution_fixture(repair, broken);
            let (document, _) = Qpdf
                .open_for_redaction(&bytes, &options(Limits::DEFAULT))
                .unwrap();
            assert!(
                !document.resolved().unwrap(),
                "{repair:?}/{broken}: the open alone must not have resolved the table"
            );
            let trailer = document.trailer().unwrap();
            let _resolved = trailer.null_beside().made_indirect().unwrap();
            assert!(
                document.resolved().unwrap(),
                "{repair:?}/{broken}: the forced resolution must set the flag"
            );
            examined += 1;
        }
    }
    assert_eq!(examined, 6, "three fixtures and their three twins");
}

/// The flag is read at all three points, each after the repair check there: on every twin it
/// reads set at the open, after the constructors and after the write, and the flatten returns; on
/// every broken file the repair check at the open refuses first, so no reading is taken.
#[test]
fn the_flag_is_read_at_each_point_beside_the_repair_check() {
    let all_set = vec![
        (Reading::Open, true),
        (Reading::Constructors, true),
        (Reading::Write, true),
    ];
    for repair in Repair::ALL {
        flag_readings();
        flatten(&resolution_fixture(repair, false), 0)
            .unwrap_or_else(|error| panic!("{repair:?}: the twin must flatten: {error}"));
        assert_eq!(flag_readings(), all_set, "{repair:?}: the twin");

        let error = flatten(&resolution_fixture(repair, true), 0).expect_err("a repair refuses");
        assert!(
            error.to_string().contains("[engine-repaired-input]"),
            "{repair:?}: {error}"
        );
        assert_eq!(flag_readings(), [], "{repair:?}: refused before the flag");
    }
}

/// "Asserts that it ran" is the flag, not the call's return (ADR 0031's §1 row: "assert on the
/// call's return instead of `fixed_dangling_refs`"). With the forced resolution replaced by a call
/// that returns Ok and resolves nothing, every well-formed twin is refused at the open as
/// burrow's own failure. Without the flag check, the constructors would resolve the table outside
/// the trap with nothing to repair, and each would come back. Twin: the same files unhooked.
/// KILLS: checking the call's return instead of the flag at the open.
#[test]
fn a_forced_resolution_that_did_not_resolve_is_refused() {
    for repair in Repair::ALL {
        let bytes = resolution_fixture(repair, false);
        hooks::RESOLUTION_SKIPPED.with(|once| once.set(true));
        flag_readings();
        let error = flatten(&bytes, 0).expect_err("an unresolved open must not flatten");
        assert!(
            matches!(error, Error::Internal(ref what) if what.contains("forced resolution did not run")),
            "{repair:?}: {error:?}"
        );
        assert_eq!(flag_readings(), [(Reading::Open, false)], "{repair:?}");
        assert!(
            !hooks::RESOLUTION_SKIPPED.with(Cell::get),
            "the hook is spent"
        );
        flatten(&bytes, 0).unwrap_or_else(|error| panic!("{repair:?}: unhooked: {error}"));
    }
}

/// A document that answers as qpdf's does, except at one point: the flag reads clear there, or a
/// repair is reported there. The first document an engine opens lies; the read-back's is honest.
struct Lying<D> {
    inner: D,
    lie: Option<(Reading, Says)>,
    resolved_asked: Cell<usize>,
    repaired_asked: Cell<usize>,
}

/// What a [`Lying`] document says at its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Says {
    FlagClear,
    Repaired,
}

/// The order the flatten reads its three engine points in.
const POINTS: [Reading; 3] = [Reading::Open, Reading::Constructors, Reading::Write];

impl<D: PdfDocument> Lying<D> {
    fn lies(&self, says: Says, asked: &Cell<usize>) -> bool {
        let at = POINTS.get(asked.get()).copied();
        asked.set(asked.get() + 1);
        matches!(self.lie, Some((point, said)) if said == says && Some(point) == at)
    }
}

impl<D: PdfDocument> PdfDocument for Lying<D> {
    type Object<'a>
        = D::Object<'a>
    where
        Self: 'a;

    fn page_count(&self) -> Result<u64> {
        self.inner.page_count()
    }

    fn page(&self, index: usize) -> Result<Self::Object<'_>> {
        self.inner.page(index)
    }

    fn object(&self, number: c_int, generation: c_int) -> Result<Self::Object<'_>> {
        self.inner.object(number, generation)
    }

    fn trailer(&self) -> Result<Self::Object<'_>> {
        self.inner.trailer()
    }

    fn write(&self) -> Result<Vec<u8>> {
        self.inner.write()
    }

    fn repaired(&self) -> bool {
        self.lies(Says::Repaired, &self.repaired_asked) || self.inner.repaired()
    }
}

impl<D: ReportsResolution> ReportsResolution for Lying<D> {
    fn resolved(&self) -> Result<bool> {
        if self.lies(Says::FlagClear, &self.resolved_asked) {
            return Ok(false);
        }
        self.inner.resolved()
    }
}

/// qpdf, whose first document opened lies as [`Lying`] says.
struct LyingQpdf {
    lie: (Reading, Says),
    opened: Cell<bool>,
}

impl OpensForRedaction for LyingQpdf {
    type Document = Lying<<Qpdf as OpensForRedaction>::Document>;

    fn open_for_redaction(
        &self,
        bytes: &[u8],
        options: &OpenOptions<'_>,
    ) -> Result<(Self::Document, Deadline)> {
        let (inner, deadline) = Qpdf.open_for_redaction(bytes, options)?;
        let lie = (!self.opened.replace(true)).then_some(self.lie);
        Ok((
            Lying {
                inner,
                lie,
                resolved_asked: Cell::new(0),
                repaired_asked: Cell::new(0),
            },
            deadline,
        ))
    }

    fn renderer_page_size(
        &self,
        bytes: &[u8],
        page: usize,
        options: &OpenOptions<'_>,
    ) -> Result<Option<(f64, f64)>> {
        Qpdf.renderer_page_size(bytes, page, options)
    }
}

/// Flatten page 0 of `bytes` through an engine that lies once, as `lie` says.
fn flatten_lying(bytes: &[u8], lie: (Reading, Says)) -> Result<Vec<u8>> {
    let (wide, high, rgb) = pixels_for_page(bytes, 0);
    let picture = Picture::new(wide, high, &rgb);
    let options = options(Limits::DEFAULT);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    let engine = LyingQpdf {
        lie,
        opened: Cell::new(false),
    };
    super::flatten(
        &engine,
        bytes,
        0,
        &picture,
        &options,
        &deadline,
        |document| Qpdf.page_frames(document, &options),
    )
}

/// The fake engine's half of §1's row: a document whose flag reads clear after a forced
/// resolution that returned Ok is refused at whichever point it reads clear -- at the open as
/// burrow's failure, and after the constructors or the write as a repair; and a document that
/// reports a repair at any of the three points is refused there. On the well-formed twin, where
/// the real qpdf reads set and reports nothing, so that only the lie stands between it and a
/// returned document. KILLS: deleting any one of the three flag readings, and either repair
/// re-check (neither of which a real input can reach once the open has resolved the table).
#[test]
fn a_lie_at_any_of_the_three_points_is_refused_there() {
    let twin = resolution_fixture(Repair::OffsetInside, false);
    let mut examined = 0;
    for point in POINTS {
        for says in [Says::FlagClear, Says::Repaired] {
            let error = flatten_lying(&twin, (point, says))
                .expect_err("a lying engine must not return a document");
            let expected = if says == Says::FlagClear && point == Reading::Open {
                "forced resolution did not run"
            } else {
                "[engine-repaired-input]"
            };
            assert!(
                error.to_string().contains(expected),
                "{point:?}/{says:?}: expected {expected:?}, got {error}"
            );
            examined += 1;
        }
    }
    assert_eq!(examined, 6, "three points, two lies each");
    // A LIE ON THE READ-BACK'S OPEN ONLY is no lie the flatten asks: the honest twin.
    flatten(&twin, 0).expect("the honest engine flattens the twin");
}

/// S1 round 2, L-7: the read-back's measured-memory check is there, and so is the write's. A
/// resident-set reading made as large as it can be at one point only is refused there with
/// `max_memory_bytes`; the twin, with no reading moved, flattens. KILLS: deleting the read-back's
/// `check_measured_memory` (and the write's).
#[test]
fn each_measured_memory_check_is_there() {
    assert!(
        crate::rss::resident_bytes().is_some(),
        "this test needs a readable resident set"
    );
    let bytes = document(&[LETTER], "");
    for point in [Reading::Write, Reading::ReadBack] {
        hooks::MEMORY_GROWS_AT.with(|at| at.set(Some(point)));
        let error = flatten(&bytes, 0).expect_err("a measured overrun must not come back");
        assert!(
            matches!(
                error,
                Error::LimitExceeded {
                    limit: "max_memory_bytes",
                    ..
                }
            ),
            "{point:?}: {error:?}"
        );
        assert_eq!(
            hooks::MEMORY_GROWS_AT.with(Cell::get),
            None,
            "{point:?}: the reading was taken"
        );
    }
    flatten(&bytes, 0).expect("the twin, with no reading moved, flattens");
}

/// A clock that moves a millisecond every time it is read.
struct Counting(std::sync::atomic::AtomicU64);
impl Clock for Counting {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

/// Flatten page 1 of the nested tree on a counting clock with `budget` ms, and how often the
/// clock was read.
fn flatten_counting(budget: u64) -> (Result<Vec<u8>>, u64) {
    let bytes = nested_tree();
    let (wide, high, rgb) = pixels_for_page(&bytes, 1);
    let clock = Arc::new(Counting(std::sync::atomic::AtomicU64::new(0)));
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = budget;
    let options = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    let deadline = Deadline::start(clock.as_ref(), &options.limits);
    let result = Qpdf.flatten_page(
        &bytes,
        1,
        &Picture::new(wide, high, &rgb),
        &options,
        &deadline,
    );
    (result, clock.0.load(std::sync::atomic::Ordering::Relaxed))
}

/// S1 round 2, L-7: the operation's deadline expiring **inside the read-back's page-tree climb**
/// is the time limit, not a rejected output. Every budget from 0 up to the clock reads an
/// unhurried flatten makes is tried, on a clock that moves one millisecond per read, so the
/// expiry lands at every checkpoint in turn -- among them both of the climb's on the nested
/// tree's page 1 -- and none of them may come back as the climb's rejection. Twin: a budget
/// spent before the read-back (the smallest), which is the time limit either way; and the
/// unhurried flatten, which returns. KILLS: mapping the climb's `LimitExceeded` to
/// `OutputRejected` again.
///
/// The read-back's other calls (its frames, its open, the decodes) still map every error to a
/// rejection; that is recorded for the spec review, not changed here.
#[test]
fn a_deadline_inside_the_read_back_climb_is_the_time_limit() {
    let (unhurried, reads) = flatten_counting(u64::MAX / 2);
    unhurried.expect("the unhurried flatten returns");
    assert!(reads > 10, "the flatten reads the clock: {reads}");
    let mut limited = 0;
    for budget in 0..reads {
        match flatten_counting(budget).0 {
            Ok(_) => {}
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            }) => limited += 1,
            Err(Error::OutputRejected(what)) => assert!(
                !what.contains("its page tree cannot be read back"),
                "budget {budget}: the climb's deadline came back as a rejection"
            ),
            Err(other) => panic!("budget {budget}: {other:?}"),
        }
    }
    assert!(limited > 0, "no budget expired: the sweep examined nothing");
    assert!(
        matches!(
            flatten_counting(0).0,
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            })
        ),
        "the twin: a budget spent before the read-back"
    );
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(24))]

    /// Any page size and rotation in range flattens to its own display frame, and the other page
    /// is untouched.
    #[test]
    fn any_frame_flattens_to_itself(
        width in 1.0_f64..700.0,
        height in 1.0_f64..700.0,
        quarter in 0_i64..4,
    ) {
        let (width, height) = ((width * 100.0).round() / 100.0, (height * 100.0).round() / 100.0);
        let page = format!("/MediaBox [0 0 {width} {height}] /Rotate {}", quarter * 90);
        let input = document(&[LETTER, &page], "");
        let expected = frame(&input, 1);
        let output = flatten(&input, 1).unwrap();
        proptest::prop_assert_eq!(frame(&output, 1), expected);
        proptest::prop_assert_eq!(frame(&output, 0), (612.0, 792.0));
    }
}
