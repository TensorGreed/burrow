//! An annotation the region removes leaves the output with everything that depends on it, on both
//! engines (#239).
//!
//! Each page is 300 x 400. The region is the band from 330 to 370; an annotation at
//! [`OVER`] is in it, one at [`MARGIN`] or [`LOWER`] is not. Every page also keeps one annotation
//! whose `/Contents` is `(WITNESS)`, so a test that finds a removed string absent has first found a
//! kept one present: absence is only believed from a search shown able to see.

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
const OVER: &str = "/Rect [20 340 120 360]";
const MARGIN: &str = "/Rect [200 50 280 90]";
const LOWER: &str = "/Rect [200 100 280 140]";
const WITNESS: &str = "<< /Type /Annot /Subtype /Text /Rect [20 10 60 30] /Contents (WITNESS) >>";

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// A document whose first page lists `first` (object numbers) and, when given, whose second lists
/// `second`. Objects 1 to 4 are the catalogue (with `catalogue` added), the page tree, the witness
/// and a font; `annotations` are objects 5 onwards; the pages follow them.
fn document(
    annotations: &[String],
    first: &[usize],
    second: Option<&[usize]>,
    catalogue: &str,
) -> Vec<u8> {
    let first_page = 5 + annotations.len();
    let kids = if second.is_some() {
        format!("{first_page} 0 R {} 0 R", first_page + 1)
    } else {
        format!("{first_page} 0 R")
    };
    let count = if second.is_some() { 2 } else { 1 };
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalogue} >>"),
        format!("<< /Type /Pages /Count {count} /Kids [{kids}] >>"),
        WITNESS.to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    objects.extend(annotations.iter().cloned());
    let page = |listed: &[usize]| {
        let refs: String = listed.iter().map(|n| format!("{n} 0 R ")).collect();
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << /F1 4 0 R \
             >> >> /Annots [3 0 R {refs}] >>"
        )
    };
    objects.push(page(first));
    if let Some(second) = second {
        objects.push(page(second));
    }
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

