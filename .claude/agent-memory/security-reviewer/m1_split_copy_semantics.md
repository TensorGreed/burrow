---
name: m1-split-copy-semantics
description: What qpdf's copyForeignObject/addPage actually carries across in burrow's split, measured — the reachability leak class ADR 0019 §2's canary test cannot see
metadata:
  type: project
---

`split` (M1, ADR 0019) copies pages with `qpdf_add_page` → `QPDF::addPage` →
`Pages::insert` → `pushInheritedAttributesToPage()` on the SOURCE, then
`copyForeignObject`. The copy is **reachability-based**, and reachability from a kept page
is attacker- (and ordinary-producer-) controlled.

**Why:** measured 2026-09-13 in the M1 split review, with a scratchpad crate depending on
`core/burrow-ops` by path (`features = ["native-engines"]`,
`LD_LIBRARY_PATH=engines/vendor/native-$(uname -m)/lib`). Ground truth is
`engines/vendor/src/qpdf-12.4.1/libqpdf/QPDF.cc`,
`Objects::Foreign::Copier::reserve_objects`.

The two stopping rules, and nothing else:
- `foreign.isPagesObject()` → return. `/Parent` chains are cut.
- non-top `foreign.isPageObject()` → mapped to a fresh **indirect null**, not traversed.
  So `/Dest [<excluded page> /XYZ 111 222 3]` emits `/Dest [ N 0 R /XYZ 111 222 3 ]` with
  `N 0 obj null` — the excluded page does not come, the coordinates do.

**Everything else reachable is deep-copied.** Measured leaks into a page-1-only output:
- a `/Resources` on the `/Pages` node (inherited) is made indirect and attached to every
  page by `pushInheritedAttributesToPageInternal`, so **every** output carries the whole
  shared resource dict — fonts and XObjects only excluded pages used.
- an AcroForm field node reached through a kept widget's `/Parent`: the output carried
  `/V (value typed on page 2)` and the excluded page's widget object, with **no `/AcroForm`
  in the output catalog**, i.e. invisible in any viewer.
- an `/Annots` array shared between a kept and an excluded page: all of it.
- `/B` article beads chain to beads on excluded pages and to the thread `/I` info dict.
- `/A << /S /GoTo /D (name) >>` — the named-destination string survives verbatim.
- catalog `/OCProperties` is dropped while the OCG dicts reachable via
  `/Resources /Properties` are kept, so **content hidden by an OFF layer becomes visible**.

Dropped, confirmed: `/Info`, catalog `/Metadata`, `/Lang`, `/PageLabels`, `/Names/Dests`,
`/Outlines`, `/StructTreeRoot`, `/AcroForm`. Outputs are written fresh — no free objects,
no incremental-update history, no `/Producer`.

**Output amplification is real and unlimited:** 1.06 MB / 200 pages sharing one 1 MB
incompressible resource → **201 MB across 200 outputs, 216 MB peak RSS**, no limit fires.
`burrow-ops`'s `split` accumulates every part in a `Vec<Vec<u8>>`, and
`PageExtractor::open` in `qpdf/extract.rs` never calls `estimate::check_measured_memory`
(the merge path in `assemble.rs` does).

**How to apply:** any subsetting operation on this seam — extract, and M2's redact — must be
reviewed against object *sharing*, not against per-page features. A fixture whose pages share
nothing (which `tools/make-split-fidelity-fixtures.py` is) cannot fail the test.
See [[m1-qpdf-exception-boundary]] and [[m1-limits-real-strength]].

**The structural successor and its blind-spot class (2026-09-13, second review).**
`tools/make-marked-document.py` + `core/burrow-engines/testsupport/object_closure.rs` replace the
canary list with a declared-ownership property. It is the right shape, and it inherits one hazard
the canary list had: a marker declared in the manifest but **absent from the generated document**
is silently un-findable, so that channel can never fail the test. Measured: object 13, the
inherited-`/Resources` object, declared `BM-013` while its body carries `BM-000`. The control that
catches this is a **source self-test** — run `survivors()` over the fixture itself and require
*all* markers, gated on the exact manifest count — which `split_no_leak.rs` has
(`the_scan_finds_a_canary_that_is_really_there`) and `object_closure.rs` does not.
`rotate`/`reorder`/`compress`/redact all inherit this harness, so check for that self-test first.
