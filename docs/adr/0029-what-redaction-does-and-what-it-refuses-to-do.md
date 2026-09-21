# 0029. What redaction does, what it refuses to do, and what "verified" is allowed to mean

Date: 2026-09-19

## Status

**Accepted**, 2026-09-21. Written from [spike 0006](../spikes/0006-what-survives-a-redaction.md) and merged as *Proposed*; accepted after §§3, 5, 6 and 7 were read, as with the ADRs before it. No M2 implementation started before this line changed.

Scoped to **decisions**, not design: what v1 does and refuses per channel, what the page says,
what verification may assert, and the named conditions for revisiting. The operation's
construction is `.claude/skills/add-operation`'s job and is deliberately not here.

Extends [0022](0022-every-operation-verifies-its-own-output.md) with redaction's content
predicate, which that record left as M2's whole problem. Applies
[0019](0019-how-split-builds-its-outputs.md) §2 to an operation that excludes no page. Carries
[0006](0006-wasm-linking-strategy.md)'s R8, R9 and R10 unchanged.

## Context

Redaction is the operation `CLAUDE.md`'s fourth non-negotiable singles out: a bug here leaks the
thing the user was removing. Until spike 0006 nobody had measured **where the text of a page can
also be**, so every plan for M2 rested on an enumeration nobody had checked.

The spike built one fixture per place and ran four instruments over each, in three states. Three
of its results decide this record.

**A naive "remove the glyphs from the content stream" redaction leaves the secret in the file in
17 of 23 places.** Three of those are plainly legible on the page afterwards. Removing the
text-showing operator is not redaction; it is the beginning of one.

**Five channels are legible to a person and invisible to every permitted scan *before* anything
is redacted.** Positioned glyph runs, a CID font with no `/ToUnicode`, text drawn as filled
paths, text inside a JPEG, and a `/ToUnicode` that lies. A check that cannot establish the secret
was there cannot report that it went.

**A subset font keeps a description of the text after the text is gone.** `/ToUnicode` written
per position spells the run out in order — measured, complete, in a file whose page content had
been correctly emptied. `/Differences` names its alphabet. The embedded font's own `cmap` covers
exactly the characters the subset was built for. This is the class
[0022](0022-every-operation-verifies-its-own-output.md), `prune/mod.rs` and `object_closure.rs`
have each warned about independently since M1 — *"for redaction that transformed form **is** the
leak"* — and the spike's first draft measured past all four fixtures that exhibit it, because
every instrument it had was looking for the secret as **text**.

That last one is why this record exists in the shape it does. **The question "where can text
survive" has a different answer from "what does the apparatus that drew it retain",** and only
the first had ever been asked.

### The constraint that shapes every decision below

`qpdf_get_root` and `qpdf_get_trailer` both fail [ADR 0013](0013-qpdf-c-api-and-prescan.md) §1's
caller rule, and `core/burrow-engines/src/qpdf/ffi.rs` already records them under *"Two functions
burrow wanted and may NOT have, recorded so nobody looks again"*. So **six document-level carriers
are unreachable**:

| carrier | lives on | how we know |
|---|---|---|
| `/StructTreeRoot` | the catalogue | **measured** — spike 0006 finding 4 asked a page handle for each and got `null` |
| `/AcroForm` | the catalogue | **measured** |
| `/OCProperties` | the catalogue | **measured** |
| `/Metadata` (document XMP) | the catalogue | **measured** |
| `/Names` → `/EmbeddedFiles` | the catalogue | **measured** |
| `/Info` (title, keywords) | the trailer | **not probed.** It rests on `qpdf_get_trailer` being refused by the same caller rule, which `ffi.rs` records. Stated as inference rather than measurement, because five of these six were probed and this one was not |

`/Annots` and `/Thumb` are on the **page** and are reachable. That line is the whole difference
between what v1 handles and what it refuses or discloses.

## Decision

### 1. v1 rewrites page content streams, the streams they reach, and the font mappings that describe them

The page's content is the one place burrow can read, edit, write and re-read entirely through
functions on `engines/qpdf-trapped-functions.txt`, on both platforms, with no new engine and no
cross-heap step.