fn text(rect: &str, extra: &str) -> String {
    format!("<< /Type /Annot /Subtype /Text {rect} {extra} >>")
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

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

/// Redact on both engines, require the same bytes, and require every `gone` string -- each
/// present in the input -- absent from the output, beside the witness, present in it.
fn removes_on_both(bytes: &[u8], gone: &[&str], what: &str) -> Vec<u8> {
    for secret in gone {
        assert!(
            contains(bytes, secret),
            "{what}: the input must carry {secret}"
        );
    }
    let [(_, natively), (_, on_the_web)] = on_both(bytes);
    let natively = natively.unwrap_or_else(|error| panic!("{what}, natively: {error:?}"));
    let on_the_web = on_the_web.unwrap_or_else(|error| panic!("{what}, on the web: {error:?}"));
    assert_eq!(
        natively, on_the_web,
        "{what}: the two engines wrote different bytes"
    );
    assert!(
        contains(&natively, "WITNESS"),
        "{what}: the kept witness must be in the output, or absence below proves nothing"
    );
    for secret in gone {
        assert!(
            !contains(&natively, secret),
            "{what}: {secret} is still in the output"
        );
    }
    natively
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

#[test]
fn a_popup_whose_parent_is_removed_goes_with_it() {
    // THE ISSUE'S SHAPE: the Popup in the margin named its removed parent as `/Parent`, and qpdf
    // wrote the parent -- `/Contents` and all -- into the output, `Ok`.
    removes_on_both(
        &document(
            &[
                text(OVER, "/Contents (PARENTSECRET) /Popup 6 0 R"),
                format!(
                    "<< /Type /Annot /Subtype /Popup {MARGIN} /Parent 5 0 R /Contents (POPUPOWN) >>"
                ),
            ],
            &[5, 6],
            None,
            "",
        ),
        &["PARENTSECRET", "POPUPOWN"],
        "a margin Popup of a removed annotation",
    );
    // AND BY ITS `/Parent` ALONE: the removed annotation does not name the Popup back. The case
    // above reaches the Popup by both edges, so it could not show this one is read.
    removes_on_both(
        &document(
            &[
                text(OVER, "/Contents (PARENTSECRET)"),
                format!(
                    "<< /Type /Annot /Subtype /Popup {MARGIN} /Parent 5 0 R /Contents (POPUPOWN) >>"
                ),
            ],
            &[5, 6],
            None,
            "",
        ),
        &["PARENTSECRET", "POPUPOWN"],
        "a Popup that names its removed parent, unnamed by it",
    );
    // AND BY ITS `/Popup` ALONE: a Popup in the margin that does not name its parent back is still
    // the removed annotation's.
    removes_on_both(
        &document(
            &[
                text(OVER, "/Contents (PARENTSECRET) /Popup 6 0 R"),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (POPUPOWN) >>"),
            ],
            &[5, 6],
            None,
            "",
        ),
        &["PARENTSECRET", "POPUPOWN"],
        "a Popup named only by the removed annotation's /Popup",
    );
}

#[test]
fn replies_to_a_removed_annotation_go_with_it_to_any_depth() {
    removes_on_both(
        &document(
            &[
                text(OVER, "/Contents (ROOTSECRET)"),
                text(MARGIN, "/Contents (FIRSTREPLY) /IRT 5 0 R"),
                text(LOWER, "/Contents (DEEPREPLY) /IRT 6 0 R"),
            ],
            // LISTED DEEPEST FIRST, so a single pass in either direction meets a reply before the
            // annotation it replies to has been marked.
            &[7, 6, 5],
            None,
            "",
        ),
        &["ROOTSECRET", "FIRSTREPLY", "DEEPREPLY"],
        "a reply to a reply to a removed annotation",
    );
}

#[test]
fn a_kept_annotation_loses_a_popup_the_region_removed() {
    // THE OTHER DIRECTION: the parent in the margin is kept; its Popup over the region goes, and
    // the parent's `/Popup` with it, or the Popup would be written out again.
    let output = removes_on_both(
        &document(
            &[
                text(MARGIN, "/Contents (KEPTPARENT) /Popup 6 0 R"),
                format!(
                    "<< /Type /Annot /Subtype /Popup {OVER} /Parent 5 0 R /Contents (POPUPSECRET) >>"
                ),
            ],
            &[5, 6],
            None,
            "",
        ),
        &["POPUPSECRET"],
        "a kept annotation whose Popup is over the region",
    );
    assert!(contains(&output, "KEPTPARENT"), "the parent is kept");
    assert!(!contains(&output, "/Popup"), "and names no popup");
}

#[test]
fn a_removed_annotation_or_its_popup_named_by_anything_kept_is_refused() {
    // A DEPENDENT KEPT FOR AN INDEPENDENT REASON, wherever the name is (#239's reviews). The first
    // version emptied the annotation in place instead, and emptied whatever else it was.
    let rule = "annotation-dependent-kept";
    // From the catalogue: an action naming the annotation.
    refused_on_both(
        &document(
            &[text(OVER, "/Contents (HIDDENSECRET)")],
            &[5],
            None,
            "/OpenAction << /S /Hide /T 5 0 R /H false >>",
        ),
        rule,
        "a removed annotation a /Hide action names",
    );
    // A Popup the page does not list, which the catalogue reaches; and one a kept annotation on
    // another page names as its own.
    let popup = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (POPUPSECRET) >>");
    refused_on_both(
        &document(
            &[text(OVER, "/Popup 6 0 R"), popup.clone()],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        rule,
        "an unlisted Popup the catalogue reaches",
    );
    refused_on_both(
        &document(
            &[
                text(OVER, "/Popup 6 0 R"),
                popup,
                text(MARGIN, "/Popup 6 0 R"),
            ],
            &[5],
            Some(&[7]),
            "",
        ),
        rule,
        "an unlisted Popup another page's annotation names",
    );
    // A reply no page lists, which the catalogue reaches.
    refused_on_both(
        &document(
            &[text(OVER, "/Contents (ROOT)"), text(MARGIN, "/IRT 5 0 R")],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        rule,
        "an unlisted reply the catalogue reaches",
    );
}

#[test]
fn an_object_that_is_also_something_else_is_refused_not_emptied() {
    // #239's REVIEWS: emptying a removed annotation emptied whatever else it was -- the page's
    // graphics state, another page's font, the trailer's `/Info` -- `Ok`, without a word.
    let rule = "annotation-dependent-kept";
    let annotation_and_state =
        format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (SECRET) /ca 0.15 >>");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [5 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        annotation_and_state,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /ExtGState << /G 4 0 R \
         >> >> /Annots [3 0 R 4 0 R] >>"
            .to_owned(),
    ];
    refused_on_both(
        &raw(&objects, ""),
        rule,
        "a removed annotation that is the page's graphics state",
    );
    // AND AS THE TRAILER'S `/Info`, which no object holds.
    objects[4] =
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 4 0 R] >>"
            .to_owned();
    refused_on_both(
        &raw(&objects, "/Info 4 0 R"),
        rule,
        "a removed annotation that is the trailer's /Info",
    );
    // AND FROM A STREAM'S DICTIONARY, and from an array that is its own object: another page whose
    // `/Annots` is written indirectly.
    objects[4] =
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Contents 6 0 R \
                  /Annots [3 0 R 4 0 R] >>"
            .to_owned();
    objects.push("<< /Extra 4 0 R /Length 0 >>\nstream\n\nendstream".to_owned());
    refused_on_both(
        &raw(&objects, ""),
        rule,
        "a removed annotation a content stream's dictionary names",
    );
    objects[1] = "<< /Type /Pages /Count 2 /Kids [5 0 R 8 0 R] >>".to_owned();
    objects[4] =
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 4 0 R] >>"
            .to_owned();
    objects[5] = "null".to_owned();
    objects.push("[4 0 R]".to_owned());
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 7 0 R >>"
            .to_owned(),
    );
    refused_on_both(
        &raw(&objects, ""),
        rule,
        "a removed annotation another page lists in an indirect array",
    );
}

#[test]
fn a_kept_annotation_another_page_lists_does_not_lose_its_popup_silently() {
    // #239's SPECIFICATION REVIEW: K is listed on both pages, its Popup is over the region on this
    // one. Dropping K's `/Popup` would change page 2 too, so it is refused.
    refused_on_both(
        &document(
            &[
                text(MARGIN, "/Contents (KEPT) /Popup 6 0 R"),
                format!("<< /Type /Annot /Subtype /Popup {OVER} /Parent 5 0 R >>"),
            ],
            &[5, 6],
            Some(&[5]),
            "",
        ),
        "annotation-dependent-kept",
        "a kept annotation listed on two pages, whose Popup this one removes",
    );
}

