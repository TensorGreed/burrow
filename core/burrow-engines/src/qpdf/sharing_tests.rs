//! Tests for the resource-graph walk behind ADR 0029's sharing refusal.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock};

use super::sharing::{MAX_RESOURCE_DEPTH, count_form_uses};
use super::{Document, handle, open_document};
use crate::OpenOptions;

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn open(bytes: Vec<u8>) -> Document {
    open_document(bytes.into_boxed_slice(), &options())
        .expect("the fixture opens")
        .0
}

/// Build a PDF from numbered object bodies, object 1 being the catalog.
fn document(objects: &[String]) -> Vec<u8> {
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

fn stream(dictionary: &str, body: &str) -> String {
    format!(
        "<< {dictionary} /Length {} >>\nstream\n{body}endstream",
        body.len()
    )
}

fn form(body: &str) -> String {
    stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", body)
}

/// Two pages that declare **no** `/Resources`, inheriting one that names a form.
fn inheriting_document() -> Vec<u8> {
    let content = "/Fm0 Do\n";
    document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        // THE RESOURCES LIVE ON THE `/Pages` NODE, which is how a word processor writes a
        // shared letterhead. Neither page dictionary mentions the form.
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] \
         /Resources << /XObject << /Fm0 6 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_owned(),
        stream("", content),
        form("/F1 10 Tf BT 0 0 Td (SHARED) Tj ET\n"),
    ])
}

#[test]
fn a_form_reached_only_through_inherited_resources_is_counted_per_page() {
    // A PAGE THAT INHERITS ITS RESOURCES MUST NOT READ AS DRAWING NOTHING. Stopping at the
    // page dictionary counts this form ONCE instead of twice, and under-counting reads as
    // unshared -- the direction that edits in place and removes the text from the other page.
    let document = open(inheriting_document());
    let counts = count_form_uses(&document).expect("walks");
    assert_eq!(
        counts.uses((6, 0)),
        2,
        "the form is drawn by both pages through the inherited dictionary; counts were {:?}",
        counts.all()
    );
}

#[test]
fn a_page_with_its_own_resources_does_not_also_inherit() {
    // The climb stops at the first `/Resources` it finds, so a page that declares its own is
    // counted once for that one and not also for the ancestor's.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] \
         /Resources << /XObject << /Fm0 5 0 R >> >> >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmOwn 6 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "/FmOwn Do\n"),
        form("0 0 1 1 re f\n"),
        form("0 0 2 2 re f\n"),
    ]);
    let document = open(bytes);
    let counts = count_form_uses(&document).expect("walks");
    assert_eq!(counts.uses((6, 0)), 1, "the page's own form");
    assert_eq!(
        counts.uses((5, 0)),
        0,
        "the ancestor's form is shadowed, not additionally counted"
    );
}

#[test]
fn an_annotation_appearance_is_counted_as_a_use() {
    // An appearance stream is reached through `/Annots`, not `/Resources`, and is a form in
    // its own right. This is the use a page-resources-only scan misses.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 5 0 R >> >> /Contents 4 0 R /Annots [6 0 R] >>"
            .to_owned(),
        stream("", "/Fm0 Do\n"),
        form("/F1 10 Tf BT 0 0 Td (SHARED) Tj ET\n"),
        "<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /AP << /N 5 0 R >> >>".to_owned(),
    ]);
    let document = open(bytes);
    let counts = count_form_uses(&document).expect("walks");
    assert_eq!(
        counts.uses((5, 0)),
        2,
        "the page draws it and the annotation's appearance is it; counts were {:?}",
        counts.all()
    );
}

#[test]
fn a_form_reached_only_through_a_type_three_font_is_counted() {
    // A Type 3 glyph procedure draws like any other stream, and the font's own `/Resources` is
    // what it draws against. A walk that stopped at `/XObject` would miss a form reached only
    // this way and report it unshared.
    //
    // Found by mutation: deleting the Type 3 descent broke nothing, because no fixture reached
    // a form through a font. A branch nothing exercises is a branch nothing checks.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        // The page draws the form directly AND selects the Type 3 font that also reaches it.
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 6 0 R >> /Font << /T3 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "/Fm0 Do BT /T3 12 Tf (a) Tj ET\n"),
        // 5: the Type 3 font, whose own resources name the same form.
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
         /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 7 0 R >> \
         /Resources << /XObject << /Fm0 6 0 R >> >> >>"
            .to_owned(),
        form("0 0 1 1 re f\n"),                // 6
        stream("", "10 0 d0\n0 0 1 1 re f\n"), // 7: the glyph procedure
    ]);
    let document = open(bytes);
    let counts = count_form_uses(&document).expect("walks");
    assert_eq!(
        counts.uses((6, 0)),
        2,
        "once from the page and once from the Type 3 font's resources; counts were {:?}",
        counts.all()
    );
}

