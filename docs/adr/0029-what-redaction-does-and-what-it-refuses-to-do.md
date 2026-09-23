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
| an **inline image** whose extent the page cannot derive | **refuse** — added by the [2026-09-21 amendment](#amendment-2026-09-21--the-channel-the-spike-missed-inline-image-extent). Spike 0006 did not measure this channel |
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

#### PDFium invents a space where a redaction leaves a gap

Measured during #131, and recorded here rather than only in a code comment because §6's
read-back is a **text-layer** read and [#134](https://github.com/TensorGreed/burrow/issues/134)
will build on it.

Removing a glyph from inside a run leaves a positioning adjustment in its place, so that the
glyphs after the cut do not move (#131's rule). Cutting `C` out of `ABCDE` produces
`[<4142> -600 <4445>] TJ`. `FPDFText_CountChars` on that page then reports **five** characters:
`A`, `B`, a **`U+0020` that is in no string in the file**, `D`, `E`. PDFium's text layer
synthesises a space when the gap between two glyphs is wide enough, and a positioning
adjustment is exactly such a gap.

Three consequences, each stated because the obvious reading of a character count gets one of
them wrong:

- **It is not a leak.** No glyph from the removed run survives, and the synthetic space carries
  no information about what was there beyond the fact that *something* was.
- **"The text is shorter by the number of glyphs removed" is false**, and a verification
  asserting it would fail on correct output. Verification must match the **kept** glyphs —
  by code and by origin — rather than count characters.
- **A wide kern the producer wrote does the same thing**, before any redaction. Measured: a
  `-200` kern at 12 pt already makes PDFium synthesise a space, so char indices and glyph
  indices are not the same sequence even on an untouched page. Anything correlating the two by
  position is wrong on documents nobody redacted.

The synthetic space is identified by **position in the expected sequence**, not by being a
space: a page may contain real spaces, and skipping every `U+0020` would skip those too, which
is how a check for word spacing would quietly stop checking anything.

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
> **And a font other pages also use keeps more than that: the widths of the removed characters,
> and the mapping from their codes to the letters they stood for.** Not the words, and not where
> they were — but more than the character set, because the `/Widths` array and the `/ToUnicode`
> map still have an entry per code. Not Only PDF leaves that font untouched on purpose: editing
> it would change how the text looks on the other pages that use it, which is damage to a page
> you did not ask about rather than a leak on the one you did.
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

Five, all named rather than general, each with the thing that would change. The fourth was
added by #129 on 2026-09-21; the fifth by #131 on 2026-09-22, when the real font resolver
first met the corpus.

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

It matters more than it looks: this and vertical writing are the conditions that govern a
document shape a person is *likely to bring*, and until it is taken the refusal has to say why
in a way that does not read as permanent (§7, and [#136](https://github.com/TensorGreed/burrow/issues/136)).

**Vertical writing, for the glyph geometry.** #129 places glyphs by composing the text, font and
current transformation matrices, and every term in it displaces horizontally. A vertical run
advances **downwards**, takes its widths from `/W2` and `/DW2` rather than `/W` and `/DW`, and
offsets each glyph origin by a vertical origin vector — so walking one as though it were
horizontal puts every box in the wrong place, and in the direction that misses text inside the
region rather than the direction that removes too much. It is therefore refused, not
approximated.

Two things about the refusal are worth recording here rather than only in the code, because both
are places a later reader would get it wrong:

- **It is keyed on `WMode`, never on the name `Identity-V`.** A name check misses the rest of
  Adobe's registry (`UniJIS-UCS2-V`, `90ms-RKSJ-V`, `ETen-B5-V`, …) and misses embedded CMaps
  altogether, since there the producer chooses the stream's name — a CMap called `/Identity-H`
  that declares `/WMode 1 def` is vertical. `WMode` is read from the stream dictionary, from the
  program, and through `usecmap`; the disagreement and the unreadable cases are refused rather
  than resolved in one direction. `CMap::Embedded` carries no name at all, so the wrong key is
  not expressible. **And the walk derives the mode rather than being handed it**: the resources
  seam carries the *encoding* the resolver found, not a conclusion about it, so a resolver that
  cannot decode a CMap stream has to say so — `Encoding::UnreadableCMap`, which is refused —
  rather than returning horizontal by default. Before that change the rule held in this module's
  unit tests and over no document at all. A real PDF whose `/Encoding` is a CMap **stream** named
  `/Ordinary-H` declaring `WMode 1`, and its one-digit horizontal twin, are both exercised in
  `core/burrow-engines/tests/glyph_geometry.rs`.
- **A vertical document need not declare vertical writing.** Measured, and the measurement is
  now a committed file (`tests/redaction/fixtures/producer-vertical-writing.pdf`) with a test
  that fails if the producer ever changes its mind: asked for a vertically
  written Japanese paragraph, LibreOffice 24.2.7.2 emitted a subset **simple** font and one `Tm` per
  glyph, stepping `y` down the page — no CID font, no `WMode`, nothing to refuse. That document
  is walked correctly by the ordinary horizontal machinery. The refusal covers the CMap case and
  only the CMap case, which is the honest statement of its reach.

What would change it: `/W2`, `/DW2` and the vertical origin vector implemented and measured
against `FPDFText_GetCharBox` on a vertical fixture, to the same tolerance §6 sets for the
horizontal terms. Until then it is a refusal with fixtures on both sides: a built document whose
`/Encoding` is a CMap **stream** named `/Ordinary-H` declaring `WMode 1`, refused, against its
one-digit twin that is not; and `tests/redaction/fixtures/producer-vertical-writing.pdf`, a real
producer's vertical document, which is the case that must **not** be refused and is not. What is
still missing is the resolver that would carry a file's CMap to the seam in production — the
seam's shape now forces that resolver to answer the question rather than skip it, but nothing
implements it yet.

### What the walk does not reach, which §6 must not be read as covering

Measured during #129's review, and recorded here because §6's read-back shares the blindness:
a page whose only text lives in a **tiling pattern** produced an empty glyph list from burrow's
walk and **zero characters** from `FPDFText_*`, while PDFium's renderer inked 740 pixels of the
word. Neither the operation nor its verification saw it. §8's rule -- a removal nothing observed
is not a measured removal -- applies to a *presence* nothing observed just as squarely.

The pattern case is closed by refusal. The one left open is an **ExtGState naming a `/Font`**,
which sets face and size with no `Tf`: refusing every `gs` would refuse most real documents, and
resolving it needs a seam the resources trait does not have. That is
[#152](https://github.com/TensorGreed/burrow/issues/152), and until it closes, §6's assertions
are bounded by "every operator the walk models, plus patterns refused" rather than by "the page".

**The standard-14 metrics, for the font that carries none.** A font with no `/Widths` array
has **no advances in the document at all**: the standard 14 — Helvetica, Times, Courier,
Symbol, ZapfDingbats and their variants — are defined by tables every viewer bundles and no
file repeats. PDFium has them built in. burrow does not, so the resolver refuses rather than
guessing, and it is right to: an invented width misplaces every glyph after it on the line, and
a region test over a misplaced glyph is a redaction that removes the wrong thing or nothing.

**Measured, because a refusal this common would be a product decision rather than an
implementation detail.** Over every committed fixture in `tests/redaction/fixtures` and
`tests/conformance/fixtures` — 26 files:

| outcome | count |
|---|--:|
| walked cleanly | 11 |
| refused: **no `/Widths`** | **1** (`font-program-with-a-paren.pdf`) |
| refused: the document does not open at all | 10 (the deliberately damaged fixtures) |
| refused: the content names a font the resources do not provide | 3 |
| refused: text shown with no font selected | 1 |

So it fires **once** in the committed corpus, and **not at all** on the four real-producer
documents — LibreOffice, pdfTeX and tesseract all embed the fonts they use and declare their
widths. That is the shape to expect: a producer that embeds a subset has to declare widths,
because the subset is not a standard font any more.

Where it will fire is the hand-written PDF and the minimal generator — a document that says
`/BaseFont /Helvetica` and stops. Those are real, and a user who brings one gets a refusal for
a document every viewer opens happily.

#### Amendment, 2026-09-22: the measurement above examined the wrong half of the corpus

The table counts `tests/redaction/fixtures` and `tests/conformance/fixtures`, 26 files. It does
not count **`tests/redaction/generated/`**, which is the 39-document survival corpus spike 0006
produced and the one this operation exists to be measured against. Running the assembled
operation over all 43 documents — the first time anything has, because until the qpdf `Steps`
implementation there was nothing to run — gives:

| outcome | generated (39) | producer (4) |
|---|--:|--:|
| redacted | **0** | **4** |
| refused: no `/Widths` | **38** | 0 |
| refused: a pattern that may draw text | 1 | 0 |

So the refusal does not fire once. It fires on **38 of 43 documents**, and on **every document
in the corpus this milestone is measured against**. The sentence above — *"it fires once in the
committed corpus"* — was true of the files it looked at and false as a statement about the
corpus, and it was load-bearing: it is why bundling the metrics was filed as the cheapest of
five conditions rather than as a blocker.

The earlier reading's other conclusion survives intact and is the reason the two halves differ
so sharply: every real-producer document redacts, because a producer that embeds a subset must
declare its widths. The hand-built corpus says `/BaseFont /Helvetica` and stops — which is what
a hand-written PDF does, and what spike 0006 wrote because the channel under test was never the
font.

**This does not change the refusal.** Guessing a width is still the wrong answer, for the reason
the paragraph above gives. What it changes is the priority: with the metrics bundled, 38 of
these 43 documents become documents this operation can act on, and without them the conformance
sweep for #134 has four documents to verify against rather than forty-two.

**What would change it: bundling the standard-14 metrics.** They are 14 tables of a few hundred
widths, they are not copyrightable data, and the Adobe Font Metrics files that carry them are
redistributable. It is a few tens of kilobytes and no new dependency — which puts it in a
different class from `hb-subset` and image re-encoding, both of which need code that does not
exist here. It is the cheapest of the five conditions and the one whose absence a user is most
likely to meet.

Until then the refusal has to say why in the voice §7 established, and name the document shape
rather than the missing table: *"this page uses a font whose measurements are not in the file,
so burrow cannot tell where its text sits."*

### Fonts follow the same rule as forms, and the same wall

Font surgery removes the `/Widths`, `/ToUnicode` and `/Differences` entries for codes the
document no longer draws (§1, and the ordering amendment). A font is shared by nearly every
multi-page document, so the two obvious rules are both wrong:

- **refuse on any shared font** refuses nearly every multi-page document;
- **edit it in place** reflows the text on every other page that uses it. The glyphs are still
  there at different widths, so the page still renders and still says something — slightly
  different. That is **corruption rather than leakage**, and it is worse in one way: §6's
  read-back asks about the page it was given, and that page is clean.

So the rule is neither: **cut a font only when every page that uses it is being redacted in
this operation.** Counted across the whole document by object identity, with the resolver's
inherited-`/Resources` handling, and including fonts reached only *through* a form or pattern —
a font named inside a form that page 3 draws is a font page 3 uses. Otherwise the font is left
intact and disclosed (§7, which now says what it retains).

**Measured, because the rule is only worth choosing if it lets documents through.** Redacting
page 0 alone, over every openable committed fixture:

| | |
|---|--:|
| fixtures examined | 15 |
| have no fonts at all | 9 |
| cut **every** font | 5 |
| retain at least one, and disclose | 1 |

The one that retains is a genuinely two-page document whose pages share a font, which is
exactly the case the rule exists for. All four real-producer documents cut everything.

**Copy-on-write is the answer that serves both, and it hits the same wall as forms**: cloning a
font dictionary needs `qpdf_oh_new_dictionary`, which is not on the trapped list and does not
meet `qpdf-untrapped-accepted.toml`'s non-parsing bar, exactly as `qpdf_oh_new_stream` does not.
So fonts join forms under one condition for revisiting — upstream trapping those two verbs —
rather than getting a condition of their own.

**None of the five is a plan.** They are written down so that the next person to look does not
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


## Amendment, 2026-09-21 — the channel the spike missed: inline-image extent

**This record's §3 says it has "no 'not applicable' rows — a channel with no bucket is how the
spike's own bar caught two omissions". It had a third, and the bar did not catch it because the
channel was never enumerated.** Spike 0006 measured twenty-three places a page's text can
survive. An inline image's *extent* is not one of them, and it is a place text can hide from the
tokeniser entirely.

### What was measured

`BI … ID <data> EI` puts uninterpreted bytes in a content stream. Where the data ends is the
question, and burrow's tokeniser answered it the way every PDF reader falls back on: the first
`EI` standing alone, preceded by white space and followed by white space or a delimiter. On this
page that is the wrong `EI`:

```text
q BI /W 1 /H 1 /BPC 8 /CS /G ID AEI
BT /F1 24 Tf 1 0 0 1 72 700 Tm (BURROW-SECRET) Tj ET
 EI Q
```

The dictionary declares **one** byte of data. The image therefore ends at the `EI` immediately
after `A` — which is preceded by `A` rather than by white space, so the scan ran past it, past
the text object, to the trailing ` EI`. **PDFium draws `BURROW-SECRET` from that page**;
`pdfsyntax::operations` reported `q`, `BI`, `ID`, `Q`, with no text operator and no string
operand anywhere in it.

PDFium is the renderer burrow ships and the one §6's read-back reads through, so this is a
measurement about burrow rather than about PDF readers in general. Other readers on the
development machine were deliberately not used: ADR 0003 is permissive-only, and a GPL tool cited
in this record's evidence is the precedent for the next one.

An `ID` with no `BI` at all is the same shape with no dictionary to consult.

### The decision

Not "handle", because there is nothing to rewrite — the leak is that the operation cannot *see*
the text, and a redactor that cannot see it reports the page clean, which §6 forbids in the
strongest terms it has.

**An inline image whose extent cannot be derived from its own dictionary is refused.** The extent
comes from `/L` (`/Length`), or is computed from `/W`, `/H`, `/BPC` and `/CS`, and an `EI` is
required exactly there; where neither is possible the stream is refused rather than scanned for.
The underivable cases are a `/F` filter with no `/L`, a `/CS` naming a colour space from the
page's `/Resources`, a missing `/W` or `/H`, a `/BPC` outside the five the specification allows,
and a dictionary that will not lex.

### What it costs, stated rather than discovered

The refusal is in the tokeniser, so it reaches **every** caller of it, `split`'s resource scan
included — a shipped operation now refuses a document it would previously have processed. No
committed fixture is affected; the corpus's one inline image declares `/L 135`. That is evidence
about the corpus as much as about the world, and it is recorded that way.

[#142](https://github.com/TensorGreed/burrow/issues/142) is the work that would narrow the
refusal again, by deriving extents currently out of reach. It is fidelity work: the leak is
closed, and what is left is how many documents pay for closing it.

### Why this is an amendment and not a correction to §3

Nothing §3 decided is reversed. A channel it never had is added to it, with its bucket, which is
what §3's own rule about empty buckets demands. The three conditions for revisiting are
unchanged.

## Amendment, 2026-09-21 — redaction ships in its own lazily-loaded wasm module

**The *Consequences* section costs this record's decisions in engine exports and in verification
work, and says nothing about the web payload.** That was an omission rather than a judgement, and
#128 turned it into a number.

### What was measured

#128 landed the content-stream rewriter — some two thousand lines across `ops.rs`, `contents.rs`
and `strings.rs`. It cost **zero brotli** in `burrow_wasm_bg.wasm`, the module every tool page
fetches on its first file. Checked against the built artifact rather than assumed: none of those
modules' error strings are in it. LTO strips code nothing calls, and on the web nothing calls them
yet.

**That stripping stops the moment [#131] wires them in.** The rewriter, then §6's geometry pass,
then §1's font surgery, all become reachable from a shipped entry point at once — and they land on
the module a person downloads to rotate a PDF.

The margin cannot absorb it. After #128 the first-load total is **471,205 brotli against a 509,812
ceiling: 38,607 bytes, 7.6% against the 10% `apps/web/size-budget.json` records as its policy.**
The gate holds and the margin does not; it has been under 10% since the page-picture strip.

### The decision

**Redaction's Rust is compiled into its own binding module, fetched on the first redaction and
never by anything else.** [ADR 0026](0026-how-rendering-loads-without-returning-to-the-old-payload.md)
§1 already established the shape for PDFium, and this is a third row of the same table:

| bundle | engine | Rust module | fetched |
|---|---|---|---|
| `burrow-worker.js` | qpdf | `burrow_wasm_bg.wasm` | on the first file, by every tool page |
| `burrow-render-worker.js` | PDFium | `burrow_wasm_render_bg.wasm` | on the first page picture |
| `burrow-redact-worker.js` | qpdf | `burrow_wasm_redact_bg.wasm` | on the first redaction |

So `bindings/burrow-wasm` gains a third mutually exclusive feature, `redact`, beside `documents`
and `render`; ADR 0026 §2's refusal of a `wasm32` build enabling none or more than one extends to
cover it. `tools/stage-web-engines.mjs` builds the bundle from its own source list and generates
its own manifest slice into it.

**The total budget is not raised for redaction.** That is the decision, not a consequence of it:
a tool that redacts nothing must not download a redaction engine, and raising the ceiling instead
would have spent the margin on exactly that.

### What it does not cover, stated because the split reads as bigger than it is

`qpdf.wasm` is one artifact shared by every tool and no feature flag divides it, so the qpdf C
exports redaction needs — `qpdf_oh_replace_stream_data` and whatever follows it — are unavoidable
base payload. [#130] measures that cost and reports it rather than re-recording past it. The
calibration on record is that `split`'s ten exports cost +3,340 brotli and `merge`'s seven cost
+53,456, and `engines/qpdf-not-exported.toml` says in those words that the variance is in what
else gets pulled in rather than in the count.

### The check that keeps it true

A split nothing verifies is a split that closes quietly the first time someone imports across it.
`tools/check-pdfium-is-render-only.sh` is the precedent — three layers, asserting PDFium reaches
the render bundle and no other part of the build — and the redaction module gets its counterpart:
**the base bundle contains no redaction symbol.** [#137] owns both, because it owns the binding
entry point, and the entry point is what decides which module the code is compiled into.

[#130]: https://github.com/TensorGreed/burrow/issues/130
[#131]: https://github.com/TensorGreed/burrow/issues/131
[#137]: https://github.com/TensorGreed/burrow/issues/137

## Amendment, 2026-09-22 — a shared Form XObject is refused, not edited

Raised on #131, before the code that would have got it wrong was written.

### The hazard

§1 removes glyphs from the streams that draw them, and a Form XObject **is** such a stream. But
a form is an object, and an object can be drawn more than once: from several pages, or several
times on one page at different `cm` transforms. Editing it in place removes the text
**everywhere it is drawn**.

That fails in both directions at once, which is what makes it worse than a missed channel:

- a page nobody asked about loses content, silently — a redaction that damaged a document
  rather than redacting it;
- and the region the user *did* select is on one of those pages, so the operation appears to
  have worked.

Nothing in §6's read-back catches it: that verification asks whether the selected region is
clear on the page it was asked about, and it is.

### The decision: refuse only when the region reaches inside a shared form

v1 refuses when **the region reaches glyphs or ink inside a Form XObject that is drawn more than
once** in the document. A form drawn exactly once is edited in place as §1 describes, because
there is nowhere else for the edit to reach.

**The scoping is the decision, not a detail of it.** Refusing any page that *contains* a shared
form would refuse a large share of real documents: a letterhead, a header, a footer, a watermark
and a logo are all commonly one form object drawn on every page, and none of them is what the
user selected. A redaction nobody can run leaks nothing only because it never runs, and §7's
disclosure is worth nothing if it fires on the ordinary case.

So the test is over the glyphs **being removed**, not over the resources present: a form is only
in question when a glyph the operation is about to cut came out of it.

**Sharing is detected by object identity, never by resource name.** Two pages may both call a
form `/Fm0` and mean different objects; one page may reach the same object under two names.
`core/CLAUDE.md` already states the rule this rests on — a `qpdf_oh` handle is a fresh number on
every call and is not an identity, so the comparison is on **object number and generation**,
`ObjectHandle::object()`. `tools/check-handle-identity.py` enforces that a raw handle is never
compared, and the detection must satisfy it rather than argue around it.

The scan is over **every page's** resource graph, not the page being redacted. A form shared
with a page the user never selected is exactly the case the refusal exists for, and a per-page
scan cannot see it.

And "the resource graph" means every graph from which a form can be drawn, not just page
resources. A use counted in one place and missed in another reads as *unshared*, which is the
direction that edits in place and damages the other page:

| graph | why it can reach a form |
|---|---|
| a page's `/Resources` → `/XObject` | the ordinary `Do` |
| a **nested form's** own `/Resources` → `/XObject` | a form drawing another form, to `MAX_FORM_DEPTH` |
| an annotation's `/AP` → `/N` (and `/D`, `/R`) appearance stream's `/Resources` | an appearance is a form, and it has resources of its own |
| a **tiling pattern's** `/Resources` | a pattern's content stream draws like any other |
| a **Type 3 font's** `/CharProcs` entries and the font's `/Resources` | a glyph procedure draws like any other |

The annotation case is the one a page-only scan misses most easily, because an appearance
stream is reached through `/Annots` rather than through `/Resources`, and it is a form object in
its own right. There is a fixture for it.

### The same rule applies to Type 3 `/CharProcs`

A Type 3 glyph procedure is a content stream that draws glyphs, and a Type 3 **font** is shared
by every page that selects it — which is the ordinary case, not the exotic one. Editing a
`/CharProcs` entry changes that character everywhere in the document.

So the same test, scoped the same way: refuse when **the region reaches text inside a glyph
procedure** belonging to a Type 3 font reachable from more than one place. A Type 3 font merely
being present refuses nothing — which matters more here than for forms, because a Type 3 font
used on one page only is uncommon, and an unscoped rule would refuse essentially every document
that has one.

**Residue, stated rather than implied:** the geometry walk does not descend into `/CharProcs`
today. `Resources` resolves forms and glyph metrics; it has no hook for a glyph procedure's
content stream, so text drawn inside one is not currently found at all — which is spike 0006's
channel 8 and is why that channel is still open. This rule is therefore written for the walk
that will reach it, and the refusal cannot fire until it does. Recording the rule now is
deliberate: the alternative is discovering it while writing the code that would have edited a
shared font in place.

### A shared page `/Contents` is the same hazard, and it is **not** covered

Raised by a review, and recorded rather than quietly left: the rule above is scoped to glyphs
whose source is a **Form XObject**. A page's own content stream is `form: None`, and nothing
counts how many pages share it.

Two pages pointing at one `/Contents` object is legal, and it is not exotic — the
`inheriting_document()` fixture in this very branch builds one, because it was the shortest way
to write a two-page document. Editing that stream removes the text from **both** pages, with
§6's read-back clean on the page it was given. Identical hazard, outside the rule.

It is not reachable today, because nothing calls the removal. It has to be closed before
anything does, and there are two ways:

- count page-`/Contents` objects in the same walk and extend the refusal to the `None` case,
  which is a few lines and the same shape as the form rule;
- or emit a **copy** of the content stream for the page being redacted and repoint only that
  page, which does not hit the `qpdf_oh_new_stream` wall below because
  `qpdf_oh_replace_stream_data` on a page's existing stream is already how the operation works —
  what is missing is a second object to point at.

**Recorded as a named gap rather than an assumption**, because §8's rule cuts both ways: a
hazard nobody wrote down is a hazard nobody will look for.

### Copy-on-write is the alternative, and the C API wall is in the way

The better answer is to clone the form, edit the clone, and repoint **only this `Do`'s**
resource entry, leaving every other reference at the original. That is `split`'s shape applied
to one object.

It needs these verbs, and their status against `engines/qpdf-trapped-functions.txt` is the
whole argument:

| verb | needed for | status |
|---|---|---|
| `qpdf_get_num_pages`, `qpdf_get_page_n` | enumerating pages to detect sharing | **trapped** |
| `qpdf_oh_get_key`, `qpdf_oh_has_key` | walking `/Resources` → `/XObject` → the form | **trapped** |
| `qpdf_oh_get_object_id`, `qpdf_oh_get_generation` | identity, for the sharing test | **trapped** |
| `qpdf_oh_get_type_code` | asking whether an object is a stream | **trapped** |
| `qpdf_oh_get_stream_data`, `qpdf_oh_replace_stream_data` | reading and writing a form's content | **trapped** |
| `qpdf_oh_replace_key` | repointing the `Do`'s resource entry | **trapped** |
| `qpdf_make_indirect_object` | giving the clone an object number | **trapped** |
| **`qpdf_oh_new_stream`** | **creating the clone** | **NOT trapped, and not acceptable** |
| **`qpdf_oh_new_dictionary`** | **the clone's stream dictionary** | **NOT trapped** |
| `qpdf_oh_get_stream_dict` | copying the original's dictionary | **neither trapped nor accepted** |

So **detection is entirely within the permitted API and the copy is not.** The refusal can be
implemented today; copy-on-write cannot.

`qpdf_oh_new_stream` is not a borderline case. Read it:

```c
qpdf_oh_new_stream(qpdf_data qpdf)
{
    QTC::TC("qpdf", "qpdf-c called qpdf_oh_new_stream");
    return new_object(qpdf, qpdf->qpdf->newStream());
}
```

A bare `QTC::TC` outside any trapping lambda, then a direct call into `QPDF`. That is the same
shape as `qpdf_get_root` and `qpdf_get_trailer`, which ADR 0013 §1's caller rule already
rejects, and for the same reason: a C++ exception crossing an `extern "C"` frame is not
something `catch_unwind` can hold.

Nor does it qualify for `engines/qpdf-untrapped-accepted.toml`. That file's bar is that the
function is **non-parsing** — "it assigns a field, flips a flag, or reads a stored value …
never resolves an object". `QPDF::newStream` mutates the document's object table. It is not in
`qpdf_oh_new_null`'s class, and an entry claiming it were would be the kind of false exemption
that file's own header warns against.

Note also that `qpdf_oh_is_stream` is **not** the way to ask whether an object is a stream:
resolving an indirect handle parses, which is `qpdf_is_linearized`'s disqualifying property.
`qpdf_oh_get_type_code` is trapped and is the verb to use.

### What the scan costs

Measured 2026-09-22, because a rule that walks every page's graph on every redaction has to be
priced rather than assumed. No large document is committed — `corpus/files/` is fetched, and the
biggest fixture here is 137 pages and 139 objects — so the measurement is on generated documents
of the shape the rule cares about: one form drawn by every page **and** by every page's
annotation appearance.

| pages | objects | bytes | reference scan | `qpdf --check` (median of 5) |
|--:|--:|--:|--:|--:|
| 1,000 | 2,004 | 283 KiB | 0.5 ms | 32 ms |
| 5,000 | 10,004 | 1,424 KiB | 2.4 ms | 110 ms |

Both columns are linear in object count, and the scan itself is not where the cost is: **qpdf's
own object resolution dominates by roughly fifty to one**. So the rule's price is one pass over
the object graph, which a redaction already pays to open the document, and the walk should reuse
that pass rather than taking its own.

Two honest qualifications. The scan column is a *string* scan standing in for the qpdf-side walk
that does not exist yet, so it bounds the bookkeeping and not the resolution. And these documents
are uniform; a real one with deeply nested forms pays `MAX_FORM_DEPTH` per entry rather than one.
The number to re-measure is the production walk when it lands.

The same generated documents also measure the annotation half of the rule: at 5,000 pages, where
each page draws the form once and its annotation is the form, a full scan counts **10,000** uses
and a page-resources-only scan counts **5,000**. A scan confined to page resources reports
**half** the uses — and under-counting reads as *unshared*, which is the direction that edits in
place.

**Those numbers were first recorded as 11,000 and 6,000, and were wrong.** The instrument was a
substring search for `"4 0 R"`, which also matches inside `"14 0 R"`, `"24 0 R"` and every other
object number ending in four — 1,000 spurious hits in both columns. A review recomputed them and
disagreed; re-measuring showed the review was right. The error is recorded rather than quietly
patched, because it is the same family as the rest of this document: **a measurement is only as
good as the instrument, and a substring scan with no word boundary is not one.** The ratio the
paragraph rests on — a page-only scan sees half — survives, which is luck rather than
robustness.

### Condition for revisiting

**Upstream trapping `qpdf_oh_new_stream` and `qpdf_oh_new_dictionary`**, which a version bump
would surface because `tools/check-qpdf-trapped.py --generate` regenerates the set. At that
point copy-on-write becomes implementable inside the permitted API and shared forms move from
*refused* to *handled*. A C++ shim is the other route and this record does not reopen it;
ADR 0013 rejected it.

Until then the refusal has to say why in a way that does not read as permanent (§7), and the
disclosure names the document shape rather than the engine limitation: *"this page draws text
from a template used elsewhere in the document, and removing it here would remove it there
too"*.

## Amendment, 2026-09-22 — the region's frame, and why it is in the type

### Four numbers with no frame can be read four ways, and three of them miss

A page has up to five boxes, may declare a `/Rotate` that turns what the reader sees away from
user space, and may declare a `/UserUnit` that changes what a point means. "The rectangle the
user selected" is therefore not well defined until the frame is stated, and a wrong reading does
not fail loudly: the region lands somewhere the text is not, no glyph intersects it, the
operation removes nothing and **reports success**.

So the frame is part of the type. `pdfsyntax::region::Region` cannot be built from four bare
numbers with the meaning left to the caller.

### The frame

A `Region` is in **display coordinates** — what the person looking at the page saw:

| question | answer |
|---|---|
| which box? | **`/CropBox`**, falling back to `/MediaBox`. A viewer shows the crop box, so that is what the user selected within |
| before or after `/Rotate`? | **after** — the user selected on the page that was on screen |
| what is a unit? | **points as displayed**, already multiplied by `/UserUnit` |
| origin | **top-left, y downwards**, as every viewer and pointing device reports |

The last row is the one most likely to be wrong silently: PDF user space has its origin at the
**bottom** left. A region converted without the flip lands mirrored about the page's horizontal
centre, which on a form or a two-column page is very often still *on* the page and over the
wrong text.

### Converted in one place

`Region::to_content_space` is the only conversion, and it does four things in order:
`/UserUnit` divides out, the y axis flips, `/Rotate` unwinds, and the display box's origin
shifts back in. Everything downstream works in content space and never sees a display
coordinate. A second conversion site is a second chance to disagree, and the disagreement would
be a redaction over the wrong part of the page.

An unreadable frame is refused rather than guessed: a `/Rotate` that is not a right angle, a
`/UserUnit` that is not positive and finite, a display box with no extent. Each has a fixture,
and each fixture asserts **which** rule refused.

The three frame fixtures each place the region so a wrong reading demonstrably misses:
a non-origin `/CropBox` whose offset exceeds the region's own width, a `/Rotate 90` page where
the region's extents swap, and a `/UserUnit 2` page where the region is half the size it looks.
Each asserts the whole rectangle rather than one edge — a first draft checked the extent only,
and a mutation that scaled the extents but not the origin survived it: the right size in the
wrong place, which removes the wrong text rather than none.

### The public entry point waits for #134

ADR 0022's rule is that an operation verifies its own output before returning it. Redaction is
the operation where that matters most, and the verification is
[#134](https://github.com/TensorGreed/burrow/issues/134).

So the assembled operation returns bytes **only** through the verified path, and until #134
exists there is no public entry point that emits a redacted document. The pieces — the walk,
the geometry, the removal, the sharing count, the frame — are crate-internal and tested, and
the seam they will be assembled behind is deliberately not exported yet.

Shipping an unverified redaction "temporarily" is the one shortcut this record will not take:
an operation that removes a secret and cannot say whether it did is indistinguishable, from the
outside, from one that did not.

## Amendment, 2026-09-22 — the order of the steps, and what a failure discards

### The order

1. **Every content edit, across every affected stream** — the page's own content, each Form
   XObject the region reaches that is not shared, each pattern.
2. **Then font surgery**, computing "no longer drawn" from the **complete** result of step 1.
3. **Then the page strip** — the keys outside §2's allowlist.

### Why step 2 cannot run early, and why the failure would be silent

Font surgery removes the `/Widths`, `/ToUnicode` and `/Differences` entries for codes the
document no longer draws. **"No longer drawn" is a fact about the finished content.** A form
edited in step 1 *after* the fonts were already cut leaves entries for codes nothing draws any
more — which is spike 0006's channel 4 and 23 residue, put back by hand.

The residue is not abstract: **the `/ToUnicode` entry for a removed glyph is the removed
character, in plain text, in the font.** A redaction that cut the fonts first removes the glyph
from the page and leaves its character in a table beside it.

And it does not fail loudly. The output is a valid PDF, the page renders correctly, and §6's
read-back — which is about glyph positions on the page — sees nothing wrong. The secret is
legible to anything that reads the font rather than the page.

So the order is a state machine rather than three calls in a comment: each step consumes the
redaction and returns the next state, so font surgery **cannot be reached** without having
finished every content edit. `font_surgery_reads_the_content_after_every_edit_not_before_any`
asserts it as an index comparison against a real run, and a mutation that moves the font step
into `edit_content` fails three tests.

**Step 3 is last** because the page-key allowlist is decided against the page as it will ship.
A key stripped before an edit that would have removed its last reference is a key whose removal
nothing observed — §8's rule, from the other direction.

### Any failure discards the whole document

Per #130's poisoned-document rule: **no partial emission, no retry, no fallback to a
partly-edited state.** The first failing step ends the redaction and the in-memory document is
dropped. A half-redacted page is the worst possible output, because it looks like a redaction.

Enforced by ownership rather than by discipline: every fallible step **consumes** the redaction
and returns it only on success, and `emit` consumes it too. A caller holding an error has
nothing left to emit from — the state that would have to be emitted no longer exists.

Measured, as the rule requires rather than as an argument: failing the **third of four** stream
rewrites refuses by name (`[document-poisoned]`), never reaches `write`, never attempts the
fourth rewrite, and runs none of the later steps — each asserted separately, because "it
returned an error" is satisfied by an implementation that also emitted something. A failure in
step 2, after every rewrite succeeded, is asserted the same way: the rule is about **any** step.

The non-vacuity control is that a clean run rewrites all four streams and does reach `write`.
Without it, every assertion about what does *not* happen after a failure would be satisfied by
an implementation that does nothing at all.

## Amendment, 2026-09-22 — what the fakes got wrong

Every test of the `Steps` seam before the qpdf implementation ran against a fake. This section
is what changed when a real document went through it, kept as a list rather than folded into
the sections it corrects, because the pattern across the six is the point: **not one of them was
a mistake in the code the fake stood in for.** Five were in code the fake never exercised at
all, and the sixth was in an instrument.

That is the shape to expect from a fake, and it is not the shape the phrase "the fake was more
forgiving" predicts. A fake is not usually wrong about the call it fakes. It is wrong about
everything downstream of that call that nobody reached.

### 1. `conservative_box` scaled its advance twice, so the box was the origin

`Glyph::advance` and `Glyph::font_size` are text-space quantities and `Glyph::to_page` begins
with the font matrix and the font size. `conservative_box` put the first two through the third,
so every advance box came back scaled by `0.001 × size` a second time. Measured on
`producer-writer.pdf` at 18 pt: a glyph PDFium boxes at 2.5 × 13 pt came out at **0.25 × 0.32
pt**, a box barely larger than the pen position.

A region overlapping a glyph's ink but not its origin therefore intersected nothing, and the
redaction removed nothing and returned `Ok`. This is the defect the conservative box exists to
prevent, inside the function that exists to prevent it.

**Why nothing caught it.** Every hand-built fixture draws its region around the glyph origin,
because that is the number a fixture author has to hand. It took a real producer document and a
region derived from **PDFium's own ink boxes** — the instrument that is not burrow — to put a
region somewhere a fixture author would not have thought to.

`Glyph` now carries `text_to_page` beside `to_page`, and the rustdoc on each says which
quantities belong in which space.

### 2. Dropping `/ToUnicode` corrupted the text the redaction kept

`narrow_font` removed the whole `/ToUnicode` stream, with a comment arguing that what it said
about the *kept* codes was already in the font's own `cmap`. It is not: a simple font's `cmap`
maps glyph selectors, and for a subset encoding there is nothing to fall back to. PDFium read
**`U+0001`** at the origin where readable text had been.

§1 already said "remove the entries for codes the document no longer draws", not "remove the
stream". The code did the second and the comment argued for it. `pdfsyntax::tounicode` now reads
a CMap and writes back one mapping exactly the kept codes; `qpdf_oh_replace_stream_data` is
trapped, so nothing new was needed at the FFI boundary.

### 3. Dropping `/Differences` was the same defect, and was found by looking rather than measuring

`/Differences` is `[ 65 /S /e /c /r /e /t ]`: it names the glyph for every code, including the
ones that stay. Removing it re-points every kept code at the base encoding — different
characters drawn, which is corruption rather than redaction. It sat in the same function as (2)
and had the same shape.

Erasing one name renumbers the rest, and constructing a replacement array is closed: neither
`qpdf_oh_new_array` nor `qpdf_oh_new_name` meets `engines/qpdf-untrapped-accepted.toml`'s
non-parsing bar. What is available is `set_array_item` with an integer, and it is exactly right
— replacing the name at index *i*, whose code is *c*, with the integer *c + 1* leaves the array
the same length and re-anchors the run at the code the next entry already had.
`[1 /a /b /c]` minus `/b` is `[1 /a 3 /c]`.

**Recorded as a finding even though no test measured it**, because the honest account is that it
was found by reading the other half of a function whose first half had just been shown wrong.

### 4. burrow refused to read its own output

A whole-page removal came back `Unsupported("a content stream gives an operator more operands
than burrow will read")` — about a `TJ` whose operator has exactly one operand. `read_composite`
pushed array *items* through the same check as an operation's operand run, so **any array of
more than 64 entries was refused**, and a `TJ` holds one entry per kerned glyph pair. An
ordinary justified line of forty characters is past it.

This is a pre-existing parser defect with nothing to do with redaction, and it had been in the
tree since the operand caps were added. No fixture had an array long enough. `MAX_COMPOSITE_ITEMS`
is now its own cap, and `MAX_TOTAL_OPERANDS` still counts composite items so the aggregate bound
is unchanged.

### 5. An inline image's `/D [1 0]` refused the page

`inline_image_length` alternates key, value, key, value. An array value is four tokens, so `/D`
put `1` in a key position and the whole dictionary was refused — and `/D` is the Decode array
every one-bit image mask carries. `producer-latex.pdf` writes its Type 3 bitmap glyphs exactly
that way, fifty-five of them.

**Found by a check that had never run.** `check_type_three_procedure` was written for the Type 3
`/CharProcs` hazard and was not wired into the qpdf steps; wiring it up was a two-line change
that immediately refused a real document, for a reason that had nothing to do with Type 3
fonts. A check nobody calls examines nothing, and this one examined nothing until now.

### 6. The standard-14 refusal fires on 38 of 43 corpus documents

See the amendment in *The standard-14 metrics* above. The earlier measurement counted 26 files
and did not include `tests/redaction/generated/`, which is the corpus this operation exists to
be measured against.

### What was checked and found sound

Stated so the list above is not read as a survey of everything:

- **`qpdf_oh_replace_stream_data` with a null `/Filter`.** The comment claimed the stream comes
  back re-compressed with a correct `/Length`. Measured on the emitted bytes: the content stream
  and both narrowed `/ToUnicode` streams come out `/FlateDecode` with correct lengths, and
  `qpdf --check` reports no stream-encoding errors. The claim holds.
- **Error latching.** Every new call site drains through `Document::take_error`, and the
  drain-after-every-call rule was already what `prune.rs` established.
- **Handle lifetimes.** `ObjectHandle<'a>` borrows the document, so the lifetime that made this
  hazard a compile error in `prune` makes it one here. `tools/check-handle-identity.py` covers
  the identity half and stays green.
