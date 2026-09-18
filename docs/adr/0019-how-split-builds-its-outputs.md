# 0019. How a split builds its outputs, and what a subsetting operation may not carry

Date: 2026-09-13

## Status

Accepted.

## Context

`split` is M1's second operation and the first that produces **several** documents from one.
Two questions had to be settled before any of it could be written, and the second turned out
to be much larger than `split`.

### Which entry points are even available

A split needs a **destination** document to put pages into, and qpdf offers exactly one way to
make an empty one: `qpdf_empty_pdf`. **burrow may not call it.** It is not on
`engines/qpdf-trapped-functions.txt`, and it does not meet the bar
`engines/qpdf-untrapped-accepted.toml` sets for an argued exception — "non-parsing … never
touches the PDF's bytes, never resolves an object". Its implementation is `QPDF::emptyPDF()`,
which is

```cpp
processMemoryFile("empty PDF", EMPTY_PDF, strlen(EMPTY_PDF));
```

— the full parser, on a constant, with no `trap_errors` wrapper on the C function. A `QPDFExc`
out of that crosses an `extern "C"` frame and aborts the process ([ADR 0013](0013-qpdf-c-api-and-prescan.md) §1).
Writing an entry that called it non-parsing would be false.

That leaves two routes built only from trapped functions:

- **CARVE** — read the source once per output and `qpdf_remove_page` everything outside the
  range. Every output inherits the source's catalog. One full parse per output.
- **BUILD** — read a near-empty PDF **of our own** as the destination, through the trapped
  `qpdf_read_memory`, then `qpdf_add_page` the wanted pages from a single open source. One
  parse of the source.

  **The destination is one BLANK PAGE, removed once the wanted pages are in — not a zero-page
  document.** qpdf will not open a document with no pages, which this corpus already records:
  the conformance case `no-pages` expects `Malformed`. Earlier drafts of this ADR said
  zero-page in four places while `extract.rs` had spent thirty lines explaining why it could
  not be, and a reader implementing the Decision as written would have got something that does
  not open. Taking the blank page back out needs `qpdf_remove_page`, which **is** trapped, so
  this route adds one FFI declaration rather than none.

### What they keep, measured

`core/burrow-engines/examples/measure-split.rs`, on fixtures from
`tools/make-split-fidelity-fixtures.py`. Pages 2–3 of a five-page document.

| fixture | feature | carve | build | carve leaks | build leaks |
|---|---|---|---|---|---|
| `outline-per-page.pdf` | outline, one entry per page | kept | LOST | **pages 1, 4, 5** | none |
| `fields-across-pages.pdf` | `/AcroForm`, fields on pp 2 & 4 | kept | kept | none | none |
| `attachment-5page.pdf` | attachment on the catalog | kept | **LOST** | none | none |

Cost, five-way split: carve 222 µs, build 115 µs.

**It took three corrections to make that table mean anything, and all three are recorded
because each produced a confident measurement of nothing.**

1. The first run used the **merge** fidelity fixtures. They are one page each, so a one-page
   split of them is the identity: every feature trivially survived and both routes scored
   perfect.
2. With five-page fixtures, `carve` "kept" the outline and `build` "lost" it — which reads as
   carve winning, by exactly the reasoning [ADR 0017](0017-merge-engine-and-failure-semantics.md)
   used to choose qpdf over PDFium. Only checking whether what survived was **coherent**
   reversed it: the two-page carve output contains all five outline titles, three of them
   pointing at pages that are not in the document.
3. The harness's decompression step **did not work and did not say so**. It piped through
   stdin — `qpdf --qdf - -`, which this qpdf rejects with `open -: No such file or directory` —
   and fell back to the raw bytes when it got nothing back. Its own missing-CLI warning never
   fired, because the CLI was present and the invocation was wrong. Every marker sitting in a
   flated stream therefore read as lost. Fixing it changed one row: carve **keeps** the
   attachment. Both the harness and the test now fail loudly rather than degrading.

## Decision

### 1. Split builds its outputs; it does not carve them

A **one-page blank** destination we ship, emptied with `qpdf_remove_page` after the copy, plus
`qpdf_add_page` from one open source. It cannot be zero-page: qpdf refuses to open such a
document, and the corpus records that as `no-pages` → `Malformed`.

`build` keeps what genuinely travels with a page — its content, its boxes, its annotations,
the widget that makes a form field a field. It drops document-level furniture: outlines and
catalog-level attachments. The `/split-pdf` page will say so, in the voice `/merge-pdf` uses to
say what it keeps.

Carve was rejected for the leak, not for the cost. Cost was the tiebreak it did not need.

### 2. A subsetting operation may carry nothing derived from what it excluded

**Generalised, because `split` is not the only operation this applies to and not the one where
it matters most.**

> For any operation whose output is a **subset** of its input — split, extract, and above all
> redact — the emitted bytes must contain **no data derived from the excluded content**. Not
> the content, and not anything computed from it: titles, names, destinations, counts,
> thumbnails, or index entries that describe what was taken away.

Carve fails this. It looks like it succeeds, because the pages are right and the file opens —
and the outline titles of the excluded pages ride along inside it. Somebody splitting pages 2–3
out of a document to send to another person would be sending the titles of pages 4 and 5 with
them, and nothing on the page or in the file would suggest it.

