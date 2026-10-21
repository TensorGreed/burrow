# 0031. Redaction v1 flattens the redacted page

Date: 2026-10-21

## Status

**Proposed**, 2026-10-21, from the owner's decision of that date (decision 55, item 1). It becomes
*Accepted* after round 0 with both reviewers (security-reviewer and code-reviewer) and the owner's
reading. No flatten code is written before this line changes.

Scoped to **decisions**, as [0029](0029-what-redaction-does-and-what-it-refuses-to-do.md) is: what
v1 does, what it refuses, what it discloses, what verification may assert, and what the page may
claim. Construction is `.claude/skills/add-operation`'s job.

**For v1, this record replaces most of ADR 0029. It leaves ADR 0029 as the record for v2.** §10 goes
through 0029 section by section and says which parts hold for v1, which are superseded, and which
are now v2's. The content-stream rewriter, its refusals and their fixtures stay on `main`, held for
v2. Nothing here deletes them.

Carries [0006](0006-wasm-linking-strategy.md)'s R8 and R9 unchanged, and R10 as amended on
2026-09-27 and 2026-10-19. Extends [0022](0022-every-operation-verifies-its-own-output.md) with a new
`Expected` variant (§4). Applies [0004](0004-native-engines.md)'s 2026-10-20 wipe-on-free amendment
to a second engine, which needs a decision that amendment did not make (§7). Uses
[0019](0019-how-split-builds-its-outputs.md)'s reachability argument for what qpdf writes.

The same owner decision moves DECISIONS.md rule 19's S9 instance, and #344 decision 4's S9 gate, to
v2 only. Those edits are made in DECISIONS.md in the same commit as this record, because the owner
has already decided them. §11 summarises them.

## Context

### What v1 was, and the class it kept meeting

ADR 0029's v1 rewrites the redacted page's content streams, the streams they reach and the fonts that
describe them. It decides what to remove with burrow's own geometry walk, and it refuses wherever
burrow, qpdf and PDFium might read the input differently (DECISIONS.md rule 1: refuse rather than
model). Each refusal was sound. **The class it belongs to has no edge that anyone has found.**

Counted on 2026-10-21, at rule level only (the shapes stay where the slices keep them):

