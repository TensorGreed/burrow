//! Tests for the resource-graph walk behind ADR 0029's sharing refusal.

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::sharing::{FormUseCounts, MAX_RESOURCE_DEPTH, count_form_uses};
use super::{Document, handle, open_document};
use crate::OpenOptions;

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn open(bytes: Vec<u8>) -> (Document, Deadline, Arc<dyn Clock>) {
    let (document, _, _, deadline) =
        open_document(bytes.into_boxed_slice(), &options()).expect("the fixture opens");
    (document, deadline, Arc::new(ManualClock::new(0)))
}

/// `count_form_uses` against a clock that never advances, for the fixtures that are not about
/// the deadline.
fn count(opened: &(Document, Deadline, Arc<dyn Clock>)) -> Result<FormUseCounts> {
    count_form_uses(&opened.0, &opened.1, &opened.2)
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
    let opened = open(inheriting_document());
    let counts = count(&opened).expect("walks");
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
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
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
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
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
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
    assert_eq!(
        counts.uses((6, 0)),
        2,
        "once from the page and once from the Type 3 font's resources; counts were {:?}",
        counts.all()
    );
}

#[test]
fn a_form_nested_inside_a_shared_form_is_counted_as_shared() {
    // THE COUNT IS TRANSITIVE, and a review found out why that matters. `FmB` is referenced
    // ONCE, by `FmA`, which two pages draw -- so editing `FmB` changes both pages.
    //
    // The first version counted references and descended each subtree once: right for cost,
    // wrong for multiplicity. Measured then: `FmA` 2, `FmB` 1. `FmB` read as unshared, took
    // the sanctioned in-place edit path, and removing its text would have removed it from a
    // page nobody selected -- which §6's read-back cannot see, because it asks about the page
    // it was given and that page is clean.
    //
    // None of the other seven fixtures is two levels deep, which is why they all passed with
    // the defect present.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmA 5 0 R >> >> /Contents 7 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmA 5 0 R >> >> /Contents 7 0 R >>"
            .to_owned(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /XObject << /FmB 6 0 R >> >>",
            "/FmB Do\n",
        ),
        form("/F1 10 Tf BT 0 0 Td (SECRET) Tj ET\n"),
        stream("", "/FmA Do\n"),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
    assert_eq!(
        counts.uses((5, 0)),
        2,
        "the outer form is drawn by both pages"
    );
    assert_eq!(
        counts.uses((6, 0)),
        2,
        "and the inner form is reached through it from both, so editing it changes both; \
         counts were {:?}",
        counts.all()
    );
}

#[test]
fn a_form_nested_inside_an_unshared_form_stays_unshared() {
    // THE NEAR-MISS. Propagating multiplicity must not turn every nested form into a shared
    // one -- that would refuse the ordinary single-use case and the rule would fire on
    // documents it exists to allow.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmA 5 0 R >> >> /Contents 7 0 R >>"
            .to_owned(),
        "<< /Type /Pages /Count 0 /Kids [] >>".to_owned(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /XObject << /FmB 6 0 R >> >>",
            "/FmB Do\n",
        ),
        form("/F1 10 Tf BT 0 0 Td (SECRET) Tj ET\n"),
        stream("", "/FmA Do\n"),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
    assert_eq!(counts.uses((5, 0)), 1);
    assert_eq!(
        counts.uses((6, 0)),
        1,
        "one page, one reference each -- nothing here is shared: {:?}",
        counts.all()
    );
}

#[test]
fn a_form_drawn_twice_by_one_container_counts_twice() {
    // Multiplicity within a single container, not just across pages: a form drawn twice by the
    // same form is drawn twice, and editing it changes both places.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /FmA 5 0 R /FmAlias 5 0 R >> >> /Contents 6 0 R >>"
            .to_owned(),
        "<< /Type /Pages /Count 0 /Kids [] >>".to_owned(),
        form("/F1 10 Tf BT 0 0 Td (SECRET) Tj ET\n"),
        stream("", "/FmA Do /FmAlias Do\n"),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
    assert_eq!(
        counts.uses((5, 0)),
        2,
        "TWO NAMES, ONE OBJECT -- counted by identity, not by name: {:?}",
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
    let opened = open(bytes);
    let error = count(&opened).expect_err("a self-referencing form must be refused");
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
    let opened = open(bytes);
    let error = count(&opened).expect_err("a mutual pair must be refused");
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
    let opened = open(document(&objects));
    let error = count(&opened).expect_err("an over-deep graph must be refused");
    assert_named(&error, "resource-graph-depth");
}

#[test]
fn a_branching_type_three_ladder_is_bounded_rather_than_exponential() {
    // DEPTH BOUNDS NOTHING ON ITS OWN. Each rung's `/Resources` names TWO Type 3 fonts on the
    // next rung, so the paths are `2^depth` while the path length stays under the cap. A
    // review measured 10.12 s at depth 22 from 6 kB of file, `4x` per two rungs, extrapolating
    // to about 2.9 hours from 8.6 kB -- returning `Ok`.
    //
    // The committed depth fixture is LINEAR and refuses, which is why it never saw this: a
    // cyclic or over-deep ladder refuses in microseconds, so the attack needs a terminating
    // branching one. Bounded now by a memo per font object and by a total-work ceiling.
    const RUNGS: u64 = 24;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /T0 5 0 R /T0b 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        stream("", "BT /T0 12 Tf (a) Tj ET\n"),
    ];
    for rung in 0..RUNGS {
        let next = 6 + rung;
        objects.push(format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << >> \
             /Resources << /Font << /A {next} 0 R /B {next} 0 R >> >> >>"
        ));
    }
    objects.push(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] \
         /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << >> >>"
            .to_owned(),
    );

    let opened = open(document(&objects));
    let started = std::time::Instant::now();
    // Either answer is acceptable -- a bounded count or a refusal. What is not acceptable is
    // taking exponential time to produce one, so the assertion is the clock.
    let _ = count(&opened);
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "a branching Type 3 ladder took {elapsed:?}; the memo or the work ceiling is not holding"
    );
    // DEFENCE IN DEPTH, and stated because a single mutation does not fail this test. The font
    // memo and `MAX_RESOURCE_DICTIONARIES` each bound this on their own, so removing either
    // leaves the other and the fixture stays green. Removing BOTH was measured at **103 s** on
    // this 24-rung ladder, so the fixture does reproduce the attack -- it just cannot tell the
    // two defences apart, which is what having two of them means.
}