**This is the property redaction depends on, and it is already written down for redaction.**
[ADR 0006](0006-wasm-linking-strategy.md)'s R8, R9 and R10 are three statements about one
thing: what matters is the **bytes that are emitted**, not what the operation meant to emit.
R8 forbids emitting output before the call that produces it returns; R9 forbids anything
leaving the heap before verification; **R10 requires verification to run on the exact byte
sequence emitted, never on a sibling copy**. Each exists because an operation's intent and its
output can disagree.

Split is the same disagreement, arriving two milestones early and without a verifier to catch
it. A redaction built on carve's shape would remove the words from a page and leave the
bookmark naming them — and would pass any check that asked "are the right pages present".

**So it is a required test, not a principle.** See below; a rule of this kind that is not
executed is a comment, which this repository has measured the cost of more than once.

**The rule was not met when this ADR was written, and is now.** §2a records six channels through
which the chosen route carried data from excluded pages, all measured; for the whole of M1 this
paragraph said so, because a section stating a standard may not claim the code reaches it —
that would be the exact failure the section is about, an operation's intent and its output
disagreeing, written down as if they agreed. Issue #54 closed them, and the 2026-09-14 amendment
below records which of §2b's answers each channel took, two defects the harness caught in the fix
itself, and what it cost. §2a and §2b are left exactly as they were: the measurement was real, and
the repair is only legible next to what it repaired.

### Amendment, 2026-09-14 (#54): the channels are closed, and what each answer cost

§2a below records six channels **as they were measured before pruning existed**, and §2b the rule
they are held to. Both are kept as written rather than edited in place: the measurement was real,
and a reader who only sees the repaired state cannot tell which of §2b's two answers each channel
took or why. What follows is that, plus the one constraint that decided three of them.

**burrow cannot reach a destination's catalog.** `qpdf_get_root` is

```c
QTC::TC("qpdf", "qpdf-c called qpdf_get_root");
return trap_oh_errors<qpdf_oh>(qpdf, return_uninitialized(qpdf), ...);
```

— two top-level statements, so [ADR 0013](0013-qpdf-c-api-and-prescan.md) §1's caller rule refuses
it and it is not on `engines/qpdf-trapped-functions.txt`. Every `qpdf_oh_*` function that **is** on
that list keeps its `QTC::TC` *inside* the lambda; that is the whole difference.
`qpdf_get_trailer`, every `qpdf_oh_is_*` predicate and the dictionary-key iterator fail the same
way. So no split output can be given an `/OCProperties`, an `/AcroForm` or an `/Outlines`.

| channel | answer | why that one |
|---|---|---|
| inherited `/Resources` | **pruned** | §2b says it must be. Filtered to the names the copied content streams mention — the page's own, every kept Form XObject's, tiling pattern's, Type 3 `/CharProcs` and annotation appearance stream's, unioned to a fixpoint |
| `/AcroForm` field tree | **dropped** | every widget's `/Parent` is cut, so the field is unreferenced and the writer never emits it. §2b's second option. The catalog constraint makes this the *only* option: a kept field would be a field with no `/AcroForm`, which is a dead field either way, so the choice was between a dead field plus a leak and neither |
| shared `/Annots` | **pruned** | annotations whose `/P` is a page in this output are kept — and only when the array is shared with a page the output does **not** contain. See below: the first version keyed on "shared at all" and deleted every annotation from a split that excluded nothing |
| article beads | **dropped** | `/B` is simply not on the page-key allowlist, and `/Threads` lives on the catalog the build route never copies |
| named destinations | **pruned** | a link pointing at a page **in this output** survives and works; a named destination goes, because the name is the leak. See the amendment below — dropping both was over-broad, and measuring said so |
| `/OCProperties` | **refused** | neither §2b answer is reachable, and the binding constraint is **reading**, not writing — see the amendment below. So a document whose kept pages reference optional content is **not split**: a third answer, stronger than both, and §4 carries the sentence the page will say it in |

**The page-key rule is an allowlist, and that is the part worth carrying to M2.** Everything not on
a named list of page keys is removed, so `/B`, `/AA`, `/Thumb`, `/PieceInfo`, `/StructParents` and
page-level `/Metadata` go through one rule — along with every key nobody enumerated. §2b's table
below lists channels; the implementation does not, and the difference is the whole lesson of
*Alternatives considered*, applied one level down from where that section applies it.

Reading a dictionary's keys needs a tokeniser, because qpdf's key iterator is not callable: that is
`core/burrow-engines/src/pdfsyntax/`, pure Rust, `forbid(unsafe_code)`, two fuzz targets. So is the
resource-name scan, for the reason §2b gives — "this is not a dictionary filter".

#### Two defects the harness caught that no amount of reading would have

Both are recorded because both produced a *correct-looking* implementation.

1. **Pruning per page destroys what pages share.** `qpdf_add_page` flattens the page tree and
   pushes inherited attributes down ([ADR 0021](0021-how-reorder-permutes-a-page-tree.md)) — and it
   pushes the **reference**: every page under a node that carried `/Resources` ends up with
   `/Resources 11 0 R`, *the same object*. A per-page pass computed page 1's used names and deleted
   everything else, then handed page 4 a dictionary with its font already gone. Resources are now
   pruned once per dictionary against the union over every page in the output that shares it, and
   the annotation filter keeps anything belonging to **any** page in the output for the same
   reason. Caught by `a_one_way_split_loses_no_page_content` — a split excluding *nothing*.