#[test]
fn a_direct_annotation_has_no_identity_and_no_dependents() {
    // THE (0, 0) GUARD (#239's code review): qpdf reports an absent key as (0, 0) too, so without
    // it every annotation with no `/Parent` was a dependent of a removed direct one. A direct
    // annotation over the region, and the indirect witness, which must be kept.
    let bytes = raw(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [4 0 R 5 0 R] >>".to_owned(),
            WITNESS.to_owned(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R \
                 << /Type /Annot /Subtype /Text {OVER} /Contents (DIRECTSECRET) >>] >>"
            ),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [{}] >>",
                text(MARGIN, "/Contents (PAGETWO)")
            ),
        ],
        "",
    );
    let output = removes_on_both(
        &bytes,
        &["DIRECTSECRET"],
        "a direct annotation over the region",
    );
    assert!(
        contains(&output, "PAGETWO"),
        "page 2's annotation is untouched"
    );
}

/// A document of `objects`, numbered from 1, with `trailer` added to its trailer.
fn raw(objects: &[String], trailer: &str) -> Vec<u8> {
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
        "trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

#[test]
fn a_removed_annotation_kept_by_another_page_is_refused() {
    // A DEPENDENT KEPT FOR AN INDEPENDENT REASON: the other page is not being redacted.
    for (key, what) in [
        ("/IRT", "a reply on another page"),
        ("/Parent", "a Popup on another page"),
        (
            "/Popup",
            "an annotation on another page naming it as its Popup",
        ),
    ] {
        refused_on_both(
            &document(
                &[
                    text(OVER, "/Contents (SECRET)"),
                    text(MARGIN, &format!("{key} 5 0 R")),
                ],
                &[5],
                Some(&[6]),
                "",
            ),
            "annotation-dependent-kept",
            what,
        );
    }
    refused_on_both(
        &document(&[text(OVER, "/Contents (SECRET)")], &[5], Some(&[5]), ""),
        "annotation-dependent-kept",
        "the same annotation listed on another page",
    );
    // THE NEAR-MISS: another page's annotations, naming nothing removed, are none of its business.
    removes_on_both(
        &document(
            &[text(OVER, "/Contents (SECRET)"), text(MARGIN, "/IRT 3 0 R")],
            &[5],
            Some(&[6]),
            "",
        ),
        &["SECRET"],
        "another page whose reply names the kept witness",
    );
}

/// Two pages naming one `/Annots` array, object 5, which holds `entries`; `catalogue` and
/// `extra` (objects from 8) are added for other referrers.
fn sharing(entries: &str, catalogue: &str, extra: &[String]) -> Vec<u8> {
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalogue} >>"),
        "<< /Type /Pages /Count 2 /Kids [6 0 R 7 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        text(OVER, "/Contents (SHAREDSECRET)"),
        format!("[{entries}]"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
            .to_owned(),
    ];
    objects.extend(extra.iter().cloned());
    raw(&objects, "")
}

#[test]
fn an_annots_array_another_page_shares_is_refused_not_edited() {
    // #240: pages 1 and 2 name one `/Annots` array. Erasing from it took the annotation off page 2
    // too -- `Ok`, a silent edit to a page nobody asked about. A per-page copy was the first
    // direction, and measurement disproved it: every entry of a shared array is the same object on
    // both pages, so a copy left the removed annotation on page 2 and in the bytes. Refused.
    let rule = "annotation-dependent-kept";
    refused_on_both(
        &sharing("3 0 R 4 0 R", "", &[]),
        rule,
        "an indirect annotation in a shared array",
    );
    let direct = format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (DIRECTSECRET) >>");
    refused_on_both(
        &sharing(&format!("3 0 R {direct}"), "", &[]),
        rule,
        "a direct annotation in a shared array",
    );
}

#[test]
fn an_annots_array_something_other_than_a_page_names_is_refused() {
    // NOT ONLY ANOTHER PAGE (owner, 2026-10-02): whatever else names the array is changed by the
    // edit too. Page 2 here has an array of its own; the catalogue, or an annotation, names page
    // 1's.
    let rule = "annotation-dependent-kept";
    let page_one_only = |catalogue: &str, extra: &[String]| {
        let mut objects = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {catalogue} >>"),
            "<< /Type /Pages /Count 2 /Kids [6 0 R 7 0 R] >>".to_owned(),
            WITNESS.to_owned(),
            text(OVER, "/Contents (SHAREDSECRET)"),
            "[3 0 R 4 0 R]".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
                .to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R] >>"
                .to_owned(),
        ];
        objects.extend(extra.iter().cloned());
        raw(&objects, "")
    };
    refused_on_both(
        &page_one_only("/Extra 5 0 R", &[]),
        rule,
        "the catalogue naming page 1's array",
    );
    refused_on_both(
        &page_one_only("/Extra 8 0 R", &[text(MARGIN, "/IRT 5 0 R")]),
        rule,
        "an annotation naming page 1's array",
    );
    // AND THE TRAILER, which no object holds.
    refused_on_both(
        &raw_with_trailer_naming_annots(),
        rule,
        "the trailer naming page 1's array",
    );
    // THE NEAR-MISS: the same page 1, its array named by nothing else.
    removes_on_both(
        &page_one_only("", &[]),
        &["SHAREDSECRET"],
        "page 1's own array, named by it alone",
    );
}