It follows content into **Form XObjects** and **Type 3 `/CharProcs`**, because those are ordinary
shapes and the spike measured a Form XObject's text surviving fully legible. The resource-graph
walk that reaches them already exists in `core/burrow-engines/src/prune/`.

**And it edits the font objects.** For every character code the output no longer draws, its
`/ToUnicode` `bfchar` entry and its `/Differences` entry go. Both are reachable; both are
editable; leaving them is leaving a description of the text that was removed. This is a decision
rather than a detail because the first draft of the spike's recommendation did not contain it.

`qpdf_oh_replace_stream_data` is the write verb. It is trapped, is declared nowhere in `core/`
today, and rewrote all 23 fixtures with every output passing `qpdf --check`.

### 2. v1 strips every page key outside the allowlist

Everything outside `KEPT_PAGE_KEYS` is removed from a page that was redacted — page `/Metadata`,
`/PieceInfo`, `/AA`, `/B`, `/StructParents`, `/Thumb`, **and every key nobody enumerated.**

An allowlist rather than a list of things to delete, which is ADR 0019's own lesson one level
down: `split`'s first resource filter enumerated seven categories and a leak came through
`/Stash`, a key nobody had thought of.

**A strip we can do beats a warning the user must act on.** The spike measured `split`'s
allowlist removing page-level `/Metadata` and `/PieceInfo`, and — this is the load-bearing part
— that removal comes from a **page-side rule**, not from `split`'s build route failing to copy a
catalogue. Three of `split`'s four measured removals are the latter and **do not transfer** to an
operation that edits in place. This one does.

It also removes more than metadata, and that is stated rather than inherited: `/Thumb` is a
survival channel in its own right, and `/B` and `/AA` can carry text. Mostly what redaction
wants; still a behaviour change.

### 3. Per channel: what v1 does

The complete assignment. Every channel spike 0006 measured is here, with no "not applicable"
rows — a channel with no bucket is how the spike's own bar caught two omissions.

| channel | v1 |
|---|---|
| content-stream text: plain, kerned `TJ`, positioned runs, `/Differences`, CID with or without `/ToUnicode` | **handle** — rewrite the stream |
| text in a **Form XObject** | **handle** — follow the resource graph |
| text in a **Type 3 `/CharProcs`** | **handle** — same walk |
| text straddling **two `/Contents` streams** | **handle** — see §4 |
| `/ActualText` and `/Alt` on marked content in the region | **handle** — it is in the page's own stream |
| **annotations** whose `/Rect` intersects the region | **handle** — `/Annots` is on the page. Remove the annotation entirely; pruning its `/Contents` and keeping its appearance is two chances to miss one |
| the page's **`/Thumb`** | **handle** — strip unconditionally on any redacted page |
| page **`/Metadata`**, **`/PieceInfo`**, and every unlisted page key | **handle** — §2's allowlist |
| the font's **`/ToUnicode`** and **`/Differences`** | **handle** — §1 |
| **optional content** referenced by a kept page | **refuse** |
| a region intersecting an **image** | **refuse** — see §5, the signal is owed |
| a region intersecting **vector path content** | **refuse** — see §5, the signal is owed |
| a document with an **`/AcroForm`** | **refuse** — see §5, the signal is owed |
| a document with a **`/StructTreeRoot`** reaching the region | **refuse** — see §5, the signal is owed |
| catalogue **`/Metadata`** and the trailer's **`/Info`** | **disclose** |
| **`/EmbeddedFiles`** attachments | **disclose** |
| the embedded font program's own **`cmap`** | **disclose** — see §7 |
| **incremental-update history** | **nothing** — qpdf's writer emits only objects reachable from the current trailer, so the superseded object is gone. See *Consequences* for how narrow this claim is |

### 4. Reading `/Contents` per stream is wrong, and the fixture that proves it is committed

`qpdf_oh_get_page_content_data` returns a page's content streams **concatenated**, so the
identity of the individual streams is already gone by the time there is anything to edit. The
tempting fix — read each array element separately with `qpdf_oh_get_stream_data` — is wrong: per
the specification the array concatenates into **one lexical stream**, so element-wise tokenising
puts `BT` in one parse and `ET` in another and neither parse holds a complete text object.