2. **"Shared" is not the thing that makes an annotation ambiguous.** Sharing with a page the output
   does not contain is. The first filter keyed on the former and dropped every annotation in the
   document on a one-way split.

Neither is a subtle case. Both are what happens when the fix for a shared-object leak is itself
written per object rather than per output, and the second layer of ADR 0019 §3's harness is what
separated them from success.

#### The fixture could not have caught them as it stood, and that is three for three

`tools/make-marked-document.py` had three defects of the class `add-operation` §2c names, all
exposed by the same run:

- the resource "only page 4 draws" was **drawn by nobody**, so a filter that pruned it from every
  output looked identical to one that pruned it correctly;
- `widget4` was in the field's `/Kids` and in **no page's `/Annots`** — a widget no page displays,
  which no producer emits;
- no annotation carried `/P`, so only the conservative half of the filter was ever exercised.

And one that is not a defect but a consequence: a page dictionary can no longer carry a `/BM`
marker, because a marker *is* a key outside the specified set and removing exactly those keys is
the rule. Page objects are now declared `unmarkable`, with that reason, and their survival is
witnessed by their content streams.

#### Cost

`cargo run -p burrow-ops --features native-engines --release --example measure-pruning`, medians of
11, five-way splits, aarch64. "Before" is `2f8e989`, the commit this branch left; the example is
copied onto it, since there is no switch that turns pruning off and deliberately is not one.

| document | before | after | pruning's share |
|---|--:|--:|--:|
| `pages-10.pdf` (10 pages) | 117 µs | 147 µs | **+26%** |
| `pages-137.pdf` (137 pages) | 594 µs | 1.020 ms | **+72%** |
| `--generated 10000` (flat tree) | 43.3 ms | 91.6 ms | **+112%** |
| `--deep 10000 60` (60-deep tree) | 42.7 ms | 89.8 ms | **+110%** |

These are the numbers **after** the walk was rewritten for the five defects security review found
below; the first version measured +33% / +99% / +161% / +167%, and the difference is mostly the
per-object name cache that fix 3 required.

**What the percentages are against, since a bare percentage is a number with no denominator.**
Each is `(after - before) / before` where both terms are the **whole `burrow_ops::split` call** —
open, the sharing sweep, copy, prune, write, for **every part of a five-way split**. Not against
one part, not against the rest of the operation, and not against verification, which `split` does
not yet have. So "+112%" means a five-way split of a 10,000-page document takes 91.6 ms where it
took 43.3 ms, and pruning is the difference.

**They do not compose with ADR 0022's table, and the reason is not scale.** That table was measured
on **`rotate`**, one page rotated, so its percentages are fractions of a different operation. Adding
73.5% to 112% describes nothing. What composes is the milliseconds, and those are worth setting out
because `split` inherits the verification in the next change:

| for a 10,000-page document, five-way split | flat tree | 60-deep tree |
|---|--:|--:|
| split, before pruning | 43.3 ms | 42.7 ms |
| + pruning | 91.6 ms | 89.8 ms |
| + ADR 0022's promise sweep over the source | +5.5 ms | **+148.6 ms** |
| **measured total, with verification** | **123.0 ms** | **263.9 ms** |

**The last row was a projection and is now a measurement, and replacing it was worth doing
because one of the two numbers was badly wrong.** The projection added a whole-document read-back
to each column and got 122 ms and 406 ms. Flat, that was right to within 1%. Deep, it overstated
by **54%**.

The reason is structural and is the same fact ADR 0021 records from the other side: `qpdf_add_page`
**flattens the page tree**, so the parts a split emits have flat ones whatever the source had. The
promise sweep is over the *source* and pays the full depth — 148.6 ms, the largest single cost in
that column. The read-backs are over the *parts*, and walk no `/Parent` chain at all: ~25 ms, the
flat figure, not the 167.7 ms a whole-document deep read-back costs.

So a deep page tree is paid for **once**, on the way in, and not again on the way out. That is not
something the arithmetic of adding two measured rows could have produced, which is the argument for
measuring a composition rather than summing its parts.

**The shape is the part worth keeping either way.** On a flat tree pruning dominates and the
verification is modest; on a deep tree the promise sweep costs more than everything else combined,
because it walks `/Parent` per page while the prune runs after the tree has been flattened. Those
two costs are sensitive to different attacker-chosen numbers, which is why they are separate rows
rather than one.

**What each row does and does not exercise**, because the two largest ones do not support the
cause the first conclusion gives them. `pages-10.pdf` and `pages-137.pdf` are committed fixtures
with real content streams. `--generated` and `--deep` are built by
`core/burrow-engines/testsupport/measure_fixtures.rs`, and **their pages have no `/Contents` and no
resources at all** — so those two rows measure per-page `unparse`, key reading and key removal with
nothing to decode. That makes them the right rows for the depth conclusion and the wrong ones for
attributing the growth to stream decoding; the honest reading is that the per-page structural work
alone is already the larger part of the cost at ten thousand pages. Found by code review, which
also measured that the +161% row has no stream in it.

Two things follow, and the second is the one worth having measured.