#[test]
fn a_shared_array_with_nothing_over_the_region_is_left_exactly_as_it_was() {
    // THE TWIN: nothing on page 1 meets the region, so nothing is erased and nothing refuses; both
    // pages still name one array holding both annotations, read back from the output.
    let kept = text("/Rect [20 100 120 120]", "/Contents (KEPTSHARED)");
    let bytes = sharing("3 0 R 4 0 R", "", &[]);
    let bytes = String::from_utf8(bytes).expect("ASCII");
    // THE SAME FILE with the annotation moved off the region: rewritten through `raw`, so the
    // offsets stay right.
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [6 0 R 7 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        kept,
        "[3 0 R 4 0 R]".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
            .to_owned(),
    ];
    assert!(
        bytes.contains("SHAREDSECRET"),
        "the shared fixture is the over-the-region one"
    );
    let [(_, natively), (_, on_the_web)] = on_both(&raw(&objects, ""));
    let natively = natively.expect("nothing over the region: redacts");
    assert_eq!(
        natively,
        on_the_web.expect("and on the web"),
        "the two engines agree"
    );
    let (document, _) = super::Qpdf
        .open_for_redaction(&natively, &options())
        .expect("the output opens");
    use crate::redact::graph::{OpensForRedaction, PdfDocument};
    let annots = crate::name::Name::literal(b"/Annots\0");
    let first = document.page(0).expect("page 1").key(&annots);
    let second = document.page(1).expect("page 2").key(&annots);
    assert_eq!(
        first.object().expect("identity"),
        second.object().expect("identity"),
        "both pages still name one array"
    );
    assert_ne!(
        first.object().expect("identity"),
        (0, 0),
        "and it is still its own object"
    );
    assert_eq!(first.array_len(), 2, "holding both annotations");
    assert!(
        contains(&natively, "KEPTSHARED"),
        "the kept annotation is in the output"
    );
}

#[test]
fn the_reference_walk_reads_the_deadline_at_each_annotation_object() {
    // MEASURED AS A DIFFERENCE, because the sharing walk reads the deadline at every annotation of
    // every page too and would hide these reads end to end. The walk over referenced objects runs
    // only when something with an identity was removed, so the same document with the annotation
    // moved out of the region is the control: the two counts differ by at least page 2's 200
    // annotations, each an object the walk reads.
    struct Counting(std::sync::atomic::AtomicU64);
    impl Clock for Counting {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let others = 200;
    let reads = |rect: &str| {
        let mut annotations = vec![text(rect, "/Contents (SECRET)")];
        annotations.extend((0..others).map(|_| text(MARGIN, "")));
        let second: Vec<usize> = (6..6 + others).collect();
        let bytes = document(&annotations, &[5], Some(&second), "");
        let clock = Arc::new(Counting(std::sync::atomic::AtomicU64::new(0)));
        let options = OpenOptions::new(Limits::default(), clock.clone() as Arc<dyn Clock>);
        super::Qpdf
            .redact_page(&bytes, 0, &BTreeSet::from([0]), UPPER_BAND, &options)
            .expect("redacts");
        clock.0.load(std::sync::atomic::Ordering::Relaxed)
    };
    let removing = reads(OVER);
    let keeping = reads(MARGIN);
    assert!(
        removing >= keeping + others as u64,
        "{removing} deadline reads removing one annotation against {keeping} keeping it: the walk \
         over page 2's {others} annotations must read it at each"
    );
}

#[test]
fn the_reference_walk_reads_the_deadline_at_each_page_object() {
    // THE SAME DIFFERENCE over 200 pages with no annotations at all, so only the per-page read can
    // account for it.
    struct Counting(std::sync::atomic::AtomicU64);
    impl Clock for Counting {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let others = 200;
    let reads = |rect: &str| {
        let first = 4 + others;
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Count {} /Kids [{}] >>",
                others + 1,
                (0..=others)
                    .map(|i| format!("{} 0 R ", first + i))
                    .collect::<String>()
            ),
            text(rect, "/Contents (SECRET)"),
        ];
        objects.extend((0..others).map(|_| "null".to_owned()));
        objects.push(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R] >>"
                .to_owned(),
        );
        objects.extend((0..others).map(|_| {
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>".to_owned()
        }));
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
        let clock = Arc::new(Counting(std::sync::atomic::AtomicU64::new(0)));
        let options = OpenOptions::new(Limits::default(), clock.clone() as Arc<dyn Clock>);
        super::Qpdf
            .redact_page(
                out.as_bytes(),
                0,
                &BTreeSet::from([0]),
                UPPER_BAND,
                &options,
            )
            .expect("redacts");
        clock.0.load(std::sync::atomic::Ordering::Relaxed)
    };
    let removing = reads(OVER);
    let keeping = reads(MARGIN);
    assert!(
        removing >= keeping + u64::try_from(others).expect("a count"),
        "{removing} deadline reads removing one annotation against {keeping} keeping it: the walk \
         over {others} other pages must read it at each"
    );
}

