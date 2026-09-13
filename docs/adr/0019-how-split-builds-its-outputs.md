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

**And the rule is not yet met.** §2a records six channels through which the chosen route still
carries data from excluded pages, all measured. This section states the standard; it does not
claim `split` reaches it. Saying otherwise here would be the exact failure the section is
about — an operation's intent and its output disagreeing, written down as if they agreed.

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

**The deliberate-leak control is `the_measured_leak_channels_are_exactly_the_ones_recorded`**,
which asserts that §2a's channels *do* fire — **two of the six by name**, and the count is
stated rather than implied. The fixture plants four mechanisms and all four fire, but only the
inherited-`/Resources` and `/AcroForm` canaries are required individually, so closing the
shared-`/Annots` or article-bead channel would not fail anything. `shared-objects.pdf` plants inherited `/Resources`, the `/AcroForm` tree,
a shared `/Annots` array and an article thread. **Named destinations (row 5) and
`/OCProperties` (row 6) have no fixture and no test**, and row 6 is the one that matters most:
it is not a string arriving where it should not, it is content becoming *visible*, which a byte
scan is the wrong instrument for. Closing that needs a structural assertion of a different
shape — an output containing an OCG dictionary must have an `/OCProperties` — and it is on
issue #54 rather than pretended here. It is an unusual shape for a control and it is the
honest one while the rule is unmet: it fails if a channel closes without the record being
updated, and it fails if the scan goes blind — two things that otherwise look identical. Without it, a scan that had stopped finding anything would report the same
green as a scan that works — and that is the failure mode the root `CLAUDE.md` catalogues
three separate instances of.

### 4. The page says what is dropped, in the voice `/merge-pdf` says what it keeps

`/merge-pdf` has a section headed *"What is kept, and what is not"* which states plainly that
bookmarks, attachments and form fields come from the first file and that merging outlines from
several documents has no obviously right answer, so burrow does not guess at one. `/split-pdf`
owes the same paragraph from the other direction, and it is written here so the page has
something to match rather than something to invent:

> **What is kept, and what is not.** Each part contains its pages exactly as they were —
> their contents, their size, their form fields and their annotations. Bookmarks and attached
> files are not carried over. They describe the whole document rather than any one part, and
> copying them into every part would put the names of pages you did not include into files
> you did — so burrow leaves them out rather than guessing which part they belong to.

**This wording is provisional on issue #54.** §2b offers "drop the widget too, so the output
has no form field rather than a dead one" and "drop the array, losing the kept page's own
annotations" as legitimate outcomes — and if #54 takes either, the sentence above stops being
true. It is written here so the page has something to match, not so it can be pasted before the
decision that determines whether it is accurate.

The last clause is the one that matters and it is not decoration: it is the measured reason,
said to the person it affects. A page that said only "bookmarks are not kept" would be true and
would leave somebody thinking it was a limitation rather than a choice.

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