1. **Pruning costs between a third and one-and-two-thirds of the split it is attached to**, growing
   with page count. On documents that have content it is decoding as well as reading structure —
   which is what finding out which resources a page uses costs, and §2b's "this is not a dictionary
   filter" is the reason there is no cheaper version. **Redaction inherits it.**
2. **Page-tree depth costs it nothing** — 89.8 ms at depth 60 against 91.6 ms flat, a 2%
   difference on a shape that costs ADR 0022's promise sweep **84%** of its operation. The reason
   is structural rather than lucky: the promise sweep walks `/Parent` per page, and the prune runs
   *after* `qpdf_add_page` has flattened the tree, so there is no chain left to walk. It was a
   prediction before it was a measurement, and it is recorded as the latter.

#### Five defects security review found in the pruning, all reproduced

The first version of the walk was written to close a leak and introduced four bugs of its own, three
of which were worse than what they fixed. They are recorded because each one is a shape the next
subsetting operation — redaction — will have the opportunity to repeat.

| | what | measured |
|---|---|---|
| **1** | the optional-content refusal read the **page's** `/Resources` only, so an OCG inside a Form XObject's own resources was past it | a document whose hidden layer lived one level down split happily, with the hidden text visible in the output and the layer's `/Name` still in it. §2a row 6, open, while its page-level test and near-miss both passed |
| **2** | the walk followed anything in `/XObject` that was a stream, so it tried to **decode images** | **every document containing a JPEG was refused** — lossy filters are not decoded at `qpdf_dl_specialized`, and the walk read "not decoded" as a refusal. A flate image decoded and then failed to lex whenever its pixels held an unbalanced `(` |
| **3** | no deadline, and the visited set and stream budget were **per page** rather than per output | a 155 kB document of 1,000 pages sharing one form: **176 seconds**, resident memory never above 65 MB — so `max_memory_bytes` never fired, and `split` consults its deadline only *between* outputs, which for a one-part split is never |
| **4** | `/Resources` was pruned by iterating seven **categories**, so any other key survived whole | `/Stash << /Secret … >>` on an inherited `/Resources` came through untouched while `/Font` was pruned correctly — §2a row 1 leaking through a key nobody enumerated |
| **5** | collected names were resolved against the **page's** categories only | a font drawn by a form two levels down was pruned off the page while the form still asked for it; the font object left the file entirely |

**Two of these are the same mistake as the one this ADR is about, made again one level down.**
Rows 1 and 4 are page-level thinking applied to a graph: the page-key rule is an allowlist *because*
a dictionary may hold keys nobody enumerated, and the resource filter was written as a denylist over
seven category names anyway. Row 5 is the same error in the other direction — a walk over the page's
dictionary rather than over the resource graph.

**Row 3 is the one worth carrying furthest.** The fix for a leak was itself an unbounded amount of
work, hidden from both ceilings: too fast to trip `max_memory_bytes`, and inside a granularity
`max_duration_ms` is not checked at. `prune_output` now takes the open's deadline and checkpoints per
page and per stream, and the name set is cached per object across the whole output — which removes
the amplification rather than merely detecting it, since a stream's names do not depend on which page
reached it. Verified: the 100-page shape now refuses at 1.003 s against a 1,000 ms ceiling, where it
previously ran 17.4 s with nothing consulted.

Each has a fixture and a regression test: `images.pdf`, `nested-forms.pdf`, `oc-nested.pdf`, and a
`/Stash` canary added to `shared-objects.pdf`'s inherited `/Resources` so the existing scan covers
row 4.

### Amendment, 2026-09-14 (#54, second pass): the three consequences, priced

Closing the leak produced three user-visible consequences larger than the outline loss §1 records,
and all three arrived as *side effects of an implementation* rather than as decisions. Each was
priced — prune correctly versus drop — before being left alone. One was cheap and was taken.

#### Links: **pruned**, and dropping them was over-broad

The rule was "remove `/A` and `/Dest` from every annotation", on the reasoning that a named
destination is a name and an explicit one points at a page the copier stopped at. **The second half
is wrong.** Splitting a four-page document at page 2, with two links on page 1:

| the link's destination | what the copy produced |
|---|---|
| page 2 — **in** this output | `[4 0 R /Fit]`, and object 4 **is** page 2 of the output |
| page 4 — excluded | `[11 0 R /Fit]`, and object 11 is `null` |

`qpdf_add_page`'s copier maps a reference to a page it copied and reserves a **null** for one it did
not. So a link into the output survives *and resolves*, and an outward one is already inert and
carries nothing from the page it named. Dropping the first was a fidelity loss with no privacy gain
— the trade this ADR exists to stop being made by accident, made by the fix for it.

What survives now is an allowlist of two: an explicit destination array whose first element is a
page in this output, and a `/URI` action with no `/Next`. A **named** destination goes, because the
name is the leak and the tree that would resolve it is on a catalog no output has. Every other
action type goes — `/SetOCGState` names optional content groups, `/GoToE` reaches into embedded
files, `/Named` and `/JavaScript` are open-ended — which is deliberate over-dropping, and an
allowlist of two is auditable where a denylist over PDF's action types would be a list of the ones
somebody thought of.

#### Form fields: **forced**, and the cost is dead metadata rather than anything a reader sees

