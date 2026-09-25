//! Tests for the resource-graph walk behind ADR 0029's sharing refusal.

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::{Document, handle, open_document};
use crate::OpenOptions;
use crate::redact::sharing::{FormUseCounts, MAX_RESOURCE_DEPTH, count_form_uses};

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
// PROCESS-ISOLATED, BECAUSE `VmHWM` IS PROCESS-WIDE.
//
// `peak_rss_kb` reads `/proc/self/status`, which reports the whole process, and the lib test
// binary runs its tests in parallel threads. A neighbouring test allocating between the two
// measurements below lands in exactly the number this compares. Measured: this failed once in
// a full-suite run at 191,848 kB against 156,608 kB and passed on four isolated runs, and it
// then reported a mutation as "caught" that it had nothing to do with, which is how a flake
// stops being cosmetic.
//
// The comment above argues the ordering makes the comparison "strictly conservative". What the
// ordering actually makes it is dependent on what ran in between, and the *dangerous* direction
// is a spike during the control run: that inflates `dictionaries` and the assertion passes for
// free, so the flake's visible failures are the harmless half of it.
//
// `#[ignore]` plus a dedicated `--test-threads=1` run is the shape this repository already uses
// for tests that cannot share a process. A test nobody runs is no test, so it is registered in
// `ci.yml` and `tools/ci-local.py` and the parity check refuses until it is.
#[ignore = "reads process-wide VmHWM; run with --test-threads=1, see the ci-local job"]
fn an_annots_array_of_non_dictionaries_does_not_retain_a_warning_each() {
    // `qpdf_oh_get_key` on a NON-DICTIONARY reaches `QPDFObjectHandle::typeWarning` ->
    // `Common::warn`, which appends to qpdf's warning vector whatever `suppress_warnings` says
    // -- that flag only stops the printing -- and burrow sets no `max_warnings`. A review
    // measured an `/Annots` of 400,000 integers peaking at 265 MB from 800 kB of file.
    //
    // THE MEASUREMENT IS DIFFERENTIAL, and the first version was not. It compared total RSS
    // growth against a fixed ceiling, which also counts qpdf legitimately materialising
    // 400,000 array items -- so it passed in release and failed in debug, for a reason that
    // was not the defect. Two documents of the same shape, differing only in whether the array
    // items are dictionaries, isolate the retention: the dictionary run is the control, and
    // the integer run must not cost dramatically more.
    const ITEMS: usize = 200_000;

    let grew = |item: &str| -> u64 {
        let annots: String = (0..ITEMS)
            .map(|n| format!("{} ", item.replace("{n}", &n.to_string())))
            .collect();
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
        let before = peak_rss_kb();
        let counts = count(&opened).expect("an array of these draws no forms and is walked");
        assert!(
            counts.all().is_empty(),
            "neither shape is an appearance stream"
        );
        peak_rss_kb().saturating_sub(before)
    };

    // The control first, so its allocation is already in the high-water mark when the second
    // runs -- `VmHWM` never falls, so the order makes the comparison strictly conservative.
    //
    // `null`, NOT INTEGERS, since #166. An annotation entry that is not a dictionary is now
    // refused by name at the first one, so an integer array stops before it could retain
    // anything, and measuring it would measure one item. `null` is still skipped, so a `null`
    // array is the shape that walks every item without being a dictionary, which is the path
    // the container check exists for.
    let dictionaries = grew("<< >>");
    let nulls = grew("null");

    assert!(
        nulls <= dictionaries + 32 * 1024,
        "an /Annots of {ITEMS} nulls grew the peak RSS by {nulls} kB against {dictionaries} kB \
         for the same array of dictionaries -- the container type check is not holding, and \
         each skipped check retains a qpdf warning"
    );

    // AND THE INTEGERS ARE REFUSED BY NAME, which is why they are no longer the measured shape.
    let integers = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents 4 0 R /Annots [0 1 2] >>"
            .to_owned(),
        stream("", "BT ET\n"),
    ]);
    let refused = count(&open(integers)).expect_err("an /Annots of integers is refused");
    assert!(
        format!("{refused:?}").contains("[not-a-dictionary-where-one-belongs]"),
        "{refused:?}"
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

#[test]
fn the_live_handle_count_returns_to_its_baseline() {
    // THE CACHE ONLY GROWS. `handle.rs` explains why a per-page walk is exactly the shape that
    // fills it, and `rotate` already measures this for its ancestor climb. A walk that leaked
    // one handle per page would be invisible on a two-page fixture and fatal on a real one.
    let pages = 200u64;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        String::new(),
        stream("", "/Fm0 Do\n"),
        form("/F1 10 Tf BT 0 0 Td (SHARED) Tj ET\n"),
    ];
    let mut kids = Vec::new();
    for index in 0..pages {
        let page_id = 5 + index * 2;
        kids.push(format!("{page_id} 0 R"));
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /XObject << /Fm0 4 0 R >> >> /Contents 3 0 R /Annots [{} 0 R] >>",
            page_id + 1
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

/// Assert a refusal names `rule`, rather than merely being a refusal.
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

/// A document whose two pages share one font, each also having a font of its own.
fn two_pages_sharing_a_font() -> Vec<u8> {
    document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        // 3: page one — the shared font (6) and its own (7)
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /Shared 6 0 R /Mine 7 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
        // 4: page two — the shared font (6) and its own (8)
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /Shared 6 0 R /Mine 8 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
        stream("", "BT /Shared 12 Tf (a) Tj /Mine 12 Tf (b) Tj ET\n"),
        simple_font(),
        simple_font(),
        simple_font(),
    ])
}