/// This process's peak resident set, in kilobytes, from `/proc`.
///
/// The amplification below is in **retained allocations**, not in time — a review measured
/// 265 MB against 401 ms — so a clock assertion cannot see it. A first version of this test
/// used one, and the mutation that removes the defence passed it in 0.27 s.
fn peak_rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .expect("VmHWM is reported on Linux")
}

#[test]
fn an_annots_array_of_non_dictionaries_does_not_retain_a_warning_each() {
    // `qpdf_oh_get_key` on a NON-DICTIONARY reaches `QPDFObjectHandle::typeWarning` ->
    // `Common::warn`, which appends to qpdf's warning vector whatever `suppress_warnings` says
    // -- that flag only stops the printing -- and burrow sets no `max_warnings`. A review
    // measured an `/Annots` of 400,000 integers peaking at **265 MB** from 800 kB of file:
    // about 330x, linear in the array, inside the engine thread.
    //
    // The walk now checks the container's type before asking for a key.
    const ITEMS: usize = 400_000;
    let annots: String = (0..ITEMS)
        .map(|n| format!("{n} "))
        .collect::<Vec<_>>()
        .join("");
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << >> /Contents 4 0 R /Annots [{}] >>",
            annots.trim_end()
        ),
        stream("", "BT ET\n"),
    ]);
    let opened = open(bytes);

    // The mark is taken AFTER opening, so the document's own footprint is not counted against
    // the walk. `VmHWM` is a high-water mark and never falls, which is the property that makes
    // this readable at all.
    let before = peak_rss_kb();
    let counts = count(&opened).expect("an array of integers draws no forms");
    let grew = peak_rss_kb().saturating_sub(before);

    assert!(
        counts.all().is_empty(),
        "integers are not appearance streams: {:?}",
        counts.all()
    );
    assert!(
        grew < 64 * 1024,
        "the walk grew the peak RSS by {grew} kB over {ITEMS} non-dictionary annotations -- \
         the container type check is not holding, and each skipped check retains a qpdf warning"
    );
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
    let opened = open(document(&objects));

    let baseline = handle::live();
    let counts = count(&opened).expect("walks");
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

#[test]
fn the_walk_honours_the_deadline_per_page() {
    // `max_duration_ms` is cooperative, which means a path that never checks it cannot honour
    // it. `rotate`'s ancestor climb checks per page and this walk did not -- a review flagged
    // it before the walk had a caller, which is the cheapest time to find it.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        String::new(),
        stream("", "/Fm0 Do\n"),
        form("/F1 10 Tf BT 0 0 Td (A) Tj ET\n"),
    ];
    let mut kids = Vec::new();
    for index in 0..40u64 {
        let id = 5 + index;
        kids.push(format!("{id} 0 R"));
        objects.push(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /XObject << /Fm0 4 0 R >> >> /Contents 3 0 R >>"
                .to_owned(),
        );
    }
    objects[1] = format!("<< /Type /Pages /Count 40 /Kids [{}] >>", kids.join(" "));

    let (document, _, _, deadline) = open_document(
        document(&objects).into_boxed_slice(),
        &OpenOptions::new(
            Limits::with(|limits| limits.max_duration_ms = 1),
            Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        ),
    )
    .expect("opens");

    // A clock already past the budget, so the first checkpoint fires.
    let expired: Arc<dyn Clock> = Arc::new(ManualClock::new(10_000));
    let error = count_form_uses(&document, &deadline, &expired)
        .expect_err("a walk past its deadline must refuse");
    assert!(
        matches!(&error, Error::LimitExceeded { limit, .. } if limit.contains("max_duration_ms")),
        "refused, but not on the deadline: {error:?}"
    );

    // THE NON-VACUITY CONTROL: the same document inside its budget walks to an answer, so the
    // refusal above is the deadline and not the fixture being unwalkable.
    let fresh: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let counts = count_form_uses(&document, &deadline, &fresh).expect("inside its budget");
    assert_eq!(
        counts.uses((4, 0)),
        40,
        "forty pages each draw the form once"
    );
}