Cutting every widget's `/Parent` is §2b's drop option, and the alternative — keeping the edge where
the field lies wholly inside the output — was priced and refused. Measured on a two-page form whose
widgets carry their own appearance streams, split at page 1:

- the surviving widget **keeps its `/AP`**, so it draws exactly as it did;
- page 2's `/V` and page 2's appearance are both **absent**.

What cutting `/Parent` costs is `/T`, `/V` and `/DA` — and those are unusable in a split output
whatever this module does, because a widget is a *field* only by virtue of the catalog's
`/AcroForm`, which no output can have. Keeping them for a wholly-contained field would retain three
keys nothing can act on, in exchange for a subtree analysis on the leakiest channel in §2a.

**"Fields are not fillable" is therefore forced by the catalog constraint, not chosen here.** The
choice this module makes is only whether the dead field's name and value ride along, and they do
not.

#### Optional content: **forced**, and the reason is narrower than first recorded

The amendment above says carrying a pruned `/OCProperties` "needs the catalog". That is true and it
is not the binding constraint, and the difference matters to whoever reads this next.

The **destination** catalog is reachable if one is willing to work for it: `BLANK_DOCUMENT` is our
own bytes, so it could carry an `/OCProperties` object referenced from both the catalog and the
blank page, and a handle taken from the page before the page is removed would reach it —
`qpdf_remove_page` erases from `/Kids` without destroying the object. `qpdf_oh_copy_foreign_object`
would then supply the dest twin of each source OCG, memoised, so the copies the pages already
reference are the ones the configuration would name.

**The source catalog is not reachable at all, and that is what forbids it.** Whether a layer is
hidden lives in `/OCProperties /D /OFF` on the *source's* catalog — PDF 32000-1 §8.11.4.3 puts it
nowhere else. An OCG dictionary does not say whether it is on. So burrow cannot find out what to
carry, and the two defaults are both wrong in the direction that matters: everything on reveals
what the source hid, everything off hides what it showed.

So the refusal is forced by an inability to **read**, not to write, and no amount of cleverness on
the destination side reaches it. Reopening it means reopening ADR 0013's caller rule, which is a
security-posture decision and not this ADR's to take.

#### What redaction inherits from all three

The shape, not the answers. Redaction removes content from a page rather than pages from a
document, so "does this still point at something the output contains" is the same question with a
different set — and the link rule is the one piece of this that transfers as written. The other two
are `split`-specific consequences of having no catalog, and redaction, which edits a document in
place rather than building a new one, will not have that constraint.

#### What transfers to M2, and what is `split`-only

| | where | redaction reuses |
|---|---|---|
| the PDF tokeniser, the resource-name scan, the dictionary-key reader | `core/burrow-engines/src/pdfsyntax/` | **unchanged** — no engine in it, no notion of a page |
| the prune driver and the six rules | `core/burrow-engines/src/qpdf/prune.rs` | **yes** — `prune_output` takes pages and a fact about each, not a page range |
| runs, cuts, the partition | `core/burrow-ops/src/split/` | no |

#### Issue #53 is not nearly free, and now for a concrete reason

Subsetting the outline means writing `/Outlines` onto the destination's catalog, which the constraint
at the top of this amendment puts out of reach. It stays a later change, and what it is blocked on is
`qpdf_get_root` rather than effort.

### 2a. What the build route still carries, measured

Six channels, found by security review and each reproduced independently before being recorded
here. Every one is reachable without adversarial input; several occur in documents ordinary
software produces.

| channel | what crosses | how |
|---|---|---|
| **Inherited `/Resources`** on the `/Pages` node | font programs, images, ICC profiles used **only** by excluded pages | the kept page inherits the dictionary, so the closure copies all of it |
| **Hierarchical `/AcroForm`** | the field's `/T`, **its `/V` — the value a person typed** — and every sibling widget's name | a kept page's widget has `/Parent`, and the field's `/Kids` reach widgets on excluded pages |
| **Shared `/Annots` array** | an excluded page's annotations | two pages referencing one array |
| **Article beads** (`/B`) | the thread's `/T` and `/I` info dictionary | bead → thread → info |
| **Named destinations** in a kept page's `/A /S /GoTo /D` | the destination *name*, which is often descriptive | inside the kept page's own annotation |
| **`/OCProperties` dropped while its OCGs are kept** | content hidden by an *off* layer becomes **visible**, and the layer's `/Name` survives | the configuration that turned it off lives on the catalog; the content does not |

The mechanism is one function: `qpdf_add_page` → `Pages::insert` → `Copier::reserve_objects`
(`engines/vendor/src/qpdf-12.4.1/libqpdf/QPDF.cc`), an unbounded deep traversal whose only
stopping rules are `/Pages` nodes and other page objects. Everything else reachable from a kept
page is deep-copied, whoever else owned it.

The AcroForm case is the one to hold in mind, because it is this section's own sentence
happening in the route this ADR chose. Reproduced on a two-page form of the shape Acrobat and
LibreOffice emit — one field, one widget per page:

```text
page-1-only output contains:
  LEAKCANARY-fieldgroup
  LEAKCANARY-value-typed-on-page-2
  LEAKCANARY-widget-page2
```

The output's catalog has no `/AcroForm`, so no viewer shows any of it, nothing on the page
suggests it, and the split looks perfect.