fn simple_font() -> String {
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 97 /LastChar 98 \
     /Widths [500 500] >>"
        .to_owned()
}

#[test]
fn a_font_only_the_redacted_pages_use_may_be_cut() {
    // THE RULE. Cutting a font's `/Widths` changes the text of every page that uses it, so it
    // is safe exactly when the operation already covers all of them.
    let opened = open(two_pages_sharing_a_font());
    let counts = count(&opened).expect("walks");

    let pages = counts.font_pages();
    assert_eq!(
        pages.get(&(6, 0)).map(std::collections::BTreeSet::len),
        Some(2),
        "the shared font is used by both pages: {pages:?}"
    );
    assert_eq!(
        pages.get(&(7, 0)).map(std::collections::BTreeSet::len),
        Some(1),
        "page one's own font is used by one page"
    );

    // Redacting page 0 only: page one's own font may be cut, the shared one may not.
    let just_page_one: std::collections::BTreeSet<usize> = [0].into_iter().collect();
    let cuttable = counts.fonts_wholly_within(&just_page_one);
    assert!(
        cuttable.contains(&(7, 0)),
        "a font only the redacted page uses must be cuttable: {cuttable:?}"
    );
    assert!(
        !cuttable.contains(&(6, 0)),
        "a font another page uses must NOT be cuttable -- editing it reflows that page's \
         text, which is corruption rather than leakage and invisible to §6's read-back"
    );
    assert!(
        !cuttable.contains(&(8, 0)),
        "page two's own font is not used by page one at all, so redacting page one must not \
         touch it: {cuttable:?}"
    );
}

#[test]
fn redacting_every_page_makes_every_font_cuttable() {
    // THE OTHER HALF, and it is the one that stops the rule being "refuse on any sharing":
    // a single-page document, or an operation covering the whole document, cuts everything.
    // Refusing whenever a font is shared would refuse nearly every multi-page document.
    let opened = open(two_pages_sharing_a_font());
    let counts = count(&opened).expect("walks");
    let both: std::collections::BTreeSet<usize> = [0, 1].into_iter().collect();
    let cuttable = counts.fonts_wholly_within(&both);
    for font in [(6, 0), (7, 0), (8, 0)] {
        assert!(
            cuttable.contains(&font),
            "with every page redacted, {font:?} has nowhere else to affect: {cuttable:?}"
        );
    }
}