**Tokenise the concatenation, keep a span map back to (stream index, offset), write each stream
from its own slice.** That is also what stops the operation silently collapsing a `/Contents`
array into one stream, which the spike measured it doing.

### 5. Four of the five refusals have no adequate page-side signal yet, and that is work this record owes

**A refusal keyed on something the operation cannot see is a bypass.** This is the sharpest open
item in the spike and it is recorded here as a debt rather than a limitation, because the
difference matters: a limitation is something a reader should accept, and this is something
somebody has to go and do.

**Corrected before this record was accepted.** The first version's heading said three refusals
lacked a signal while its table marked only two, and the table was the more wrong of the two: it
called the image and vector-path signals *measured* because the redactor tokenises the page's
content stream. **That is a page-content-stream signal, and this section's own rule says a
signal that cannot see the thing it refuses is a bypass.** An image inside a Form XObject is not
in the page's content stream — spike 0006 channel 7 measured exactly that for text and there is
no reason it is different for an image. So neither number was right: **one is measured, four are
owed.**

| refusal | the signal it fires on | status |
|---|---|---|
| optional content | a page's resources reference an OCG, **followed transitively over the resource graph** | **MEASURED, and to this section's bar.** `prune/mod.rs` calls `refuse_optional_content_in` from inside `follow_resources`, so it descends nested forms and Type 3 `/CharProcs`; ADR 0019's 2026-09-14 amendment records the defect where it read the page's `/Resources` only, and the fix. The evade fixture exists and passes: `oc-nested.pdf`, an OCG one level down inside a form's own resources, and `split_no_leak.rs::a_layer_one_level_down_is_refused_like_one_on_the_page`. **One level is what is measured**; deeper nesting is walked by the code and not pinned by a fixture |
| an image in the region | the page's own content stream | **OWED.** A `Do` of an image inside a **Form XObject**, an **inline `BI`…`ID`…`EI`** image, and an image reached as a **tiling pattern** or a **shading** fill are all past a page-content-stream signal. Spike 0006's channel 20 draws its image directly on the page, so nothing here has been measured against an evasion |
| vector paths in the region | the page's own content stream | **OWED.** Same shape: paths inside a **Form XObject**, and inside a **Type 3 glyph procedure**, are past it. Channel 18 draws its paths directly on the page |
| `/AcroForm` | proposed: an `/Annots` entry with `/Subtype /Widget` | **OWED.** A field whose widget sits on a *different* page walks straight through |
| `/StructTreeRoot` | proposed: the page's `/StructParents` | **OWED.** A `/StructElem` reaching this page's MCIDs without the page carrying `/StructParents` walks straight through |

**The four are owed in two different ways, and the remedies differ.**

- **Under-scoped signal** — image, vector paths. A signal exists and reads too little. The fix is
  to key the refusal on the same **resource-graph walk** the optional-content refusal already
  uses, rather than on the page's content stream alone. That walk is written, bounded and
  deadline-checkpointed; this is reuse, not new machinery.
- **No signal at all** — `/AcroForm`, `/StructTreeRoot`. The carrier is on the catalogue and
  §*The constraint that shapes every decision below* measures it unreachable. A page-side proxy
  has to be invented, and then shown to work.