**Why the original canary test saw none of this.** It enumerated *features* — outline, field,
annotation, attachment — when what decides whether data crosses is *object sharing*.
`tools/make-split-fidelity-fixtures.py` gave every page its own `/Annots`, its own `/Resources`
and its own everything: precisely the structure in which this class cannot occur. Twenty
canaries, twenty measurements of a case that was never at risk. It is the third time in this
milestone that a fixture set has produced a confident measurement of nothing, and
`add-operation` §2c carries the pattern.

### 2b. Prune correctly, or drop entirely — never carry through wholesale

**The rule every channel in §2a is held to, and the bound on what closing them means.**

> For each thing an included page reaches that also belongs to excluded content, there are
> exactly two acceptable outcomes: **prune it correctly**, so what survives describes only what
> the output contains; or **drop it entirely**, and say so on the page. Carrying it through
> wholesale is not one of them.

Wholesale carry-through is the failure this whole ADR is about: it is the outcome that looks
like fidelity and is a disclosure. It is also the outcome that is cheapest to reach by
accident, because it is what an engine's copy does by default.

Dropping is a legitimate answer and not a lesser one. `split` already drops outlines wholesale
and the `/split-pdf` copy says so in §4 — a person told "bookmarks are not carried over, because
copying them would put the names of pages you did not include into files you did" has been
given something true they can act on. A person handed an outline full of entries pointing at
pages that are not there has been given something worse than nothing, and told nothing.

What this bounds, per channel:

| channel | prune correctly means | dropping means |
|---|---|---|
| inherited `/Resources` | keep the names **any copied stream** references — see below, this is not a dictionary filter | no resources, so a page that inherited its font renders wrong — **not acceptable here**, this one must be pruned |
| `/AcroForm` field tree | keep the field, drop `/Kids` on excluded pages, keep `/V` only if the field is wholly inside the output | drop the widget too, so the output has no form field rather than a dead one |
| shared `/Annots` | keep annotations whose `/P` is a page in this output | drop the array, losing the kept page's own annotations — acceptable only if said |
| article beads | keep beads on included pages and rebuild the thread | drop `/B` and `/Threads` |
| named destinations on a kept page | rewrite to a destination inside the output, or | drop the action, leaving a link that does nothing |
| `/OCProperties` | carry only the `/OCGs`, `/D /ON`, `/D /OFF` and `/D /Order` entries whose OCG objects were themselves copied | remove the OCGs' content — **dropping the configuration alone is forbidden**, because it makes hidden content visible |

**Two of those rows hide more work than one line can hold, and saying so is the point of a
bound.**

*Inherited `/Resources` is not a dictionary filter.* A resource name is referenced from more
places than the page's own content stream: a nested Form XObject's stream, a tiling pattern's
content, a Type 3 font's `/CharProcs`, an annotation's appearance stream, and `BDC /OC /Name`
marked-content operators. Miss one and the choice is between breaking the render and keeping
the resource — and keeping it is the leak. Doing it properly means **a content-stream parser on
attacker-controlled bytes**, inside a crate that has none today and is `forbid(unsafe_code)` for
good reason. Issue #54 carries that as a design consequence rather than a detail.

*Carrying `/OCProperties` wholesale re-creates the leak it fixes.* The catalog's copy names every
layer in the source, including layers only excluded pages use — which is §2a row 6's second
clause arriving through the repair. Only the entries whose OCG objects were themselves copied
may come across.

Two rows say the choice is not free. Inherited resources cannot simply be dropped without
breaking the page, and `/OCProperties` cannot be dropped alone without turning a
confidentiality failure into a *different* confidentiality failure. Everywhere else, dropping
with a sentence on the page is a complete answer, and issue #54 may take it.

### 3. The check that makes it real

**The primary check is structural, not a canary list.** `tools/make-marked-document.py` marks
**every object** in a source document and declares which pages each belongs to;
`core/burrow-engines/testsupport/object_closure.rs` asserts that every object surviving in an
output belongs to a page that output contains. That fails for categories nobody named, which is
the failure mode a canary list does not have — a canary list's failure mode is that it passes.
The harness is shared rather than `split`-specific, and the operations that are *not* subsetting
call a different entry point on it — `assert_nothing_lost`, which names what must be there.
Asking `assert_closed` with every page included answers nothing: every owned object belongs to
an included page, so the trespasser list is empty whatever the operation did.

The named channels remain **as regression cases on top**, because the structural check has one
blind spot it cannot close: a leak *inside* an object that legitimately survives. A form field
owned by pages 1 and 4 is allowed into an output containing page 1, and it carries the `/V`
somebody typed on page 4. For that, `tools/make-split-fidelity-fixtures.py` gives every page's
outline title, field name, annotation text and attachment name a **unique canary** naming its own page. The test splits,
decompresses the emitted bytes — `qpdf --qdf`, because a canary inside a compressed stream is
invisible to a naive search — and asserts that **no canary belonging to an excluded page
appears anywhere in the output**.

It ships with two controls. `the_scan_finds_a_canary_that_is_really_there` runs the identical
scan over a document that contains every canary and requires it to report a leak, and
`the_included_pages_canaries_do_survive` requires the kept pages' own canaries to be present —
without which an operation emitting an empty document would satisfy the negative perfectly.