#[test]
fn a_font_reached_only_through_a_form_belongs_to_the_page_that_draws_the_form() {
    // REACHED-THROUGH COUNTS. A font named inside a form that page two draws is a font page
    // two uses, and cutting it changes page two. A direct-references-only join would miss
    // that -- the direction that cuts a font somebody else still needs.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        // page one draws the form; page two draws it too
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 6 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 6 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
        stream("", "/Fm0 Do\n"),
        // 6: the form, whose own resources name the font
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] \
             /Resources << /Font << /F1 7 0 R >> >>",
            "BT /F1 12 Tf (a) Tj ET\n",
        ),
        simple_font(),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("walks");
    assert_eq!(
        counts
            .font_pages()
            .get(&(7, 0))
            .map(std::collections::BTreeSet::len),
        Some(2),
        "both pages reach the font through the form: {:?}",
        counts.font_pages()
    );
    let just_page_one: std::collections::BTreeSet<usize> = [0].into_iter().collect();
    assert!(
        !counts.fonts_wholly_within(&just_page_one).contains(&(7, 0)),
        "redacting page one must not cut a font page two reaches through the same form"
    );
}

#[test]
fn the_ordinary_shape_retains_its_font_and_says_by_how_much() {
    // THE CASE THE COMMITTED CORPUS CANNOT SHOW. It is almost entirely single-page, where
    // every font is cuttable by construction — so the retain path has one example in it and
    // the rate the rule was chosen against is unmeasurable from fixtures alone.
    //
    // A several-page document sharing one font, redacted on a single page, is what a person
    // actually brings: a report, a contract, a statement. Here the font is used by five pages
    // and the operation covers one, so it is retained and §7's disclosure applies — and the
    // count of pages it is retained *for* is what the disclosure is about.
    const PAGES: u64 = 5;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        String::new(),
        stream("", "BT /F1 12 Tf (a) Tj ET\n"),
        simple_font(),
    ];
    let mut kids = Vec::new();
    for index in 0..PAGES {
        let id = 5 + index;
        kids.push(format!("{id} 0 R"));
        objects.push(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 3 0 R >>"
                .to_owned(),
        );
    }
    objects[1] = format!(
        "<< /Type /Pages /Count {PAGES} /Kids [{}] >>",
        kids.join(" ")
    );

    let opened = open(document(&objects));
    let counts = count(&opened).expect("walks");

    let pages = counts.font_pages();
    assert_eq!(
        pages.get(&(4, 0)).map(std::collections::BTreeSet::len),
        Some(usize::try_from(PAGES).expect("fits")),
        "every page uses the one font: {pages:?}"
    );

    // Redacting page 2 alone — a page in the middle, not the first, because an off-by-one in
    // the page index would be invisible on page 0.
    let one_page: std::collections::BTreeSet<usize> = [2].into_iter().collect();
    let cuttable = counts.fonts_wholly_within(&one_page);
    assert!(
        cuttable.is_empty(),
        "the font must be retained, not cut: cutting it would reflow the other four pages, \
         which is damage to documents nobody asked about: {cuttable:?}"
    );

    // And the number the disclosure is about: four pages outside the operation.
    let outside = pages
        .get(&(4, 0))
        .map(|used| used.difference(&one_page).count())
        .expect("the font is used");
    assert_eq!(
        outside, 4,
        "§7's disclosure is about the four pages that keep using this font"
    );

    // THE CONTROL: redacting every page makes the same font cuttable, so the retention above
    // is the sharing and not something structural about the fixture.
    let every_page: std::collections::BTreeSet<usize> =
        (0..usize::try_from(PAGES).expect("fits")).collect();
    assert!(
        counts.fonts_wholly_within(&every_page).contains(&(4, 0)),
        "with the whole document redacted the font has nowhere else to affect"
    );
}

#[test]
fn the_walk_counts_every_reference_to_every_page_content_stream() {
    // THE WALK HALF OF THE SHARED-/CONTENTS RULE, asked directly. The operation's tests go in
    // through `redact_page` and can only see the refusal; this sees the counts, which is what
    // distinguishes "the walk found nothing" from "the walk found one".
    //
    // Object 5 is referenced three times: twice by page 1's array and once by page 2. Object 6
    // once. A rule counting PAGES would say 5 is used by two pages and miss the repeat, which
    // is the shape `/Contents [5 0 R 5 0 R]` makes ordinary.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents [5 0 R 5 0 R 6 0 R] >>"
            .to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents 5 0 R >>"
            .to_owned(),
        stream("", "BT ET\n"),
        stream("", "q Q\n"),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("the fixture walks");

    let all = counts.all_contents();
    assert_eq!(all.len(), 2, "two distinct content streams: {all:?}");
    let mut references: Vec<usize> = all.values().copied().collect();
    references.sort_unstable();
    assert_eq!(
        references,
        vec![1, 3],
        "object 5 is referenced three times and object 6 once: {all:?}"
    );

    // AND THE PAGE SETS, which are the diagnostic rather than the rule. Object 5 is reached by
    // both pages; object 6 by one.
    let (shared, _) = all
        .iter()
        .find(|(_, count)| **count == 3)
        .expect("the shared stream");
    assert_eq!(
        counts.content_pages_of(*shared).len(),
        2,
        "three references across two pages"
    );
}