#[test]
fn what_something_kept_also_names_is_kept_with_it() {
    // THE RESIDUAL, PINNED (#239): only the annotations and their Popups must be named by nothing
    // kept. What a removed one reaches that something kept names as well -- an appearance another
    // annotation draws with, a string the catalogue names -- is that thing's, and stays. Following
    // every key instead refused 49 of 107 regions over the real documents' annotations, measured.
    let stream =
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Length 4 >>\nstream\n0 g \nendstream";
    let output = removes_on_both(
        &document(
            &[
                text(OVER, "/NM (REMOVEDNAME) /AP << /N 6 0 R >>"),
                stream.to_owned(),
                text(MARGIN, "/NM (KEPTNAME) /AP << /N 6 0 R >>"),
            ],
            &[5, 7],
            None,
            "",
        ),
        &["REMOVEDNAME"],
        "an appearance a kept annotation shares",
    );
    assert!(contains(&output, "KEPTNAME"), "the kept annotation stays");
    assert!(
        contains(&output, "/Subtype /Form"),
        "and so does the appearance it shares with the removed one"
    );
    let output = removes_on_both(
        &document(
            &[
                text(OVER, "/NM (REMOVEDNAME) /Contents 6 0 R"),
                "(SHAREDTEXT)".to_owned(),
            ],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        &["REMOVEDNAME"],
        "an indirect /Contents the catalogue also names",
    );
    assert!(
        contains(&output, "SHAREDTEXT"),
        "the catalogue's own string is the catalogue's, and stays"
    );
}

#[test]
fn the_popup_walk_reads_the_deadline_at_each_popup() {
    // A CHAIN OF UNLISTED POPUPS, each naming the next as `/Popup`: bounded by the file, not by the
    // page. Measured as a difference, against the same removal with no chain: each popup is read
    // once by the walk that collects them and once by the walk over every referenced object, so
    // the difference is at least twice the chain.
    struct Counting(std::sync::atomic::AtomicU64);
    impl Clock for Counting {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let chain = 200_usize;
    let reads = |length: usize| {
        let mut annotations = vec![if length == 0 {
            text(OVER, "")
        } else {
            text(OVER, "/Popup 6 0 R")
        }];
        for at in 0..length {
            let next = if at + 1 < length {
                format!("/Popup {} 0 R", 7 + at)
            } else {
                String::new()
            };
            annotations.push(format!(
                "<< /Type /Annot /Subtype /Popup {MARGIN} {next} >>"
            ));
        }
        let bytes = document(&annotations, &[5], None, "");
        let clock = Arc::new(Counting(std::sync::atomic::AtomicU64::new(0)));
        let options = OpenOptions::new(Limits::default(), clock.clone() as Arc<dyn Clock>);
        super::Qpdf
            .redact_page(&bytes, 0, &BTreeSet::from([0]), UPPER_BAND, &options)
            .expect("redacts");
        clock.0.load(std::sync::atomic::Ordering::Relaxed)
    };
    let with_chain = reads(chain);
    let without = reads(0);
    assert!(
        with_chain >= without + 2 * u64::try_from(chain).expect("a count"),
        "{with_chain} deadline reads with a chain of {chain} Popups against {without} without: \
         the walk collecting them must read it at each"
    );
}

/// As [`raw`], with a cross-reference **stream** in place of a classic table, whose dictionary --
/// the trailer -- carries `trailer`. Uncompressed: `/W [1 4 2]`.
fn with_xref_stream(objects: &[String], trailer: &str) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref_number = objects.len() + 1;
    let xref_at = out.len();
    offsets.push(xref_at);
    let mut entries = vec![0_u8, 0, 0, 0, 0, 0xff, 0xff];
    for offset in &offsets {
        entries.push(1);
        entries.extend_from_slice(&u32::try_from(*offset).expect("a small file").to_be_bytes());
        entries.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(
        format!(
            "{xref_number} 0 obj\n<< /Type /XRef /Size {} /W [1 4 2] /Root 1 0 R {trailer} /Length {} >>\nstream\n",
            xref_number + 1,
            entries.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&entries);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn a_trailer_that_is_a_cross_reference_stream_is_read_too() {
    // #239's SECOND CODE REVIEW: the walk read every classic trailer and every referenced object,
    // and nothing references a cross-reference stream -- whose dictionary is the trailer since PDF
    // 1.5. A removed annotation it named as `/Info` was written out after an `Ok`, measured.
    let rule = "annotation-dependent-kept";
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [5 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (INFOSECRET) >>"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 4 0 R] >>"
            .to_owned(),
    ];
    refused_on_both(
        &with_xref_stream(&objects, "/Info 4 0 R"),
        rule,
        "a removed annotation an xref stream's trailer names as /Info",
    );
    // THE NEAR-MISS: the same file, its trailer naming nothing removed, redacts.
    removes_on_both(
        &with_xref_stream(&objects, ""),
        &["INFOSECRET"],
        "an xref-stream file whose trailer names nothing removed",
    );
    // AND THE CATALOGUE ITSELF LISTED AS AN ANNOTATION OVER THE REGION, which only `/Root` names:
    // under either kind of trailer.
    let catalogue_too = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Subtype /Text {OVER} /Contents (ROOTSECRET) >>"),
        "<< /Type /Pages /Count 1 /Kids [4 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 1 0 R] >>"
            .to_owned(),
    ];
    refused_on_both(
        &raw(&catalogue_too, ""),
        rule,
        "the catalogue, listed as an annotation, classic trailer",
    );
    refused_on_both(
        &with_xref_stream(&catalogue_too, ""),
        rule,
        "the catalogue, listed as an annotation, xref-stream trailer",
    );
}

#[test]
fn a_page_whose_annots_is_its_own_object_still_lists_its_kept_annotation() {
    // #239's SECOND CODE REVIEW: a kept annotation that loses its `/Popup` may be named by this
    // page's own listing, and an `/Annots` written as its own object is that listing too. The guard
    // that says so had no test; indirect `/Annots` arrays are ordinary.
    let output = removes_on_both(
        &raw(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [4 0 R] >>".to_owned(),
                WITNESS.to_owned(),
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
                    .to_owned(),
                "[3 0 R 6 0 R 7 0 R]".to_owned(),
                text(MARGIN, "/Contents (KEPTPARENT) /Popup 7 0 R"),
                format!("<< /Type /Annot /Subtype /Popup {OVER} /Parent 6 0 R /Contents (POPSECRET) >>"),
            ],
            "",
        ),
        &["POPSECRET"],
        "a kept annotation, listed by an indirect /Annots, whose Popup is over the region",
    );
    assert!(contains(&output, "KEPTPARENT"), "the parent is kept");
}

#[test]
fn the_popup_of_a_direct_annotation_is_its_own_too() {
    // #239's SECOND SPECIFICATION REVIEW: a removed annotation written directly in `/Annots` has
    // no identity, but the Popup it names does, and was not followed -- `Ok` with the Popup's text
    // in the bytes when anything kept named it. Each way of naming it, and the near-miss.
    let direct = format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (ROOT) /Popup 5 0 R >>");
    let popup = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (POPUPSECRET) >>");
    let page = |annots: &str| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [{annots}] >>"
        )
    };
    let build = |catalogue: &str, second: Option<&str>| {
        let kids = if second.is_some() {
            "4 0 R 6 0 R"
        } else {
            "4 0 R"
        };
        let count = if second.is_some() { 2 } else { 1 };
        let mut objects = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {catalogue} >>"),
            format!("<< /Type /Pages /Count {count} /Kids [{kids}] >>"),
            WITNESS.to_owned(),
            page(&format!("3 0 R {direct}")),
            popup.clone(),
        ];
        if let Some(second) = second {
            objects.push(page(second));
        }
        raw(&objects, "")
    };
    let rule = "annotation-dependent-kept";
    refused_on_both(
        &build("/Extra 5 0 R", None),
        rule,
        "a direct annotation's Popup the catalogue names",
    );
    refused_on_both(
        &build("", Some("5 0 R")),
        rule,
        "a direct annotation's Popup another page lists",
    );
    refused_on_both(
        &build("", Some(&text(MARGIN, "/Popup 5 0 R"))),
        rule,
        "a direct annotation's Popup another page's annotation names",
    );
    removes_on_both(
        &build("", None),
        &["ROOT", "POPUPSECRET"],
        "a direct annotation's Popup nothing else names",
    );
}