**The deliberate-leak control was `the_measured_leak_channels_are_exactly_the_ones_recorded`**,
which asserted that §2a's channels *did* fire. It was an unusual shape for a control and it was the
honest one while the rule was unmet: it failed if a channel closed without the record being updated,
and it failed if the scan went blind — two things that otherwise look identical from a green run.

**When #54 closed it, that test inverted rather than disappearing**, and so did its structural twin
`the_closure_property_is_violated_exactly_where_the_adr_records_it`. Deleting them was the obvious
move and the wrong one: the state they were distinguishing has not gone anywhere. A changed canary
prefix, a fixture whose markers moved, a manifest the reader stopped understanding, an expansion
that silently stopped decompressing — each still produces an empty leak list, and now that empty
list reads as success rather than as failure. So each was replaced by the same assertion pointed at
a document that definitely leaks: `the_scan_can_still_see_every_channel_it_is_the_gate_for` runs the
identical scan over the source and requires **every** channel by name, and
`the_closure_scan_can_still_find_a_trespasser_that_is_really_there` does the same for the structural
harness, gated on the manifest's own object count rather than on non-zero.

Two channels had no fixture and no test when this was written, and both now do.
**Named destinations** (row 5) are a canary in a link on a *kept* page — the leak rides inside an
object that legitimately survives, so the structural harness cannot see it by construction, which is
the clearest example of why ADR 0019 needs both layers and why neither is sufficient.
**`/OCProperties`** (row 6) is the one a byte scan is the wrong instrument for entirely: it is not a
string arriving where it should not, it is content becoming *visible*. Its fixture is
`optional-content.pdf` and its test is a **refusal** — `a_document_that_uses_layers_is_refused_rather_than_split`,
with `a_document_without_layers_is_not_caught_by_the_layer_refusal` as the near-miss, because a
refusal that fired on everything would close the channel perfectly and make `split` useless.

### 4. The page says what is dropped, in the voice `/merge-pdf` says what it keeps

`/merge-pdf` has a section headed *"What is kept, and what is not"* which states plainly that
bookmarks, attachments and form fields come from the first file and that merging outlines from
several documents has no obviously right answer, so burrow does not guess at one. `/split-pdf`
owes the same paragraph from the other direction, and it is written here so the page has something
to match rather than something to invent:

> **What is kept, and what is not.** Each part contains its pages exactly as they were — their
> contents, their size and their annotations. **Links keep working when they point inside the same
> part**, and links to a page in a different part are removed rather than left to go nowhere.
> Web links are kept.
>
> Form fields keep their appearance but are no longer fillable. What makes a field a field is
> recorded for the document as a whole, not on the page, so it cannot come with one part — and
> carrying it would carry the names and the values typed on pages that part does not contain.
> Bookmarks and attached files are left out for the same reason: they describe the whole document,
> and copying them into every part would put the names of pages you did not include into files you
> did.
>
> A document that uses layers cannot be split here at all. Whether a layer is hidden is recorded
> for the document as a whole, so burrow has no way to find out — and a hidden layer that arrived
> visible would be worse than a refusal.

**This version is not provisional, and it has already been corrected once.** The original said
"their contents, their size, their form fields and their annotations", marked provisional on #54
because §2b offered dropping the widget as a legitimate outcome — and #54 took it. The first
rewrite then said flatly that "links to somewhere else in the document stop working", which was
true of the implementation and not of what the implementation *should* do: the second-pass
amendment measured that a link into the same part survives and resolves, so that sentence
overstated the loss and the code was changed rather than the copy.

Three of the sentences above carry their reason, and the reasons are not decoration. "Fields are
not fillable" reads as a bug without "recorded for the document as a whole"; "layered documents are
refused" reads as a missing feature without "burrow has no way to find out". The point of each is
that the person affected can tell a deliberate limit from a defect.

**The reasons are the load-bearing part and they are not decoration.** A page that said only
"bookmarks are not kept" would be true and would leave somebody thinking it was a limitation rather
than a choice. A page that said only "documents with layers cannot be split" would read as a bug.

**There is no `/split-pdf` yet**, and every present-tense claim in this repository that the page
says any of this is describing a page that does not exist. The route is kept out of the shipped
bundle by `apps/web/src/production-build.test.ts`, which is a check rather than an intention, and
this section is what the page will be held to when it is written.

## Consequences

- Outlines are dropped from every split output, including entries that point only at pages the
  output contains. That is a real fidelity loss and it is the conservative direction: keeping
  the entries that survive means rewriting the outline tree, which is its own change. Filed as
  an issue rather than smuggled in here.
- **Catalog-level attachments are dropped, and this is a real loss against carve**, which keeps
  them. The first version of this ADR said carve lost them too and that nothing was given up —
  that was the broken decompression talking, and it is corrected here rather than quietly. The
  trade stands anyway: an attachment belongs to the document, so a five-way split would put
  five copies of it in five places, and there is no reading of "which part does the attachment
  belong to" that is obviously right. What decided it was the outline leak, not this.
- The blank destination is **our bytes**, parsed through the trapped `qpdf_read_memory`.
  That is the whole reason it exists; if a future qpdf makes `qpdf_empty_pdf` trapped, this can
  be revisited, and the ADR should be amended rather than the constant quietly replaced.
