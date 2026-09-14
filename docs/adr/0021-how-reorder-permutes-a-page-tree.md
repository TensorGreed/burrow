# 0021. Reorder permutes in place, and accepts a flattened page tree

Date: 2026-09-13

## Status

Accepted.

The `add-operation` checklist's §1 and §2a, answered before the operation was written. §2a asks
whether an operation needs a capability no engine has; reorder does not. What it needs is a
decision about **which** of two existing routes to take, and the measurement that decided it
turned up a fidelity consequence neither route avoids.

## Context

`reorder` produces a permutation of its input: no page lost, added or duplicated, and the
identity permutation is a no-op (`docs/ROADMAP.md`). Every input page appears in the output, so
it is **not a subsetting operation** and [ADR 0019](0019-how-split-builds-its-outputs.md) §2's
rule — an output may carry nothing derived from what it excluded — has nothing to bite on. The
obligation runs the other way, as it does for `rotate`: nothing may be lost.

Two routes were available, and the project has now taken each of them once:

| | operation | why |
|---|---|---|
| **build** — blank destination, copy pages in | `split` | ADR 0019 §1: an allowlist by construction, because a subsetting operation must not carry the source's catalog |
| **in place** — edit the document qpdf parsed | `rotate` | nothing is excluded, so building would drop navigation for no safety reason |

### What building would cost here, and why it is the wrong trade

`split` builds, and ADR 0019 §1 records what that costs: the destination starts empty, so the
outline, the attachments and the `/AcroForm` do not come across at all. That is the right trade
for a subsetting operation, where carrying them is a *leak*.

For reorder it would be pure loss. Reordering a document's pages is not a reason to lose its
bookmarks. So reorder permutes in place, through `qpdf_remove_page` and `qpdf_add_page_at`.

### Both calls are cleared, and one is not yet declared

`qpdf_remove_page`, `qpdf_add_page_at`, `qpdf_add_page` and `qpdf_get_page_n` are all on
`engines/qpdf-trapped-functions.txt` as `direct` — they call `trap_errors` in their own bodies,
so ADR 0013 §1 clears them without any of the helper-following argument the `qpdf_oh_*` family
needed. `qpdf_add_page_at` is the only one not yet declared in `qpdf/ffi.rs`, and it is not in
the wasm export list; it takes an entry in `engines/qpdf-not-exported.toml` until reorder's
bridge lands, the same lifecycle `rotate`'s six `qpdf_oh_*` functions went through.

### A page handle survives its removal, and that is the thing the design rests on

`QPDF::removePage` is `m->pages.erase(page)`, and `Pages::erase` does `kids.eraseItem(pos)` —
it takes the page out of `/Kids` and does not destroy the object. So a `qpdf_oh` obtained
before a removal is still usable after it, which is what makes "take the page out, put it back
somewhere else" possible at all. Read from `libqpdf/QPDF_pages.cc`, not assumed.

## The measurement, which is the part worth keeping

`Pages::erase` calls `findPage`, whose comment says it "also ensures flat /Pages", and
`flattenPagesTree` begins by calling `pushInheritedAttributesToPage(true, true)`. So the source
says: reordering flattens the page tree, and pushes inherited attributes down before it does.

Measured end to end, with the committed `inherited-rotation-6page.pdf` fixture — a two-level
tree whose **root** carries `/Rotate 90`, so no page has one of its own:

```
qpdf inherited-rotation-6page.pdf --pages . 6,5,4,3,2,1 -- reordered.pdf
```

| | `/Rotate` | `/Type /Pages` | objects | page order |
|---|--:|--:|--:|---|
| input | 1, on the root | 3 | 16 | 1…6 |
| output | **6, one per page** | **1** | **14** | 6…1 |

Three facts, in order of how much they matter:

1. **What each page displays is preserved.** The inherited `/Rotate 90` is pushed onto all six
   pages before the tree is flattened, so every page still displays turned. Had qpdf flattened
   *without* pushing, reordering would silently unrotate an entire scanned document — which is
   the failure mode this ADR exists to have checked rather than assumed.
2. **The page tree is flattened.** Two intermediate `/Pages` nodes are gone: 16 objects in, 14
   out. That is a structural rewrite of the document, not a permutation of it.
3. **Pages gain keys they did not have.** `/Rotate` appears on each page where it appeared on
   none. A byte-level comparison of any page dictionary will differ.

## Decision

**We will reorder in place, and accept the flattened page tree.**

