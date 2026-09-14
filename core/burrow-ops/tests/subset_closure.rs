//! The structural property, on `split`: an output carries no object belonging only to pages it
//! excluded.
//!
//! **This is the test ADR 0019 §2 needs and the canary version never was.** The canary test
//! plants markers by feature and confirms the enumeration it was given; this asserts a property
//! over *every* object in the source, so it fails for categories nobody named.
//!
//! The harness is shared (`core/burrow-engines/testsupport/object_closure.rs`) rather than
//! written here, because the next three operations need it too — and they need it pointing the
//! other way. `rotate`, `reorder` and `compress` are not subsetting operations: every page
//! survives, so *every* marked object should, and `assert_closed` with all pages included says
//! exactly that.
//!
//! ADR 0019 §2a's named channels stay as regression cases on top, in `split_no_leak.rs`, and
//! **both layers are required while neither is sufficient**. They catch what this cannot: a leak
//! **inside** an object that legitimately survives -- a form field owned by two pages carrying the
//! value somebody typed on the excluded one (row 2), and a named destination sitting in a kept
//! page's own link annotation (row 5). This catches what they cannot: a category nobody named.

#![cfg(all(feature = "native-engines", target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

/// The shared harness, included by path rather than copied.
///
/// Same reason `minimal_pdf.rs` is included this way from three places: two copies agree until
/// the day they do not, and this one is about to be inherited by three more operations.
#[path = "../../burrow-engines/testsupport/object_closure.rs"]
mod object_closure;

use std::sync::Arc;

use burrow_engines::OpenOptions;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::{Cuts, split};
use burrow_types::{Clock, Limits, ManualClock};
use object_closure::{assert_closed, assert_nothing_lost, marked_document, owned_by, trespassers};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn split_marked(cuts: &[u64]) -> Vec<Vec<u8>> {
    split(
        &Qpdf::new(),
        marked_document().bytes.into_boxed_slice(),
        Cuts::after_pages(cuts),
        &options(),
    )
    .expect("the marked document must split")
}

#[test]
fn every_surviving_object_belongs_to_a_page_the_output_contains() {
    // THE PROPERTY, and the gate on ADR 0019 §2. `#[ignore]`d and expected to fail for the
    // whole of M1; issue #54 closed it.
    let marked = marked_document();
    let outputs = split_marked(&[1, 3]);
    // Pages 1 | 2-3 | 4-5.
    for (output, included) in outputs.iter().zip([vec![1u64], vec![2, 3], vec![4, 5]]) {
        assert_closed(&marked, output, &included, "split");
    }
}

#[test]
fn the_closure_scan_can_still_find_a_trespasser_that_is_really_there() {
    // THE REPLACEMENT FOR `the_closure_property_is_violated_exactly_where_the_adr_records_it`,
    // and a replacement rather than a deletion.
    //
    // That test asserted the closure property was still VIOLATED. While the rule was unmet it
    // was the only thing separating two states a green run cannot tell apart: the leak closing,
    // and the harness going blind. Pruning has made the first true. The second is exactly as
    // possible as it was before -- a changed marker prefix, a manifest the reader stopped
    // understanding, an expansion that silently stopped decompressing -- and it now produces the
    // same clean run as success.
    //
    // So the assertion inverts rather than disappearing: the identical `trespassers` call, over
    // the SOURCE, where every page's objects are present by construction. It must report the
    // objects belonging only to pages 2 to 5 as trespassing on a claim of page 1.
    let marked = marked_document();
    let intruders = trespassers(&marked, &marked.bytes, &[1]);
    assert!(
        !intruders.is_empty(),
        "the harness found no trespasser in a document that contains every page's objects, so \
         every closure assertion in this file is measuring nothing"
    );

    // AND IT SEES EACH KIND, not merely something. A scan that had lost its ability to match
    // stream-borne markers would still find the dictionary-borne ones and look healthy -- which
    // is the shape of the near-miss `CLAUDE.md` asks every rule-driven check to carry.
    for kind in ["content", "navigation"] {
        assert!(
            intruders.iter().any(|(number, _, _)| marked
                .objects
                .get(number)
                .is_some_and(|object| object.kind == kind)),
            "the harness cannot see a trespassing {kind} object, so a leak through one would \
             be invisible"
        );
    }

    // AND THE DENOMINATOR. `survivors` over the source must be every marked object, because the
    // source contains all of them -- so a reader that had understood half the manifest, or a
    // scan matching half the markers, fails here rather than reporting a clean split.
    let survived = object_closure::survivors(&marked, &marked.bytes);
    assert_eq!(
        survived.len(),
        marked.objects.len(),
        "the scan found {} of the source's own {} markers; it is not seeing what it reports on",
        survived.len(),
        marked.objects.len()
    );
}