/// As [`raw`], with the classic `trailer` keyword written straight after the last cross-reference
/// subsection's count -- `1 0trailer` -- which qpdf reads without a warning.
fn with_glued_trailer(objects: &[String], trailer: &str) -> Vec<u8> {
    let classic = String::from_utf8(raw(objects, trailer)).expect("ASCII");
    let (body, tail) = classic.split_once("trailer\n").expect("a classic trailer");
    // AN EMPTY SUBSECTION, `N 0`, its count glued to the keyword.
    format!("{body}{} 0trailer\n{tail}", objects.len() + 1).into_bytes()
}

#[test]
fn a_trailer_the_bytes_hide_is_read_as_qpdf_reads_it() {
    // #239's THIRD SPECIFICATION REVIEW: trailers were found by scanning the bytes, and qpdf reads
    // `11 0trailer` -- the keyword glued to a digit -- where the scan did not. A removed annotation
    // that trailer named was written out after an `Ok`, measured. The trailer is now asked of qpdf.
    let rule = "annotation-dependent-kept";
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [5 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (GLUEDSECRET) >>"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 4 0 R] >>"
            .to_owned(),
        "<< /X 4 0 R >>".to_owned(),
    ];
    refused_on_both(
        &with_glued_trailer(&objects, "/Foo 4 0 R"),
        rule,
        "a glued trailer naming it",
    );
    refused_on_both(
        &with_glued_trailer(&objects, "/Info 4 0 R"),
        rule,
        "a glued trailer's /Info",
    );
    // AN OBJECT ONLY THAT TRAILER NAMES, which names the annotation: nothing the scan saw
    // referenced object 6, so the walk never read it.
    refused_on_both(
        &with_glued_trailer(&objects, "/Foo 6 0 R"),
        rule,
        "an object only a glued trailer names",
    );
    // THE NEAR-MISS: the same glued trailer, naming nothing removed.
    removes_on_both(
        &with_glued_trailer(&objects, ""),
        &["GLUEDSECRET"],
        "a glued trailer naming nothing",
    );
}