The alternative is rewriting `/Kids` ourselves through the object-handle API — permuting the
array, fixing every `/Count`, re-parenting pages, and pushing inherited attributes down by hand
so a cross-branch permutation does not change what a page displays. That is reimplementing
`flattenPagesTree` in our own code, against hostile input, to preserve a structure no reader
can observe. It would be more code, more `unsafe`, and more ways to get a document wrong, in
exchange for a property nobody has asked for.

**Three consequences are written down rather than discovered:**

- **The closure harness is used in its inverted form, with page-tree scaffolding excluded.**
  `rotate` asserts `assert_nothing_lost` over `content` and `navigation` and loses nothing.
  Reorder legitimately loses the intermediate `/Pages` nodes, so the fixture's page-tree
  scaffolding is classified apart and the test states the loss. **The harness is not weakened
  to accommodate it**: the `content` and `navigation` sets are still required whole, and it is
  the third kind — structure that exists only to hold other pages — that is exempt, by name.
- **`/reorder-pdf` says what changes.** Every tool page owes the paragraph `/merge-pdf` has and
  `/rotate-pdf` has: what is kept and what is not. Reorder's says that bookmarks, attachments,
  form fields and annotations come through, that what each page displays is unchanged, and that
  the document's internal page tree is rebuilt — which matters to nobody reading the document
  and to anybody comparing its bytes.
- **A signature does not survive.** No operation here preserves one — any write invalidates it
  — but reorder is the first where somebody might expect otherwise, because "the pages are the
  same pages" is true and the file is still not the same file.

### The identity is the one case that does not flatten

If every page is already where it belongs, **no page is moved** — every comparison in the
permutation loop matches and every iteration is skipped — so nothing calls into qpdf's page
machinery and **the page tree survives intact**. Any permutation that actually moves a page
flattens it.

**The short-circuit is not the mechanism, and this paragraph originally said it was.** Code
review replaced `if !order.is_identity()` with `if true` and the whole suite stayed green:
`qpdf_get_page_n` does not flatten, and `qpdf_remove_page`/`qpdf_add_page_at` are never reached
for an identity order. The branch saves `n` comparisons and `n` engine calls and changes no
output. The correction is recorded rather than smoothed over because the two claims have
different consequences: if the short-circuit were the mechanism, deleting it would be a
correctness change, and it is not.

So the output's structure depends on whether anything actually moved — which is a fact about
qpdf's page machinery, not a choice this code makes. That is not a defect: doing less damage
when asked to do nothing is the better behaviour, and the alternative (flattening
unconditionally, for consistency) would mean rewriting a document's page tree to carry out a
request to change nothing. It is recorded because it is surprising, and because it
caught a test: the first version of the order-reading helper read the first `/Kids` array,
which is correct for a flattened tree and wrong for the two-level one the identity leaves
behind. The helper walks the tree now.

## Consequences

**Easier.** No new engine capability: one FFI declaration, no bridge method beyond what the
web already has plus that one, no engine rebuild, no size-budget movement for the core. The
operation is a page-handle permutation, which is the smallest thing any of M1's five operations
has needed.

**Harder, and accepted.** The output's object graph differs from the input's in a way that is
not a permutation, so the inverse-closure test needs a third object kind and the fixture needs
to declare it. That is a change to a shared harness for one operation's benefit, and it is
justified only because the alternative — weakening `assert_nothing_lost` to a subset check —
would quietly weaken it for `rotate` and `compress` too.

**A measurement we now rely on.** That qpdf pushes inherited attributes down before flattening
is load-bearing for correctness, and it is upstream behaviour rather than ours. It is checked
by a test on a document whose pages inherit, so a qpdf bump that changed it fails here rather
than silently unrotating somebody's scan.

## Alternatives considered

**Build, as `split` does.** Rejected: it drops the outline, the attachments and the
`/AcroForm`, and there is no safety argument for the loss because nothing is excluded. ADR 0019
§1's reasoning is specific to subsetting and does not transfer.

**Rewrite `/Kids` through the object-handle API.** Rejected above — reimplementing qpdf's page
tree handling to preserve an unobservable structure.

**Remove every page, then add them all back in order.** Simpler to write than the
position-by-position permutation, and rejected because it puts the document through a state
with no pages. qpdf refuses to *open* a zero-page document — the corpus records `no-pages` as
`Malformed` — and while nothing says it refuses a transient one, "nothing says it refuses" is
not a guarantee to build on. The permutation keeps at least one page in the tree at all times.

**Refusing to reorder a document whose page tree is not already flat.** Would preserve the
structure by declining the cases where it cannot. Rejected as a refusal nobody could act on:
a person cannot flatten their own page tree, the shape is invisible in every viewer, and the
attribute-pushing means the document is not harmed by it.