#[test]
fn a_form_whose_resources_reach_itself_is_refused() {
    // A CYCLE IS A REFUSAL, not a stop. A walk that stopped would report a count that is too
    // low, which is the direction that edits in place.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "/Fm0 Do\n"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /XObject << /Self 5 0 R >> >>",
            "/Self Do\n",
        ),
    ]);
    let document = open(bytes);
    let error = count_form_uses(&document).expect_err("a self-referencing form must be refused");
    assert_named(&error, "resource-graph-cycle");
}

#[test]
fn two_forms_that_reach_each_other_are_refused() {
    // Keyed on the OPEN PATH, not on everything seen -- a form drawn from two pages is
    // sharing, which is the thing being measured, and confusing the two would refuse every
    // document this walk exists for. A mutual pair is the case that distinguishes them.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmA 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "/FmA Do\n"),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /XObject << /FmB 6 0 R >> >>",
            "/FmB Do\n",
        ),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /XObject << /FmA 5 0 R >> >>",
            "/FmA Do\n",
        ),
    ]);
    let document = open(bytes);
    let error = count_form_uses(&document).expect_err("a mutual pair must be refused");
    assert_named(&error, "resource-graph-cycle");
}

#[test]
fn a_resource_graph_nested_past_the_cap_is_refused() {
    // Acyclic, so the cycle check sees nothing: this is the depth cap's own case, and a test
    // that accepted either refusal would pass with the cap deleted.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "/Fm0 Do\n"),
    ];
    let levels = MAX_RESOURCE_DEPTH + 4;
    for level in 0..levels {
        let next = 6 + level;
        objects.push(stream(
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
                 /Resources << /XObject << /Fm{} {next} 0 R >> >>",
                level + 1
            ),
            "0 0 1 1 re f\n",
        ));
    }
    objects.push(form("0 0 1 1 re f\n"));
    let document = open(document(&objects));
    let error = count_form_uses(&document).expect_err("an over-deep graph must be refused");
    assert_named(&error, "resource-graph-depth");
}

#[test]
fn the_live_handle_count_returns_to_its_baseline() {
    // THE CACHE ONLY GROWS. `handle.rs` explains why a per-page walk is exactly the shape that
    // fills it, and `rotate` already measures this for its ancestor climb. A walk that leaked
    // one handle per page would be invisible on a two-page fixture and fatal on a real one, so
    // the document here is large enough for a leak to show as a number rather than as noise.
    let pages = 200;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        String::new(), // filled in below, once the kid ids are known
        stream("", "/Fm0 Do\n"),
        form("/F1 10 Tf BT 0 0 Td (SHARED) Tj ET\n"),
    ];
    let first_page = 5;
    let mut kids = Vec::new();
    for index in 0..pages {
        let page_id = first_page + index * 2;
        let annot_id = page_id + 1;
        kids.push(format!("{page_id} 0 R"));
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /XObject << /Fm0 4 0 R >> >> /Contents 3 0 R /Annots [{annot_id} 0 R] >>"
        ));
        objects.push(
            "<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /AP << /N 4 0 R >> >>".to_owned(),
        );
    }
    objects[1] = format!(
        "<< /Type /Pages /Count {pages} /Kids [{}] >>",
        kids.join(" ")
    );
    let document = open(document(&objects));

    let baseline = handle::live();
    let counts = count_form_uses(&document).expect("walks");
    let after = handle::live();

    // NON-VACUITY: the walk must actually have done the work whose handles are being counted.
    assert_eq!(
        counts.uses((4, 0)),
        usize::try_from(pages * 2).expect("fits"),
        "every page draws the form and every annotation is it"
    );
    assert_eq!(
        after,
        baseline,
        "the walk leaked {} handle(s) over {pages} pages",
        after.saturating_sub(baseline)
    );
}

#[track_caller]
fn assert_named(error: &Error, rule: &str) {
    let message = match error {
        Error::Unsupported(message) | Error::Malformed(message) => message,
        other => panic!("expected a refusal naming `{rule}`, got {other:?}"),
    };
    assert!(
        message.contains(&format!("[{rule}]")),
        "refused, but by a different rule: wanted `{rule}`, got `{message}`"
    );
}