#[test]
fn a_popup_chain_is_followed_through_a_popup_written_inline() {
    // #239's THIRD SPECIFICATION REVIEW: the walk followed `/Popup` only from objects with an
    // identity, so a Popup written inline ended the chain, and the indirect Popup past it was
    // written out when the catalogue named it -- while the same chain written indirectly refused.
    let inline_then_seven = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Popup 6 0 R >>");
    let seven = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (CHAINSECRET) >>");
    refused_on_both(
        &document(
            &[
                text(OVER, &format!("/Popup {inline_then_seven}")),
                seven.clone(),
            ],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        "annotation-dependent-kept",
        "an indirect Popup past one written inline, which the catalogue names",
    );
    // THE NEAR-MISS: nothing else names it, so it goes with the rest.
    removes_on_both(
        &document(
            &[text(OVER, &format!("/Popup {inline_then_seven}")), seven],
            &[5],
            None,
            "",
        ),
        &["CHAINSECRET"],
        "the same chain, named by nothing else",
    );
}

#[test]
fn a_popup_named_in_a_form_the_walk_cannot_follow_refuses_only_when_removed() {
    // #239's FOURTH SPECIFICATION REVIEW: `/Popup [7 0 R]` -- an array, not a dictionary -- named
    // nothing the walk followed, and the Popup was written out after an `Ok` when the catalogue
    // named it. On a removed annotation that is now refused; on a kept one it is its own business.
    let popup = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (ARRAYSECRET) >>");
    refused_on_both(
        &document(
            &[text(OVER, "/Popup [6 0 R]"), popup.clone()],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        "annotation-popup-unreadable",
        "a removed annotation whose /Popup is an array",
    );
    // AND A REMOVED ONE WRITTEN DIRECTLY, which only the annotation pass's own check reaches: the
    // walk over what the removal owns starts from identities, and this one has none.
    let direct = format!("<< /Type /Annot /Subtype /Text {OVER} /Popup [5 0 R] >>");
    refused_on_both(
        &raw(
            &[
                "<< /Type /Catalog /Pages 2 0 R /Extra 5 0 R >>".to_owned(),
                "<< /Type /Pages /Count 1 /Kids [4 0 R] >>".to_owned(),
                WITNESS.to_owned(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R {direct}] >>"
                ),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (ARRAYSECRET) >>"),
            ],
            "",
        ),
        "annotation-popup-unreadable",
        "a removed annotation written directly, whose /Popup is an array",
    );
    let output = removes_on_both(
        &document(
            &[
                text(OVER, "/Contents (REMOVEDTEXT)"),
                text(LOWER, "/Contents (KEPTODD) /Popup [7 0 R]"),
                popup,
            ],
            &[5, 6],
            None,
            "",
        ),
        &["REMOVEDTEXT"],
        "a kept annotation whose /Popup is an array",
    );
    assert!(contains(&output, "KEPTODD"), "the kept annotation stays");
}

#[test]
fn each_place_a_popup_is_read_follows_an_inline_one() {
    // #239's FOURTH CODE REVIEW: `popup_of` is read in two places, and the chain test reached both
    // at once, so either could go back to reading only identities with every suite green -- and
    // each alone wrote a Popup out after an `Ok`. One shape per place, each with its near-miss.
    let rule = "annotation-dependent-kept";
    let six = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (SIXSECRET) >>");
    let inline_to_six = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Popup 6 0 R >>");
    // THE WALK OVER WHAT THE REMOVAL OWNS: an indirect Popup, 7, whose own `/Popup` is inline.
    let through_seven = |catalogue: &str| {
        document(
            &[
                text(OVER, "/Popup 7 0 R"),
                six.clone(),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Popup {inline_to_six} >>"),
            ],
            &[5],
            None,
            catalogue,
        )
    };
    refused_on_both(
        &through_seven("/Extra 6 0 R"),
        rule,
        "an indirect Popup whose own is inline",
    );
    removes_on_both(
        &through_seven(""),
        &["SIXSECRET"],
        "the same, named by nothing else",
    );
    // THE ANNOTATION PASS: a removed entry written directly in `/Annots`, its Popup inline.
    let direct = |catalogue: &str| {
        raw(
            &[
                format!("<< /Type /Catalog /Pages 2 0 R {catalogue} >>"),
                "<< /Type /Pages /Count 1 /Kids [4 0 R] >>".to_owned(),
                WITNESS.to_owned(),
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R \
                     << /Type /Annot /Subtype /Text {OVER} /Popup << /Popup 5 0 R >> >>] >>"
                ),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (FIVESECRET) >>"),
            ],
            "",
        )
    };
    refused_on_both(
        &direct("/Extra 5 0 R"),
        rule,
        "a direct removed entry whose Popup is inline",
    );
    removes_on_both(
        &direct(""),
        &["FIVESECRET"],
        "the same, named by nothing else",
    );
}