- **The subsetting rule is now in `.claude/skills/add-operation/SKILL.md`**, so the next
  operation meets it at design time rather than at review.
- M2's redaction work inherits a measured example of the failure R8–R10 describe, in a file
  anybody can generate and run.

## Alternatives considered

**Carve, and strip the catalog afterwards.** Removing `/Outlines`, `/Names` and `/AcroForm`
from a carved output would close the measured outline leak — and it is a denylist, on a
structure whose whole design is that a dictionary may contain keys nobody enumerated.

Build is better and **it is not the allowlist this ADR originally called it.** That sentence
said "nothing is in the output unless it was copied in", which is true and useless: what gets
copied in is a *reachability closure*, not a list. §2a records the six channels that survive it,
measured. Build is still the right choice — it closes the catalog-level leaks carve cannot, and
carve leaks everything build does *plus* the whole catalog — but the difference between the two
is smaller than the first version of this document claimed.

**`qpdf_empty_pdf` with an argued untrapped entry.** Rejected on the evidence above; the
argument would have to be false.

**Rewrite the outline to keep surviving entries.** The right long-term answer and not this
change. It needs object-level surgery through entry points we do not have declared, and doing
it badly produces exactly the half-correct structure this ADR is about.

---

## Amendment, 2026-09-17 (#112): the image fix closed an instance, not a class

**A valid 11 MB document refused to split, and told the person their file might be damaged.**
It was an ordinary course PDF: qpdf opened it, counted 370 pages, and `compress` rewrote the
whole of it without complaint. Only `split` failed, at every cut point, with a typed error the
interface never showed:

```
Malformed("pdf syntax: a ')' with no string to close")
```

That is **burrow's own lexer**, and the byte it refused on was inside a **CFF font program** —
`FJPBJE+BodegaSans-Black`, one of 37 Type1C fonts in the file. The resource walk in §2b had
followed `/Font` → the font dictionary → `/FontDescriptor` → `/FontFile3` and lexed a font as
though it were page content.

### The part worth recording is that this had already been found once

Security review found the identical mechanism in `/XObject`: the first version followed anything
that was a stream, so a `/DCTDecode` image refused to decode and a `/FlateDecode` image's pixels
"failed to lex as PDF syntax roughly whenever they contained an unbalanced `(`" — on **every**
JPEG-bearing document. The fix gated `/XObject` on `/Subtype /Form`, and the code carries a long
comment explaining why an image is not worth following.

**That fix closed the key it was found on.** It did not close the thing that made the key
dangerous: a `Follow::Anything` mode, which existed for Type 3 `/CharProcs`, and which the
`DICTIONARY` arm of the walk inherited into *every* key of *every* dictionary it descended. So
the same defect was still reachable one key along — through `/Font`, which was also declared
`Anything` — and it stayed reachable for as long as no fixture had an embedded font.

**The pattern, stated so it is checkable next time: when a review finds a mode too permissive
for one key, the question is which OTHER keys inherit that mode, not whether that key is now
handled.** The narrow fix is the one that gets written, because the failing input names one key;
the class lives in the mode.

So `Follow::Anything` is **gone**, not narrowed. What replaced it says where a stream was
established to be content:

| mode | follows | reached from |
|---|---|---|
| `FormsOnly` | `/Subtype /Form` only | `/XObject` |
| `TilingOnly` | `/PatternType 1` only | `/Pattern` |
| `ContentStream` | any stream — it **is** content | a Type 3 `/CharProcs` entry, an annotation `/AP` |
| `FontDictionary` | **no streams at all**; dictionaries only as far as `/CharProcs` | `/Font` |

A font's own streams — the program, its `/ToUnicode` CMap, its `/Metadata` — are not content and
cannot name a resource, so nothing is lost by refusing to read them.

### Why the corpus did not have this, and what was added

**Every fixture in `tests/conformance/` is synthesised, and a synthesised font is a dictionary
rather than a program.** Twenty-two cases, none with an embedded subset font — which is why a
real document found this in minutes. Two fixtures now exist, both generated by
`tools/make-embedded-font-fixture.py` and both run through **every** operation rather than
through `split` alone:

* `font-program-with-a-paren.pdf` (1,263 bytes) — a `/FontFile3` whose bytes contain a bare `)`.
  Verified to refuse without the fix and split with it. Its test also asserts the `)` is still
  there with no `(` before it, so the fixture cannot be tidied into one that proves nothing.
* `type3-glyph-draws-an-xobject.pdf` — the opposite hazard. `/Im1` is named **only** inside a
  Type 3 glyph procedure and lives in the *page's* `/Resources`, where the policy can delete it.
  If the fix over-corrects and stops descending font dictionaries, the XObject is pruned and the
  page comes out drawing a glyph whose picture is gone — a valid PDF, silently missing its
  content. Verified failing under exactly that mutation.

**The first version of that second test was worthless**, and it is recorded because the failure
is the instructive kind: `/Im1` was put in the *font's* own `/Resources`, which the policy never
prunes, so the test passed under the over-correction it existed to catch.

Running both fixtures through every operation turned up nothing else: `page_count`,
`structure_check`, `rotate`, `reorder` and `compress` all accept them unchanged. Only `split`
ever failed, because only `split` tokenises.