- **21 of the 25 ship-blocker rows** that block the redaction tool concern how page content, or an
  object it uses, is read: burrow's walk against PDFium, or PDFium against the other four readers
  (MuPDF, Ghostscript, Poppler, pdf.js). The other four are the heap (#199 and qpdf's freed memory),
  the annotation rule (#136 D3) and #125's discharged signal row. This classification is this
  record's own.
- **At least nine §3 rows added since 2026-10-08** were found only by rendering burrow's `Ok` output
  in a reader other than PDFium (#300, #304, #306, #307, #309, #312, #320, #254, #338). Verification
  reads back through burrow's walk and PDFium, so it could not have seen any of them.
- **Ten content-stream slices were parked as v2 by the same decision** (decision 55, item 3). Of these,
  one was blocked after round 2 on a Critical finding (#256), and one opened when another closed (#404
  after #408). The Type 3 slice (#296) took eight review rounds.

Every round 0 that rendered in five readers found a new disagreement. Each disagreement became a
refusal. Each refusal needed a fixture, a twin, a census and a review. **That is a rate of discovery,
not a count of what is left.**

### What flattening changes, and what it does not

If the redacted page's content is not carried at all, but replaced by the pixels PDFium drew with the
region painted, then nothing about that page's content is left to read. Every reader decodes the same
lossless image, so no reader divergence on that page survives into the output. And the person chose
the region on those same pixels (§2 rule 7), so what they saw is what is written.

It does **not** remove:

- **document-level carriers**: the outline, names, labels, `/Info`, XMP, attachments, JavaScript,
  forms and the structure tree. Each is closed below by a refusal, a drop or a disclosure;
- **objects the redacted page shares** with other pages, or with anything else the output keeps;
- **the heaps**: PDFium decodes the page and holds the unpainted bitmap;
- **page identity**: the page qpdf replaces must be the page PDFium rendered;
- **qpdf's rewrite of every other page**: the same exposure `split` has.

### Facts this record rests on, checked against `main` (b8bf0ba)

| fact | where |
|---|---|
| `pdfium.wasm` is a **prebuilt, already-linked** module. PDFium is not built here, and no static library exists to link against | `engines/build-wasm.sh` line 4; ADR 0004's 2026-10-20 amendment: "PDFium is a prebuilt module and is not rebuilt here" |
| The catalogue **is reachable** now: `qpdf_get_trailer` is accepted (#239), and `/Root` is read through it | `engines/qpdf-untrapped-accepted.toml`; `redact/references.rs`. This changes ADR 0029's *The constraint that shapes every decision below* |
| qpdf writes only objects reachable from the trailer (`preserve_unreferenced` is off by default and burrow never sets it). It **renumbers** every object. It decodes and re-encodes streams with generalized filters (decode level default) | `qpdf/extract.rs`'s `write_out` sets only the deterministic ID and the object-stream mode |
| `qpdf_oh_new_stream`, `qpdf_oh_new_dictionary` and `qpdf_oh_new_name` are **untrapped**: each calls `QTC::TC` outside a trap. None is accepted in `qpdf-untrapped-accepted.toml` | `qpdf-c.cc` 1509, 1551, 1558 (12.4.1) |
| `page_frames` (S2) reads every frame **through qpdf**, natively and on the web. PDFium vouches only for a `/MediaBox` inherited from the page tree, and only natively | `core/burrow-ops/src/redact/mod.rs`; `qpdf/page_frames_tests.rs` |
| The web asks for `max_pixels` = **4,194,304**. The core default is 268,435,456 | `apps/web/src/components/tool-host.ts`; `burrow_types::Limits::DEFAULT`; ADR 0027 §1 |
| The web PDFium bridge already exposes `FPDFBitmap_FillRect` | `apps/web/src/worker/bridge-pdfium.js` |

## Decision

### 1. The pipeline: which engine does what, and in which heap

One region on one page per run, as ADR 0029 and #136 D2 already set.

| step | engine | heap (web / native) | what happens |
|---|---|---|---|
| 1. open | qpdf | redaction worker / caller's thread | Redaction's open: `Limits`, password, repair, page count, the document-level refusals of §2, and the frames (`page_frames`) |
| 2. render | PDFium | render worker's `pdfium.wasm` / the engine thread (ADR 0011) | The page at **150 dpi**, with `FPDF_ANNOT` alone (the flags the selection picture uses, ADR 0029's S4 amendment; §2 rule 9), on opaque white. It is laid out in unrotated user space over the page's effective box, so that the page keeps its `/Rotate` |
| 3. paint | PDFium | the same | The region, given in display points with a top-left origin (ADR 0029's frame amendment), is mapped to bitmap pixels and **widened outward to whole pixels**. Every pixel it touches is filled with opaque black by `FPDFBitmap_FillRect`, **in PDFium's heap**. Until this step, no pixel of this render leaves that heap |
| 4. bind | PDFium side | the same | The painted bitmap's digest is taken. The unpainted render's digest is compared with the selection picture's (§2 rule 7) |
| 5. hand-off | | render worker → redaction worker | Only the painted bitmap, and its digest, cross. The redaction worker digests what it received and refuses on a mismatch |
| 6. replace | qpdf | redaction worker / caller's thread | **In place**: the page dictionary keeps its object identity, so every reference to the page (outline, destinations, other pages' links) still resolves to it. Every key outside §3's allowlist is removed. `/Resources` becomes a new dictionary naming one image XObject. `/Contents` becomes one new stream holding one line that draws it. The catalogue loses `/StructTreeRoot` (§2 rule 1). burrow does not edit any other page. qpdf writes the document |
| 7. verify | qpdf, fresh | redaction worker / caller's thread | §4, on the emitted bytes |
| 8. emit | | redaction worker | One reply, after verification: R8, R9 and R10 (§8) |

**burrow never reads the redacted page's content stream in v1.** No geometry walk, no lexer, no font
resolver runs over it. PDFium parses it in order to draw it, and that is the only reader it has. That
is why none of ADR 0029's content refusals applies to v1 (§10).

The image is 8-bit `/DeviceRGB`, `/FlateDecode`, with no `/DecodeParms`, `/Decode`, `/SMask`, `/Mask`,
`/Interpolate`, `/Intent` or `/OC`. That is the plainest shape every reader decodes alike. The one
line is fixed text, with only the image's size and origin in points varying. The slice writes it
down exactly, and verification compares it byte for byte.

### 2. The v1 rules

Each rule states what refuses, what is dropped and what is disclosed. Rule ids in brackets are
proposals for the slice. A refusal answers with its rule, as every redaction refusal does.

**Rule 1. The structure tree is dropped from the output, and that is disclosed.** The catalogue's
`/StructTreeRoot` is removed, and so is the redacted page's `/StructParents` (by §3's allowlist). This
holds for every document, tagged or not, and costs the whole document its tags, not only the redacted
page.

- A tree is dropped only if nothing the output keeps still names it. **If anything kept names an
  element of the tree**, removing the root leaves that element, and everything it reaches, in the
  output. The redaction then **refuses**, `[structure-tree-kept]`. The reference reader that
  `[annotation-dependent-kept]` already uses (#239) reads every referrer. Verification asserts that
  no object of type `/StructTreeRoot` or `/StructElem` remains (§4, V4).
- **Open for round 0:** whether the catalogue's claim to be tagged (`/MarkInfo`) goes with the tree.
  A document claiming tags it no longer has misleads assistive technology.
- **Disclosed:** §9, item 3.

This replaces ADR 0029's `[structure-tree]` refusal for v1. **It is an acceptance change**: every
tagged document refused today becomes redactable.

**Rule 2. Refuse if any AcroForm field has a widget on a redacted page,** `[form-field-on-page]`. ADR
0029 §5 holds: a refusal keyed on a signal the operation cannot see is a bypass. So the signal fails
closed. Any one of these refuses:

- an `/Annots` entry on the page with `/Subtype /Widget`, whether or not a field claims it;
- an `/Annots` entry that is a field or a field's kid: a `/FT` on itself or on any `/Parent`
  ancestor, or a place in the `/AcroForm /Fields` tree;
- an object in the `/Fields` tree whose `/P` names the page.

Signature fields count as fields. **Fields whose widgets are all on other pages are not refused**.
They, their values and the whole `/AcroForm` are carried unchanged, and that is disclosed (§9). This
narrows ADR 0029's `[acroform-field]`, which refuses every document with a form. **It is an
acceptance change**, and the census must show it on real files.

**Rule 3. Refuse if an annotation on another page references one on a redacted page** through
`/IRT`, `/Parent` or `/Popup`, `[annotation-dependent-kept]` (the #239 rule id, kept). Such an
annotation keeps the redacted page's annotation, with its text and appearance, in the output.

- **Proposed for round 0, as a widening that only refuses more:** refuse when *anything* the output
  keeps names an annotation of the redacted page, not only another page's annotation. That covers
  actions, the catalogue, names, the trailer and fields. The same reference reader already reads
  every referrer. The owner's rule names three keys, and the class is wider than three keys.
- Annotations on the redacted page are **flattened into the picture** where `FPDF_ANNOT` draws them,
  and dropped with `/Annots`. A form widget is refused first, by rule 2.

**Rule 4. The outline, names, page labels, `/Info`, XMP, embedded files and JavaScript are
disclosed, not scrubbed.** All of them sit on the catalogue or the trailer, describe the whole
document, and are carried as qpdf reads them. This is a **class**, not a list. Anything else on the
catalogue or the trailer is carried too and falls under the same disclosure: layer names (optional
content configuration), article threads, document-level actions, associated files, document parts,
output intents and signature data, for example.

Since #239 the catalogue *can* be reached, so scrubbing any of these is now possible. Not doing it is
the owner's decision for v1, not a wall (§*Conditions for revisiting*).

**Rule 5. Objects the redacted page shares with other pages are disclosed.** Precisely: let *S* be
every object reachable from the input page's dictionary, not counting the page tree (`/Parent` and
its ancestors). After the replacement, the **survivors** are the members of *S* still reachable from
the output's trailer, through another page, a page-tree ancestor's inherited `/Resources`, the
`/AcroForm` default resources, or anything else kept. Rules 1 and 3 already refuse the survivors
that are tree elements or annotations.

- The operation **reports the survivors**: a count by kind (content streams, forms, fonts, images,
  other).
- Verification asserts that the survivors in the output are exactly the set reported (§4, V5).
- **The disclosure is conditional on that report being non-empty.** A page that shares nothing does
  not carry a caveat about sharing.
- **Open for round 0:** whether a survivor that is a content stream, or a form the redacted page
  drew, should refuse instead of disclose. Either could hold the region's text in a form that other
  pages carry but never draw.

**Rule 6. 150 dpi, lossless Flate, and refuse above the pixel ceiling,** `[page-too-large-to-flatten]`.
The bitmap is ⌈w·150/72⌉ × ⌈h·150/72⌉ pixels over the page's effective box. It is checked against the
caller's `max_pixels` at `Stage::Pixels`, **before anything is allocated**, as every raster is (ADR
0027 §1). Above the ceiling, the redaction refuses. Nothing is rendered at a lower resolution
instead. "Lossless" is a verification claim, not a hope: the decoded image equals the painted bitmap
(§4, V3). §6 has the numbers.

**Rule 7. Preview and output come from the same render.** Read here as follows. **The picture the
person chooses the region on is the render the output is made from.** The selection picture is
rendered at 150 dpi with the run's flags (the page scales it only to display it), and its digest is
recorded as a number.

At run time PDFium renders the page again, compares that render's digest with the recorded one, and
refuses on a mismatch, `[render-not-reproducible]`. Only then does it paint, still in its own heap.
So the unpainted pixels of the run never leave PDFium's heap. What the person saw is what is
written, apart from the paint.

PDFium is expected to be deterministic for one build, one size and one set of flags. The slice
measures that on the census before relying on it.

If the owner meant the picture *after* the run instead, that is ADR 0029's D9 question (*Open
questions*, item 1).

**Rule 8. The region's edge changes meaning, and the copy says so.** Under ADR 0029, a glyph whose
conservative box meets the region is removed whole, and ink crossing the edge refuses. **Under v1 the
edge is a pixel edge.** Every pixel the region touches is painted, and nothing outside it is. A
glyph, a line of text, an image or a signature crossing the edge **stays visible outside it**, and
part of a word can stay legible. No content refusal stands in for this. The page tells the person to
mark generously (§9, item 2).

**Rule 9. Annotations on the redacted page: what reaches the picture, and what the person is told.**
**This rule is proposed for round 0** (owner, decision 56 item 3). The owner's rules above do not
settle it.

- **The render's flags are `FPDF_ANNOT`, and nothing else.** That is the selection picture's setting
  (ADR 0029's S4 amendment). There is no `FPDF_PRINTING`, so the screen appearance is drawn, not the
  print one. There is no `FPDF_LCD_TEXT` either, so no colour-fringed pixels depend on the display.
- **The picture holds exactly what PDFium draws with those flags.** For each annotation that is
  - the normal appearance, `/AP /N`, in the state `/AS` selects;
  - **the appearance PDFium generates** where `/AP` is missing, for the subtypes it generates one
    for. As ADR 0029's S4 amendment records, a generated appearance is not held to `/Rect`;
  - **nothing** for an annotation flagged Hidden or NoView;
  - **nothing** for a form widget. No form-fill environment exists, and rule 2 refuses widgets first.

  An annotation flagged for print only, and not NoView, is drawn as the screen draws it. The rollover
  and down appearances (`/R`, `/D`) are never drawn. Popup notes are drawn only where PDFium draws
  them closed or open by their flags. **Which subtypes get a generated appearance, and how PDFium
  treats a Popup, is to be measured on the pinned PDFium, not taken from this list** (DECISIONS.md
  rule 2's amendment: a reading of a dependency is a claim).
- **Nothing of any annotation on the redacted page survives in the bytes** except through rule 3's
  refusals and rule 5's survivors. That covers its `/Contents`, rich text, author, subject, popup,
  appearance streams and replies. `/Annots` goes with §3's allowlist. V2 and V5 assert it.
- **Nothing can appear in, or vanish from, the output relative to what the person chose on.** The
  selection picture and the output are one render, bound by digest (rule 7). An annotation that
  another viewer would draw differently, or would not draw at all, is drawn as PDFium drew it in the
  picture the person saw.
- **Disclosed** (§9, item 1): the visible marks of comments on this page are part of the picture.
  Their text, author, replies and popup notes are not kept. Comments hidden on screen are gone.
- **Not v1's:** how the v2 rewriter handles generated appearances. #412 is parked as v2.

### 3. What the redacted page keeps

**The page dictionary's keys after the replacement are `/Type`, `/Parent`, `/MediaBox`, `/Rotate`,
`/Resources` and `/Contents`, and nothing else.** `/MediaBox` is the box PDFium rendered, written
explicitly on the page. `/Rotate` is the page's effective rotation, written explicitly. Inherited
attributes are made explicit rather than left to the page tree.

This is ADR 0029 §2's allowlist, narrowed. Every other key goes. That includes `/Annots`, `/B`, `/AA`,
`/Thumb`, `/StructParents`, `/PieceInfo`, `/Metadata`, `/Group`, `/CropBox` and the other boxes,
`/Tabs`, `/VP`, `/AF`, and every key nobody listed. **Whether `/CropBox` is kept instead, so that the
input's boxes survive as written, is construction.** The frame check (§4, V1) holds either way.

### 4. Verification (ADR 0022)

**`Expected::PageFlattened`**, a proposed name. Not `Redacted`, for the reason ADR 0029's #134
amendment gives against `RegionCleared`'s alternatives. It asserts the following, every item
re-derived from the emitted bytes through a fresh qpdf:

| | asserts |
|---|---|
| V1 | `verify::output`'s half: the input's page count and rotation vector. Also, the flattened page's display frame read from the output (`page_frames`) equals the input's |
| V2 | The flattened page's dictionary has exactly §3's keys. `/Resources` has exactly one entry, `/XObject`, naming exactly one image. `/Contents` is one stream whose decoded bytes equal the line burrow wrote, **byte for byte** |
| V3 | The image dictionary has exactly the keys §1 lists, and its `/Width` and `/Height` are the render's. **Its decoded bytes have the digest bound to the painted bitmap** (§1 step 4), and **every pixel in the widened region is the paint**, compared exactly |
| V4 | The catalogue has no `/StructTreeRoot`, and no object of type `/StructTreeRoot` or `/StructElem` is in the output |
| V5 | The survivors (§2 rule 5) present in the output are **exactly** the set the operation reported. Nothing reachable only from the input page is present. qpdf renumbers, so this is a parallel walk that maps each input page to its output page, not a comparison of object numbers |
| V6 | Every other page's reachable graph equals the input's as qpdf reads it, under the same mapping: unparsed dictionaries equal, and stream data equal **decoded** |

**V6 is not "byte-identical to the input", and the claim must not be worded so.** qpdf renumbers every
object, and re-encodes streams whose filters it generalizes. Whether to set qpdf's decode level to
`none`, so that more of the bytes survive as written, is the slice's choice, and the wording follows
whatever it measures.

**What it leaves undetectable, stated here because the variant's rustdoc must state it:**

- **Page identity across the two engines.** V1 to V6 are qpdf's view. That qpdf's page *N* is the
  page PDFium rendered as *N* is held by §5's checks, not by verification. The residue is two pages
  with identical frames that the two engines order differently: ADR 0022's #111 class, with
  `five-pages-or-six.pdf` as its example.
- **The region-to-pixel arithmetic is same-source.** The same code computes the pixels to paint and
  the pixels to check, so a wrong conversion paints the wrong place and then passes. DECISIONS.md
  rule 19's principle applies (§11). The independent reading is PDFium's own page-to-device
  transform, compared with burrow's on the census and the fixtures. It gates S9.
- **The survivor set is same-source too.** The walk that decides it also checks it. V5 checks that
  the output agrees with the report, not that the report was right. The independent reading is a
  test-time decoded scan (spike 0006's I2) over the census outputs. It gates S9 under rule 19.
- **Whether a survivor, or a document-level carrier, holds the region's content.** These are
  disclosures because no read-back can turn them into assertions.
- **Anything PDFium draws outside the region** that the person meant to cover: rule 8.
- **That another viewer showed the original page the way PDFium did.** The output is PDFium's
  picture. That is the point, and it is also a limit.
- **The heaps.** That is not a property of bytes. Canaries hold it (§7).

The DoD's fake-engine test applies. Fakes return an image with one region pixel unpainted, an image
one row short, an extra page key, a second `/Resources` entry, a content stream one byte off, and a
dropped tree still present. **The mutation that deletes each check makes its test fail.**

### 5. Page identity: S2 becomes the check, and it is not that check yet

The page qpdf replaces must be the page PDFium rendered. If the two engines disagree about which page
is page *N*, qpdf replaces one page with a picture of another. The page that was meant to be redacted
is then written out intact, elsewhere in the document. V1 to V6 would pass.

**`page_frames` does not hold this today.** It reads frames through qpdf only (*Facts*, above). v1
therefore requires the following:

- PDFium's page count equals qpdf's, or the redaction refuses;
- PDFium's frame for the rendered page equals qpdf's frame for page *N*, compared exactly, or the
  redaction refuses. On the web this is the first time PDFium vouches for a frame, so
  `[media-box-unverified]`'s web half can be lifted. That is an acceptance change, and it needs a
  census;
- `[engine-repaired-input]` stays: a repaired file is where the two engines' page trees part.
  `[reference-to-nothing]` and `[reference-unreadable]` stay with it, for the same reason.

The residue is in §4. **Open for round 0:** whether PDFium exposes enough to compare page *object*
identity rather than frames.

### 6. Limits

`max_input_bytes` and `max_pages` stay exact, checked at open as today. Every other limit, with its
strength stated as `burrow_types::Limits`' rustdoc requires:

**`max_pixels` is exact, on the 150 dpi bitmap, before `FPDFBitmap_Create`.** At 150 dpi a page of
*w* × *h* points needs about 4.34·*w*·*h* pixels:

| page | bitmap | pixels | RGB bytes | web (4,194,304) | native default (268,435,456) |
|---|---|--:|--:|:-:|:-:|
| US Letter | 1275 × 1650 | 2,103,750 | 6.3 MB | yes | yes |
| A4 | 1241 × 1754 | 2,176,714 | 6.5 MB | yes | yes |
| US Legal | 1275 × 2100 | 2,677,500 | 8.0 MB | yes | yes |
| Tabloid | 1650 × 2550 | 4,207,500 | 12.6 MB | **refused**, by 13,196 px | yes |
| A3 | 1754 × 2481 | 4,351,674 | 13.1 MB | **refused** | yes |
| A0 | 4967 × 7022 | 34,878,274 | 104.6 MB | **refused** | yes |
| `producer-ocr-scan.pdf` (1748.57 × 2262.86 pt) | 3643 × 4715 | 17,176,745 | 51.5 MB | **refused** | yes |
| the specification's largest page (14,400 pt square) | 30000 × 30000 | 900,000,000 | | **refused** | **refused** |

So the web flattens a page of up to about 966,000 square points (983 pt on a side for a square), about
twice the area of US Letter. Native's default flattens up to about 61.8 million square points (7,864 pt
on a side). Of the 369 pages in the committed fixtures' `page-frames.tsv`, one exceeds the web ceiling:
the OCR scan. The 99-document census is the slice's to run (rule 2).

**Open for the owner:** whether redaction's page asks for a higher `max_pixels` than the other tools,
which is ADR 0027's table, row 1. Tabloid needs 4,207,500 and A3 needs 4,351,674.

**`max_duration_ms` is cooperative.** The render is progressive (ADR 0027's 2026-09-17 amendment),
checkpointed between pauses. Overshoot of up to one engine call remains, as everywhere. ADR 0027 §2a
measured one path-heavy page at 34 s, so the render, not the write, is where a deadline will be
spent. Encoding and the verification decode checkpoint per chunk.

**`max_memory_bytes` is detected, not bounded.** A length-based estimate is made before the render:
the bitmap, its copy across the hand-off, the encoder's output, and verification's decoded copy, so
about four times the RGB bytes. A measured check follows after. **What PDFium's rasteriser allocates
for itself is bounded by nothing** (ADR 0027's *What is bounded*), as for every render.

DECISIONS.md rule 21's ceilings in front of PDFium's page load (`MAX_MARKED_WORK`) apply to this load
as they do to the selection picture's.

### 7. Heap posture (#199)

**The rule, unchanged in its words:** at the reply, no heap holds what the redaction removed. Under
flatten, that is the redacted page's decoded content, and the region's unpainted pixels (#136 D4:
rendered pixels are decoded content).

- **qpdf's heap.** In v1 qpdf never decodes the redacted page's content. The page is replaced before
  the write, and the old streams are unreachable and not written, unless they are survivors. It
  holds the input, which #199 already covers, and the painted bitmap, which carries nothing removed.
  ADR 0004's wipe-on-free stays.
- **PDFium's heap.** It holds the page's decoded content streams, fonts and images, and the unpainted
  bitmap until step 3. The owner's decision (55, item 2) extends wipe-on-free to `pdfium.wasm` in the
  flatten slice. **ADR 0004's hook cannot be extended that way as built.** It is a C object linked
  into burrow's own qpdf build, and `pdfium.wasm` arrives already linked. The slice must therefore
  choose between the following, and the choice goes to the owner (an engine build or acquisition
  question, under CLAUDE.md's *Stop and ask*):
  - **(a)** build PDFium for wasm from source with the hook. That changes the acquisition route and
    needs ADR 0004 and 0008's audit again;
  - **(b)** zero the render worker's **entire linear memory** from JavaScript after the paint and the
    hand-off, before the reply, and then terminate the worker. No engine change is needed, and it
    covers live and freed blocks alike, because the module is discarded;
  - **(c)** both.

  **This record recommends (b) for v1.** Its canary is the qpdf canary's counterpart: a planted text
  and a planted pixel pattern in the region, searched for in the render worker's heap at the reply,
  with a mutation that skips the wipe and turns the canary red.
- **The JavaScript heaps.** At run time only painted pixels cross. The selection picture, which is
  unpainted and is the person's own input, keeps #136 D1's posture: terminate on file change, before
  a run, on cancel and on `pagehide`.
- **Native.** PDFium's heap is the process heap, and it is not wiped. Native posture is M3/M4's, as
  ADR 0004's amendment already says of qpdf.

### 8. Workers, modules, and R8 to R10

- **PDFium stays in the render worker; qpdf stays in the redaction worker.** No worker holds both
  engines.
- **The paint and the digest are redaction-only code in a bundle every tool loads**
  (`burrow_wasm_render_bg.wasm`). DECISIONS.md rule 22 applies: under 0.5% of the page's total, or a
  separate bundle that only `/redact-pdf` loads. The slice measures which.
- **The v2 rewriter is not called by v1.** Whether it stays compiled into the shipped redaction
  module is the slice's measurement and the owner's call (*Open questions*). Shipping unreachable
  content-stream code to visitors costs bytes and buys nothing.
- **R8** is unchanged: one reply carrying output, after verification, from the worker that wrote the
  document. **The painted bitmap is an input to the write, not output**, and it may reach the page as
  the selection picture's counterpart without breaching R8. Round 0 is asked to confirm that reading.
- **R9** is unchanged.
- **R10** is unchanged, and still unmet on the web. Its final hop needs #206's web half, re-scoped to
  the runtime digest only, since the second reading it also carried is v2 by decision 55. The
  hand-off digest in §1 binds the *pixels* to the output. It does not discharge R10, which concerns
  the bytes verified and the bytes delivered.

### 9. What the page may claim, and the copy

ADR 0029 §7's voice holds: every limit carries its reason, and the product is "Not Only PDF", never
"burrow". **What changes:**

- **The page may say that the marked area is covered in black in a picture of the page, and that
  every PDF reader shows the same picture.** #136's list said "it never says the area is blank" and
  "never says pictures or shapes are redacted". Under v1 both are true of the marked area, to the
  nearest pixel, and verification asserts it (V3).
- **It still never says** that the secret is gone, permanent, secure or compliant. It never says
  anything about what lies outside the marked area, that metadata is scrubbed, or that the rest of
  the document is unchanged byte for byte (V6).

The disclosures, as a draft. Lane S1's copy and `redact-copy.ts`'s test take their wording from this
list:

1. **The page becomes a picture.** Its text can no longer be selected, searched or read aloud. Its
   links are gone. The comments on it are no longer comments. The marks you could see are part of
   the picture. Their text, author, replies and pop-up notes are not kept, and comments hidden on
   screen are gone.
2. **Only the area you marked is covered, to the nearest pixel.** A word, a line or a signature that
   crosses the edge stays visible outside it. Mark generously.
3. **The document's accessibility tags are removed**, on every page, not only this one.
4. **What sits outside the pages is not changed:** the title and keywords, the document's XMP
   metadata, attached files, bookmarks, named destinations, page labels, layer names and any
   JavaScript. Check File → Properties, the bookmarks panel and the attachments panel. If one of
   them holds what you removed, it is still in this file.
5. *(Only when §2 rule 5's report is non-empty.)* **Parts of this page that other parts of the
   document also use stay in the file**, such as fonts, images and drawings, with whatever they
   contained.
6. **Form fields on other pages, and their values, are not changed.**
7. **The file usually grows**, because the page is now an image.

### 10. ADR 0029, section by section

| 0029 | for v1 |
|---|---|
| *Context* | Kept as history. Its central measurement, that text survives in many places besides the glyphs, is why flatten drops the page's content wholesale |
| *The constraint that shapes every decision below* (the catalogue wall) | **Superseded as fact**: #239 made the trailer, and through it the catalogue, reachable. v1 writes to the catalogue only to remove `/StructTreeRoot` |
| §1 rewrite content streams and fonts | **v2** |
| §2 page-key allowlist | **Kept, narrowed** to §3's six keys |
| §3 per-channel table | **Superseded for v1** by §2 here. Content-stream channels (rows 1–9, 18–22 of spike 0006) are handled by not carrying the content. The page's `/Thumb` and page metadata go by §3. Annotations are flattened or refused (rules 2 and 3). Optional content hidden on the page is not drawn and not carried; layer names are disclosed (rule 4). Images and vector paths in the region are painted. **Every content refusal in the table is v2's**, because burrow does not read the content |
| §3 rows kept for v1 | The rows that keep qpdf's object graph the one PDFium reads: `[engine-repaired-input]`, `[reference-to-nothing]`, `[reference-unreadable]`, the 2026-10-14 object-syntax rows (names outside the allowlist, vertical tab, a value qpdf drops on write), `[page-frame-unreadable]` and `[user-unit-not-one]`. `[annotation-dependent-kept]` is reused by rule 3 |
| §3 rows changed for v1 | `[acroform-field]` narrowed (rule 2). `[structure-tree]` replaced by the drop (rule 1). `[media-box-unverified]`'s web half lifted by §5. `[page-attribute-inherited]` is not needed, because inherited attributes are made explicit (§3) and inherited resources are survivors (rule 5). `[annotation-in-region]` (#136 D3) cannot arise as D3 framed it, since nothing in the region is silently kept; whether it is retired for v1 is the owner's (*Open questions*) |
| §4 `/Contents` per stream | **v2** |
| §5 refusal keyed on what the operation can see | **Kept as the rule.** It binds rules 1–3 here: each signal needs an evasion fixture it fires on |
| §6 what verification asserts | **Superseded for v1** by §4 here. The rule that "verified" is never worded as "the secret is gone" is kept |
| §6, the 2026-09-27 PDFium second opinion (#206) | **v2** |
| §7 copy | **Superseded for v1** by §9 here. Its voice is kept |
| §8 a removal nothing observed | **Kept.** V3 observes the paint. V5 observes that the page's own objects went, against an input set that had them |
| *Consequences*, *Conditions*, *Alternatives* | v2's, except where §*Conditions for revisiting* here repeats them |
| Amendments: inline-image extent; shared Form XObject; Type 3; `/ToUnicode`; widths; marked content; strokes; #224 to #344 | **v2** |
| Amendment: redaction's own wasm module | **Kept**: the qpdf half stays in its own module. §8 covers the render half |
| Amendment: the region's frame | **Kept**: display points, top-left |
| Amendment: the order of steps, and any failure discards the document | **Kept** |
| Amendment: #134, redaction is an operation and verifies | **Superseded** by §4 here: the variant changes |
| Amendment: S4, `flags` a default | **Kept, and extended**: the flatten render uses the selection picture's flags |

### 11. DECISIONS.md, amended by the same decision

- **Rule 19:** its principle stands, that a same-source check may gate a merge and never the launch.
  Its S9 instance (the PDFium ink read-back, after #344) and rule 17's 2026-10-17 sentence ("the
  PDFium ink read-back still gates S9 via #206's ink half") **now apply to v2 only**. For v1, the
  same-source assertions are named in §4: the region-to-pixel arithmetic and the survivor set. Their
  independent readings gate S9.
- **#344 decision 4** (owner, 2026-10-15): #206's ink half asserting PDFium's tight character boxes
  against the region, as an S9 gate on the web path, **applies to v2 only**. v1 does not rely on
  `/FontBBox`, or on any glyph box, at all. ADR 0029 gets a short dated amendment saying the same,
  because its #344 amendment names that gate. The ROADMAP #344 row moves with the other content
  rows in the ROADMAP restructure, which is another lane's.

## Consequences

**What becomes easier.**

- **The reader-divergence class has nothing to act on for the redacted page.** The 21 ship-blocker
  rows that concern how content is read become v2's (the ROADMAP restructure is another lane's).
- **Five-reader agreement on the redacted page holds by construction**, since all five decode one
  lossless image. It is still measured by DECISIONS.md rule 17's 2026-10-19 five-reader twin render,
  never claimed from reading the code.
- **Verification asserts something exact about pixels**: V3, which ADR 0029 could never assert.
- **Images, shapes, scans, Type 3 glyphs and annotations in the region are covered** rather than
  refused. A scan, the commonest document there is, is redactable within the pixel ceiling.

**What becomes harder, or is lost.**

- **The redacted page loses** its text layer, its links, its annotations as objects, its form fields
  (by refusal) and its tags. Its fidelity is capped at 150 dpi. Colour is converted to RGB, so CMYK,
  spot colours and ICC intent are lost, and transparency is flattened. This is the price of the
  decision, and §9 says it to the person.
- **Size grows.** About 6.3 MB of raw RGB for a Letter page before Flate, with the ratio content-
  dependent (to be measured: high for text pages, low for photographs).
- **Every untouched page still goes through qpdf's rewrite.** That is `split`'s exposure: qpdf
  renumbers, re-encodes generalized filters, and drops what it cannot read. Every document-level
  object is carried. "Other pages untouched" means burrow does not edit them, not that their bytes
  are unchanged.
- **New qpdf C entry points.** The new image stream, dictionaries and names need
  `qpdf_oh_new_stream`, `qpdf_oh_new_dictionary` and `qpdf_oh_new_name`, each untrapped today. Each
  needs an argued entry in `engines/qpdf-untrapped-accepted.toml` on `qpdf_oh_new_integer`'s argument
  (it allocates, and parses nothing), or a trapped route. Their cost to `qpdf.wasm`, which every tool
  loads, is measured and reported, not assumed.
- **The web refuses Tabloid and A3** at 150 dpi under today's `max_pixels` (§6).
- **Native heaps are not wiped** (§7).
- **Rule 17 tier: acceptance.** v1 accepts much of what ADR 0029 refuses: every content refusal, the
  tree, forms elsewhere, and the web's inherited media box. So the slice gets round 0 and both
  reviewers. Its census shows at least one real file moving from refused to `Ok` for each acceptance.
  It also measures the new refusals (rules 2, 3, 6, 7, `[structure-tree-kept]`) against rule 2's 1%
  bar.

**What is parked.** The content-stream slices decision 55 parked (#255, #256, clip-path, soft mask,
the edge rule, #404, F-2 and trailing spacing, 5n, #344, slice 2b) are v2. Their branches are kept,
and nothing in this record reopens them.

## Conditions for revisiting

- **v2 is ready**: the content-stream rewriter's blockers discharged, with its own final spec review.
  Then a text-preserving redaction can be offered beside flattening. It does not replace it.
- **The resolution.** Measured complaints about legibility or size, or the owner raising redaction's
  `max_pixels` (§6).
- **A real document measured with region content in a survivor** (rule 5) or in a document-level
  carrier (rule 4). Disclosure would then become a refusal or a scrub, a scrub being possible since
  #239.
- **PDFium exposing page-object identity**, which would close §5's residue.
- **PDFium built from source for wasm**, which would make option (a) in §7 available.

## Alternatives considered

**Keep ADR 0029's rewriter as v1.** This is v2. It is rejected for v1 on the rate of discovery in
*Context*, not on any single defect. Its strength stands: it keeps the page's text everywhere outside
the region.

**Flatten every page.** It closes no document-level carrier that rule 4 leaves open, and it costs every
page its text for a region on one. It is rejected as disproportionate.

**Copy the bitmap out of PDFium and paint it in Rust or JavaScript.** It is simpler to write. It is
rejected because the unpainted region would then exist in a second heap and cross a worker boundary,
and the owner's decision paints in PDFium's heap.

**JPEG, or a lower resolution.** It is smaller. It is rejected because lossy encoding means V3 cannot
assert the paint exactly, and the record would be back to "probably blank".

**300 dpi.** Letter becomes 8.4 Mpx, refused on the web, at four times the bytes. It is rejected for
v1, and it is a condition for revisiting.

**A black rectangle over the original content.** This is concealment. Spike 0006 measured two readers
reading the text out from under it. It is rejected, as it was in ADR 0029.

**Scrub the document-level carriers.** Possible since #239. It is the owner's decision to disclose for
v1, and each scrub would be a decision of its own.

## Open questions, for round 0 and the owner

1. **D9 ("no after-picture of the result").** The output's pixels are exact (V3), so a picture of the
   result can no longer mislead the way ADR 0029's S4 amendment feared. Whether to show one is worth
   revisiting. It is not decided here.
2. **Rule 7's reading.** This record reads it as one render for selection and output, bound by a digest.
   The owner confirms the reading.
3. **The web's `max_pixels` for redaction** (§6).
4. **Rule 3's widening** to any kept referrer of the page's annotations.
5. **Rule 1's** `[structure-tree-kept]`, and whether `/MarkInfo` goes.
6. **Rule 5:** disclosure, or refusal for survivors that are content streams or forms.
7. **§7's wipe route** for `pdfium.wasm`: (a), (b) or (c).
8. **V6's wording**, and qpdf's decode level.
9. **Whether the v2 rewriter ships**, compiled into v1's module.
10. **`[annotation-in-region]`** (#136 D3) for v1.
11. **§5's residue**, and whether page-object identity can be had from PDFium.
12. **The §3 rows kept for v1** (§10): confirm that the list is complete.
13. **Rule 9**: the render flags, which annotations reach the picture, and the disclosure. This is
    proposed, and it is not yet decided.