#[test]
fn a_page_with_no_contents_contributes_nothing_rather_than_refusing() {
    // A page may legally have no `/Contents` -- it draws nothing. The walk must pass over it,
    // because refusing here would refuse the whole document for a page the operation was never
    // asked about.
    let bytes = document(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents 5 0 R >>"
            .to_owned(),
        stream("", "BT ET\n"),
    ]);
    let opened = open(bytes);
    let counts = count(&opened).expect("a page with no /Contents is not an error");
    assert_eq!(counts.all_contents().len(), 1);
    assert_eq!(counts.all_contents().values().copied().sum::<usize>(), 1);
}

#[test]
fn a_contents_array_past_the_element_ceiling_is_refused_by_the_walk() {
    // ASKED OF THE WALK DIRECTLY, and that is the point. The operation refuses this too, but
    // only because the walk runs first inside `QpdfRedaction::new` -- so a test going in
    // through `redact_page` cannot tell which ceiling fired, and for a while there were two
    // ceilings with one rule name, each masking the other from a mutation sweep.
    //
    // The ceiling bounds the WALK's work: an `array_item` and an `object()` per element, before
    // anything has decided the document is one this operation will touch.
    let elements = crate::pdfsyntax::contents::MAX_ELEMENTS + 1;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        String::new(),
    ];
    let content = objects.len() + 1;
    objects.push(stream("", "BT ET\n"));
    let refs: Vec<String> = std::iter::repeat_n(format!("{content} 0 R"), elements).collect();
    objects[2] = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents [{}] >>",
        refs.join(" ")
    );
    let opened = open(document(&objects));
    let error = count(&opened).expect_err("past the element ceiling");
    assert!(
        format!("{error}").contains("contents-too-many"),
        "the walk must refuse by name, got: {error}"
    );
}

#[test]
fn a_contents_array_at_the_element_ceiling_is_walked() {
    // THE NEAR-MISS. Without it the test above passes for a ceiling of one, which would refuse
    // every two-element document there is.
    let elements = crate::pdfsyntax::contents::MAX_ELEMENTS;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        String::new(),
    ];
    let content = objects.len() + 1;
    objects.push(stream("", "BT ET\n"));
    let refs: Vec<String> = std::iter::repeat_n(format!("{content} 0 R"), elements).collect();
    objects[2] = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> \
         /Contents [{}] >>",
        refs.join(" ")
    );
    let opened = open(document(&objects));
    let counts = count(&opened).expect("exactly at the ceiling");
    assert_eq!(
        counts.all_contents().values().copied().sum::<usize>(),
        elements,
        "every reference is counted, including the repeats"
    );
}

#[test]
fn one_shared_properties_dictionary_is_listed_once_however_many_forms_reach_it() {
    // KILLS: the `/Properties` identity memo. A security review of #166 measured the shape
    // without it at 44.3 s against a 1 s deadline: every form's resources re-listed the one
    // shared dictionary. Pinned by a count, not a clock -- see `properties_listed`.
    let forms = 40;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
    ];
    let names: String = (0..forms)
        .map(|at| format!("/X{at} {} 0 R ", at + 6))
        .collect();
    objects.push(format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
         /Resources << /XObject << {names}>> >> >>"
    ));
    objects.push(stream("", ""));
    objects.push("<< /M0 << /MCID 0 >> /M1 << /MCID 1 >> >>".to_owned());
    for _ in 0..forms {
        objects.push(stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Properties 5 0 R >>",
            "",
        ));
    }
    let counts = count(&open(document(&objects))).expect("the walk accepts it");
    assert_eq!(
        counts.properties_listed(),
        1,
        "{forms} forms share one /Properties; it must be listed once"
    );
}