**No refusal may ship on its signal until that signal has been measured to fire on a fixture
built to evade it** — tracked as [#125](https://github.com/TensorGreed/burrow/issues/125), which
carries a fixture list per refusal — and that applies to all five, including the one marked measured, whose
fixture pins one level of nesting and not the depths its code walks. That is the same bar every
other check in this repository is held to — a rule that matches nothing passes everything — and
it is a condition of this decision rather than advice.

### 6. What verification asserts, and what it may not be worded as

ADR 0022's frame holds: one value, emitted only on success, verified on the bytes emitted, read
back through a fresh engine. What redaction adds is the content predicate that record left open.

**It may assert:**

- the output is a document, with the input's page count and `/Rotate` vector — 0022 unchanged;
- **no text-showing operator remains whose glyphs fall inside the region**, re-derived from the
  emitted bytes through a fresh parse;
- **no `/ToUnicode` or `/Differences` entry remains for a code the output no longer draws**;
- **the named page carriers are absent**: `/Thumb`, every annotation whose `/Rect` intersects the
  region, and every page key outside the allowlist;
- **the document contained no shape the operation refuses** — subject to §5.

**It may not assert, and may not be worded as though it could:**

- **that the secret is absent.** On five channels no permitted instrument can confirm it was ever
  present;
- **that the embedded font no longer describes the removed run** — §7;
- **that the region is visually blank.** The spike measured ink 1.0000 on a black bar with
  nothing readable under it and 0.1361 on a page where the secret was fully legible. **The number
  is higher where nothing can be read.** Ink is not legibility;
- that text outside the region is untouched, beyond the page count and rotation vector.

**The variant is named for what it checks, not for what a reader hopes it checks.**
`Expected::Redacted` would be read as *the secret is gone* by everybody who has not read spike
0006. The name states the rule — the region was cleared — and its rustdoc states the residue in
the words above.

**#111 is sharper here than anywhere else, and this record does not close it.** For a page count
the promise is the operation's own reading of its input. For redaction the same structure applies
to *where the text is*, and the source of that answer is `/ToUnicode` and `/ActualText` — **written
by the producer of the document being redacted.** A page count is the engine's opinion about a
file; a glyph-to-character mapping is the *file's* opinion about itself. The spike measured a
document whose `/ToUnicode` lies while the same font's `cmap` tells the truth: two halves of one
object disagreeing, neither authoritative.

**And #111's shape appears a second time, in geometry rather than in mapping.** *"No glyph remains
inside the region"* is the strongest assertion in the list above, and it is computed by
**re-deriving glyph positions with the same geometry code the operation used to decide what to
remove**. A bug in that code is therefore invisible to the check that is supposed to catch it:
the operation removes the wrong glyphs and the verification agrees they are gone, because both
asked the same wrong question. The places it can go wrong are ordinary, not exotic —

- `Tz` horizontal scaling and `Ts` rise;
- `Tc` and `Tw`, character and word spacing, which accumulate along a run;
- the CTM through `q`/`Q` nesting, and again through a Form XObject's `/Matrix`;
- a Type 3 font's `/FontMatrix`, where glyph space is whatever the font says it is.

Each of those moves where a glyph *is* without changing what it *says*, so nothing in the
`/ToUnicode` half of this section touches it.

**The oracle for it is `FPDFText_GetCharBox`, at TEST time, in the native suite.** For a fixture
whose glyph positions are known, assert burrow's derived boxes against PDFium's for every
character on the page. It is the independent second reading #111 asks for, and it is available
here for three reasons the runtime check cannot claim:

| | |
|---|---|
| **it is geometry, not mapping** | a lying `/ToUnicode` does not defeat it. PDFium computes a box from the text state and the font's metrics, so a document can misdescribe what its glyphs mean without moving them |
| **it is test-only** | no payload cost, no bridge, no wasm export, and no collision with `tools/check-pdfium-is-render-only.sh` — which is a claim about what a *visitor downloads*, not about what a native test links |
| **it is already linked** | the native test binary has `libpdfium.so`; spike 0006 called this exact function |

**It is an oracle for the geometry code, not a component of the operation.** Putting it in the
shipped path would reintroduce every objection *Alternatives considered* raises against
`FPDFText_*`, and would make the operation depend on the engine this project deliberately keeps
render-only. The test asserts burrow's geometry agrees with a second implementation on documents
where both are asked; the runtime check continues to assert burrow's own rule against burrow's
own reading, and that residue stays exactly as stated above.

### 7. The page says what is left, and names where to look

In the voice [ADR 0019](0019-how-split-builds-its-outputs.md) §4 established, where every limit
carries its reason so a person can tell a deliberate limit from a defect. **A person looking at a
redacted page cannot see metadata**, so a disclosure they cannot act on is a caveat:

> **What is removed, and what is not.** The text you selected is removed from the page, from the
> file, and from the font's record of what it said. Anything the page itself carried about that
> text — its thumbnail, its accessibility text, the private notes some editors attach to a page —
> goes with it.
>
> **Three things are outside a page and are not changed: the document's title and keywords, any
> files attached to it, and the document's own XMP metadata.** Those describe the whole document
> rather than any page, and Not Only PDF has no way to reach them — so if the text you removed also
> appears in the title, in an attached file, or in the properties your editor wrote when it
> saved, it is still in this file. **Check File → Properties, and check the attachments panel.**
> If either holds what you were removing, the fix is in the program you made the document with,
> not here.
>
> **A font that was used only for the text you removed still says which letters that text used.**
> Not the words, and not the order — the set of characters. Rebuilding a font is a thing Not Only
> PDF cannot do yet, so this is stated rather than fixed.
>
> Not Only PDF refuses documents it cannot redact safely rather than doing its best, and says
> which shape it found. **And it verifies that it did what it said, not that your secret is
> gone:** it cannot read a secret drawn as a picture or as shapes, and it cannot tell whether a
> font is telling the truth about what its glyphs say.

**The copy says "Not Only PDF", never "burrow".** `burrow` is the repository and the crates;
the product a person is using is Not Only PDF, and `apps/web/src/pages/` says so on every page it
serves. ADR 0019 §4 drafted its copy with "burrow" and `split-pdf.astro` ships it with "Not Only
PDF" — the page was right and the record was not, so this one is written the way it will be
served. The rest of this ADR says "burrow", because the rest of it is about the codebase.

Three sentences there carry a reason and one carries an instruction, and both kinds are the
point. *"The title is not changed"* reads as a bug without *"those describe the whole document"*.
A person told only that metadata "may remain" has been given something true they cannot use; a
person told **File → Properties and the attachments panel** has been given somewhere to look.

### 8. The project rule: a removal nothing observed is not a measured removal

**Generalised from spike 0006's control 5, and it is here rather than in the spike because it is
not about redaction.**

> Where a check reports that something is **gone**, it must also show that something **changed**.
> A verdict of absence computed only from instruments that found nothing is a verdict with no
> evidence under it.

Every control this repository already has checks that something was **found**: the non-vacuity
control (`add-operation` §2c — a fixture that cannot fail the test is not a fixture), the
per-rule probes (a rule that matches nothing passes everything), the inertness controls, the
positive controls in `check-pdfium-is-render-only.sh`. All of them guard the **presence** side.

Spike 0006 measured what the absence side costs. Its harness scored a channel "survives" as a
disjunction of witnesses — any scan, or a human verdict, or a probe — so a channel where every
instrument was blind **and** the human wrote "no" scored *not in the file* on no evidence
whatever. **Four controls were in place and none could catch it**: non-vacuity keys on the input,
where a raw scan fires on uncompressed generator output, and a no-op mutation leaves both states
equally blind, so they agree.

An entire survival channel sat behind that gap — the font's own record of the removed text — and
the headline moved from 12 of 22 to 17 of 23 when the rule was written down. **That is the whole
evidence for this section, and it is one instance**; it is recorded as a rule because the failure
mode is structural rather than particular, and because it is the cheapest control in the spike to
state and the only one that found a channel.

This binds redaction's own harnesses and is offered to the rest: `split_no_leak.rs` and
`subset_closure.rs` both assert absence and neither requires that anything witnessed a removal.
**Whether to retrofit them is not decided here** — it is a change to tests that currently pass,
and it belongs with whoever next touches them.

## Consequences

**What becomes easier.** M2 can start. The write verb is trapped, the read verbs and the
tokeniser ship today, and nothing in this record reopens the linking gate, needs PDFium in a
document worker, or needs two engines resident in one tab. [#107](https://github.com/TensorGreed/burrow/issues/107)
and [#111](https://github.com/TensorGreed/burrow/issues/111) are both untouched by it.

**What becomes harder, and it is most of the work.** Five refusals, **four of which have no
adequate trigger yet** (§5) — two whose signal reads only the page's content stream and so misses
a Form XObject, and two whose carrier is on the unreachable catalogue. A tokeniser that needs byte spans, string values and operator
arity — and `pdfsyntax/names.rs` currently argues *against* an operator table, correctly for an
over-approximating filter and backwards for a rewriter. Font-object surgery in an operation that
had none. And a page that has to say four uncomfortable things.

**A cost that is not a nicety: the size budget.** `engines/qpdf-not-exported.toml` records
split's ten object-surgery exports at **+3,340 brotli** and merge's seven at **+53,456**, and says
in those words that *"the variance is in what else gets pulled in, not in the count"*. The
estimate for this batch is that range, to be **measured** rather than assumed — quoting the low
number as a precedent is the one use that file tells you not to make of it.

**`qpdf_oh_new_null` needs an argued exemption, or v1 ships a trick.** The write verb takes two
object handles for `/Filter` and `/DecodeParms`. The obvious constructor is neither trapped nor
exempted. The spike obtained a null handle by asking a dictionary for a key it does not have,
through the trapped `qpdf_oh_get_key` — which works, and which is cleverness in the place this
repository has least appetite for it. The honest route is an entry in
`engines/qpdf-untrapped-accepted.toml` on the same argument `qpdf_oh_new_integer` already
carries: it allocates and returns a handle, touching no bytes and resolving no object.

**Incremental-update history is closed, and the claim is narrower than it reads.** The spike's
fixture replaces *the same object number*, and qpdf's writer emits only what the current trailer
reaches. It does **not** measure an update that adds a new object while the old stays referenced,
an old page object reachable through a `/Prev` chain, or a damaged cross-reference that qpdf
reconstructs by scanning the body — where the old object can be picked up. *"Redaction does not
have to close incremental-update history"* would be a stronger sentence than the evidence.

**The render cannot preview a redaction.** burrow's render passes `flags = 0`
(`core/burrow-engines/src/pdfium/ffi.rs`, *"annotations are not part of the page"*), and the
spike measured a FreeText annotation still carrying the secret drawing at ink 0.0693 **with**
`FPDF_ANNOT` and 0.0000 without. A preview built on the shipped render would show a person a
clean page over an annotation that still says it.

**Two refusals are the price of ADR 0013's bar, and are worth naming as such.** `/AcroForm` and
`/StructTreeRoot` are refused *only* because the catalogue is unreachable. That is a real cost of
a rule this project is right to keep — a 356-byte PDF killed the process through an untrapped
function — and somebody reopening `qpdf_get_root` should find out here what it would buy.

**The `redaction` corpus set in `corpus/manifest.toml` is declared and empty**, and spike 0006's
fixture generator is the obvious first entry. Both `known_gap` entries in
`tests/conformance/expectations.json` target M2 and `MAX_KNOWN_GAPS` is 2.

## Conditions for revisiting

Three, all named rather than general, each with the thing that would change.

**`hb-subset`, for the `cmap` residue.** §1 closes `/ToUnicode` and `/Differences`. The embedded
font program itself still covers the removed run's alphabet in its own `cmap` — and its `post`
table and glyph inventory describe the same subset, neither of which spike 0006's probe reads,
so the measured residue is a **floor**. Closing it means rebuilding the font, which means
`hb-subset`. It is already on `docs/ROADMAP.md`'s M2 list as a wish; this record makes it **the
named condition** under which channel 23 moves from *disclosed* to *handled*. The disclosure is
weaker on a font that also sets other text, which spike 0006 did not measure — that measurement
is the first thing to take before deciding.

**The catalogue wall, for the six document-level carriers.** `/StructTreeRoot`, `/AcroForm`,
`/OCProperties`, `/Metadata`, `/Names` and `/Info`. Two of them are refusals and three are
disclosures **solely** because `qpdf_get_root` and `qpdf_get_trailer` fail ADR 0013 §1's caller
rule. Any of these would change that:

- upstream qpdf restructuring either function so its `QTC::TC` call moves inside the trapped
  lambda — a version bump would surface it, since `tools/check-qpdf-trapped.py` regenerates the
  set;
- a C++ shim, which ADR 0013 rejected and which this record does not reopen;
- a structural route to the catalogue outside the engine, which is [#24](https://github.com/TensorGreed/burrow/issues/24)'s
  inflater and the same wall #111 hit from the other side.

**Image re-encoding, for the scanned-document refusal.** §3 refuses a page whose region
intersects an image, and a scan is the commonest document there is — the whole page is one
image. **The reason is narrower than "we cannot redact images", and an earlier draft of the
corpus entry got it wrong in a way that made a solvable problem look unsolvable**: it said
removing the pixels would mean deciding *which* pixels, and that deciding meant recognising text
in a bitmap, and therefore OCR.

Redaction here is **region-based**. The person selects the region, so *which pixels* is not a
question anybody has to answer: every pixel inside the rectangle is blacked out, and the
invisible text glyphs whose boxes fall in the same region go through the ordinary
content-stream path. **No recognition is involved at any point.**

What actually blocks it is writing the edited image back, and it differs per filter:

| filter | what re-encoding costs |
|---|---|
| `/FlateDecode`, greyscale or RGB | **Straightforward.** Decode, black out the region, re-deflate. zlib is already linked, and the image dictionary needs nothing beyond a new `/Length` |
| `/DCTDecode` | **Lossy.** Re-encoding the whole image imposes generation loss on the parts nobody touched. Editing in the DCT domain — zeroing the coefficient blocks the region covers — avoids that and is specialised work |
| `/JBIG2Decode` | **Hardest.** Symbol dictionaries are shared across pages, neither engine decodes them here, and [spike 0005](../spikes/0005-what-qpdf-alone-compresses.md) refused `jbig2enc` on correctness grounds rather than licence ones |

So the condition is **not** "acquire OCR". It is **an image re-encoding path, taken per filter,
starting with Flate** — which is the easy case and is what `tests/redaction/fixtures/producer-ocr-scan.pdf`
happens to be, making it the fixture to measure against.

It matters more than it looks: this is the only one of the three conditions that governs a
document shape a person is *likely to bring*, and until it is taken the refusal has to say why
in a way that does not read as permanent (§7, and [#136](https://github.com/TensorGreed/burrow/issues/136)).

**None of the three is a plan.** They are written down so that the next person to look does not
have to re-derive why five channels are handled the way they are.

## Alternatives considered

**Refuse every document with metadata or attachments.** Rejected: nearly every PDF a person owns
has XMP, so this is not a refusal, it is not shipping. The spike's measurement that page-level
metadata *is* strippable is what made disclosure the residue rather than the whole answer.

**Use PDFium's `FPDFText_*` to find the text and to verify it went.** Rejected on three grounds,
the first fatal. It is **not admissible evidence**: a document can write a `/ToUnicode` that
lies or an `/ActualText` that substitutes, and a check that decodes through the font is verifying
the document's own claim about itself — with the same font's `cmap` measured disagreeing. It also
collides with `tools/check-pdfium-is-render-only.sh`'s three-layer claim, and running the pass in
the render worker to sidestep #107 puts it in **a sibling heap**, which R10 forbids. Cheap —
330 µs for 137 pages — and inadmissible.

**Render every page and check the region is blank.** Rejected on cost and on meaning. Roughly
3.8 s for 137 *empty* pages at 4×, against ADR 0027 §2a's measured 34 s and 819 MB for one
path-heavy page at thumbnail size: a second operation larger than the first. And ink is not
legibility — measured at 1.0000 for a black bar hiding nothing readable.

**Black-box it: draw a rectangle and ship.** Not seriously considered, and recorded because the
spike now has the measurement rather than the slogan. `qpdf --qdf` and PDFium both read the text
straight out from under the rectangle. `docs/ROADMAP.md` has said *"removal, not concealment"*
from the beginning; this is the file that proves it.

**Carry a partial redaction with a warning instead of refusing.** Rejected: the alternative to a
refusal here is an output that looks redacted, which is the one failure this milestone exists to
prevent. `split` refusing a layered document is the precedent, and its wording is the model.

**Put ADR 0022's read-back to work on the content.** Rejected as insufficient rather than wrong.
`OutputReader`'s entire witness surface is a page count and a `/Rotate` vector; neither can see
content. §6 adds a predicate rather than stretching a witness.