#[test]
fn a_resource_survives_exactly_where_it_is_drawn() {
    // THE TWO-SIDED ASSERTION FOR THE RESOURCE FILTER, and the only test here that fails for
    // BOTH of its failure modes rather than one.
    //
    // `assert_closed` catches over-keeping: a resource belonging only to page 4 in an output
    // that excludes page 4. `assert_nothing_lost` catches over-pruning across a whole output.
    // Neither says the two are the SAME resource — and the filter's most likely defect, a name
    // scan that stops finding names, prunes everything everywhere, which reads as a perfectly
    // closed output.
    //
    // The fixture's inherited `/Resources` holds one stream that page 4 draws and nobody else
    // does, so it must be in the part containing page 4 and absent from the part that is not.
    // It is looked up BY ROLE rather than by number: object numbers shift whenever the
    // generator gains an object, and a test pinned to a literal would then assert about
    // whatever moved into its place — silently, and in the direction of passing.
    let marked = marked_document();
    let resource = *marked
        .roles
        .get("inherited_resource")
        .expect("the manifest names the resource only page 4 draws");

    let outputs = split_marked(&[3]);
    assert_eq!(outputs.len(), 2);
    let without = object_closure::survivors(&marked, &outputs[0]); // pages 1-3
    let with = object_closure::survivors(&marked, &outputs[1]); // pages 4-5

    assert!(
        with.contains(&resource),
        "the resource page 4 draws with is missing from the part that CONTAINS page 4 — the \
         name filter is pruning what the page uses"
    );
    assert!(
        !without.contains(&resource),
        "the resource only page 4 draws is in the part that EXCLUDES page 4 — ADR 0019 §2a \
         row 1, the inherited-`/Resources` channel, is open again"
    );
}

#[test]
fn an_annotation_that_cannot_prove_where_it_lives_does_not_travel() {
    // THE COST OF THE CONSERVATIVE RULE, pinned rather than discovered.
    //
    // `/P` is optional and producers routinely omit it. When an `/Annots` array is shared with a
    // page the output does not contain, an annotation with no `/P` cannot be placed — so it is
    // dropped, from EVERY part, including the one containing the page it actually belonged to.
    // That is ADR 0019 §2b's "drop the array, losing the kept page's own annotations —
    // acceptable only if said", and this is where it is said in executable form.
    //
    // It is a real fidelity loss and it is the safe direction: the alternative is carrying an
    // annotation into a file whose pages it may have nothing to do with. It was found by this
    // file's sibling test asserting the opposite, which is the right way round to find it.
    let marked = marked_document();
    let orphan = *marked
        .roles
        .get("annotation_without_p")
        .expect("the manifest names the annotation with no /P");

    let outputs = split_marked(&[3]);
    for (part, output) in outputs.iter().enumerate() {
        let survived = object_closure::survivors(&marked, output);
        assert!(
            !survived.contains(&orphan),
            "part {part} kept an annotation that cannot say which page it is on, out of an \
             array shared with a page this part does not contain"
        );
    }

    // AND THE NEAR-MISS: the annotation that CAN prove it belongs survives. Without this, the
    // assertion above is satisfied by a filter that erases every annotation in the document —
    // which is exactly what the first version of the rule did.
    let with_p: Vec<u64> = owned_by(&marked, &[1], &["content"]);
    let kept = object_closure::survivors(&marked, &outputs[0]);
    assert!(
        with_p.iter().any(|object| kept.contains(object)),
        "nothing page 1 owns survived into the part containing page 1, so the assertion above \
         is passing because everything was erased"
    );
}

#[test]
fn a_one_way_split_loses_no_page_content() {
    // THE INVERSE ASSERTION, and the shape `rotate`, `reorder` and `compress` will use.
    //
    // The first version of this called `assert_closed` with every page included and claimed to
    // be "the control". It could not fail: with every page included the trespasser list is
    // mathematically empty whatever the operation did. Code review measured it by handing the
    // page-1-only output to `assert_closed` while claiming all five pages — a must-lose-nothing
    // operation losing four pages in five, accepted.
    //
    // `assert_nothing_lost` names what has to be there instead. A one-way split legitimately
    // drops the catalog furniture ADR 0019 §1 drops, so the required set is what belongs to the
    // pages rather than everything marked.
    let marked = marked_document();
    let outputs = split_marked(&[]);
    assert_eq!(outputs.len(), 1);

    let all_pages: Vec<u64> = (1..=marked.pages).collect();
    assert_nothing_lost(
        &marked,
        &outputs[0],
        // CONTENT ONLY. A split may drop navigation — ADR 0019 §1 drops outlines and the page
        // says so — and requiring it here would fail a correct implementation, which is how
        // this test first failed: 18 of 23, the five missing being outline entries.
        &owned_by(&marked, &all_pages, &["content"]),
        "one-way split",
    );
}

#[test]
fn losing_a_page_is_caught_by_the_inverse_assertion() {
    // THE CONTROL FOR THE CONTROL. Without it, `assert_nothing_lost` passing would say nothing
    // about whether it can fail -- which is exactly what went wrong with the assertion it
    // replaced. A two-way split's first part must NOT satisfy a requirement built from all
    // five pages, because four of them are not in it.
    let marked = marked_document();
    let outputs = split_marked(&[1]);
    let all_pages: Vec<u64> = (1..=marked.pages).collect();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_nothing_lost(
            &marked,
            &outputs[0],
            &owned_by(&marked, &all_pages, &["content"]),
            "deliberately wrong claim",
        );
    }));
    assert!(
        result.is_err(),
        "a one-page output satisfied a requirement naming all five pages' objects, so \
         assert_nothing_lost cannot detect loss and every use of it is decoration"
    );
}