#[test]
fn an_inline_popup_chain_is_followed_to_the_cap_and_refused_past_it() {
    // THE CAP: up to `MAX_DIRECT_POPUPS` (32) Popups written inline past the annotation's own are
    // followed; one more cannot be, and refuses rather than ending the chain short -- which, the
    // fourth code review measured, wrote the Popup at the end out after an `Ok`.
    let chain = |links: usize| {
        let mut inline = "6 0 R".to_owned();
        for _ in 0..links {
            inline = format!("<< /Popup {inline} >>");
        }
        document(
            &[
                text(OVER, &format!("/Popup {inline}")),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (CAPSECRET) >>"),
            ],
            &[5],
            None,
            "/Extra 6 0 R",
        )
    };
    refused_on_both(
        &chain(32),
        "annotation-dependent-kept",
        "32 inline Popups, then one named elsewhere",
    );
    refused_on_both(
        &chain(33),
        "annotation-popup-unreadable",
        "33 inline Popups",
    );
}

#[test]
fn a_superseded_trailer_naming_a_removed_annotation_refuses_nothing() {
    // THE OVER-REFUSAL THAT IS GONE, pinned: the byte scan read every trailer, so an older one an
    // incremental update superseded refused although qpdf writes only the newest. Asked of qpdf,
    // the trailer is the one written.
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [5 0 R] >>".to_owned(),
        WITNESS.to_owned(),
        format!("<< /Type /Annot /Subtype /Text {OVER} /Contents (OLDTRAILERSECRET) >>"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R 4 0 R] >>"
            .to_owned(),
    ];
    let mut bytes = raw(&objects, "/Foo 4 0 R");
    let text = String::from_utf8(bytes.clone()).expect("ASCII");
    let previous = text
        .rsplit_once("startxref\n")
        .and_then(|(_, tail)| tail.split_once('\n'))
        .map(|(offset, _)| offset.to_owned())
        .expect("a startxref");
    // AN EMPTY UPDATE: a new, empty cross-reference section and a newest trailer without `/Foo`.
    let update_at = bytes.len();
    bytes.extend_from_slice(
        format!(
            "xref\n0 0\ntrailer\n<< /Size {} /Root 1 0 R /Prev {previous} >>\nstartxref\n{update_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    removes_on_both(
        &bytes,
        &["OLDTRAILERSECRET"],
        "a superseded trailer naming the removed annotation",
    );
}

#[test]
fn a_popup_further_down_the_chain_named_in_a_form_the_walk_cannot_follow_is_refused() {
    // THE WALK'S OWN CHECK, which the annotation pass's cannot reach: the removed annotation's
    // Popup, 7, is listed nowhere and names its own Popup as an array. Only the walk over what the
    // removal owns reads 7.
    refused_on_both(
        &document(
            &[
                text(OVER, "/Popup 7 0 R"),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (SIXSECRET) >>"),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Popup [6 0 R] >>"),
            ],
            &[5],
            None,
            "/Extra 6 0 R",
        ),
        "annotation-popup-unreadable",
        "an unlisted Popup whose own /Popup is an array",
    );
}

#[test]
fn a_popup_that_is_its_own_object_but_not_a_dictionary_is_refused() {
    // #239's FIFTH SPECIFICATION REVIEW: `/Popup 8 0 R` over `8 0 obj [7 0 R]` had an identity,
    // so it was owned and the walk stopped there; 7 was written out after an `Ok`. Anything the
    // walk owns must be a dictionary or nothing -- from the removed annotation, and further down.
    let rule = "annotation-popup-unreadable";
    let seven = format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Contents (SEVENSECRET) >>");
    refused_on_both(
        &document(
            &[
                text(OVER, "/Popup 6 0 R"),
                "[7 0 R]".to_owned(),
                seven.clone(),
            ],
            &[5],
            None,
            "/Extra 7 0 R",
        ),
        rule,
        "a removed annotation whose /Popup is an indirect array",
    );
    refused_on_both(
        &document(
            &[
                text(OVER, "/Popup 6 0 R"),
                format!("<< /Type /Annot /Subtype /Popup {MARGIN} /Popup 7 0 R >>"),
                "[8 0 R]".to_owned(),
                seven,
            ],
            &[5],
            None,
            "/Extra 8 0 R",
        ),
        rule,
        "an indirect array further down a removed annotation's Popup chain",
    );
}

#[test]
fn a_popup_that_is_a_number_or_a_name_on_a_removed_annotation_is_refused() {
    // WITNESSED, NOT INCIDENTAL (#239's fifth code review): a scalar cannot name an object, so this
    // is an over-refusal of a shape nobody writes -- but it is the row's stated rule, and a test
    // is what stops it changing unseen.
    for popup in ["5", "true", "/Name", "(text)"] {
        refused_on_both(
            &document(&[text(OVER, &format!("/Popup {popup}"))], &[5], None, ""),
            "annotation-popup-unreadable",
            &format!("a removed annotation whose /Popup is {popup}"),
        );
    }
}

/// Page 1's `/Annots` array, object 5, named by the trailer as well.
fn raw_with_trailer_naming_annots() -> Vec<u8> {
    raw(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 2 /Kids [6 0 R 7 0 R] >>".to_owned(),
            WITNESS.to_owned(),
            text(OVER, "/Contents (SHAREDSECRET)"),
            "[3 0 R 4 0 R]".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots 5 0 R >>"
                .to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Annots [3 0 R] >>"
                .to_owned(),
        ],
        "/Foo 5 0 R",
    )
}
