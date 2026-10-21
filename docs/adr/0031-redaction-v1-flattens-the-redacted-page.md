# 0031. Redaction v1 flattens the redacted page

Date: 2026-10-21

## Status

**Proposed**, 2026-10-21, from the owner's decision of that date (decision 55, item 1). Revised the
same day after round 0, under the owner's decision 57 (*Round 0*, below). It becomes *Accepted* after
round 1 with both reviewers (security-reviewer and code-reviewer) and the owner's reading. No
flatten code is written before this line changes.

Scoped to **decisions**, as [0029](0029-what-redaction-does-and-what-it-refuses-to-do.md) is: what
v1 does, what it refuses, what it discloses, what verification may assert, and what the page may
claim. Construction is `.claude/skills/add-operation`'s job.

**For v1, this record replaces most of ADR 0029. It leaves ADR 0029 as the record for v2.** §10 goes
through 0029 section by section and says which parts hold for v1, which are superseded, and which
are now v2's. The content-stream rewriter, its refusals and their fixtures stay on `main`, held for
v2. Nothing here deletes them (§8 says how v2 stays callable).

Carries [0006](0006-wasm-linking-strategy.md)'s R8 and R9, and R10 as amended on 2026-09-27 and
2026-10-19; §8 says how R9's words meet a bitmap crossing between workers. Extends
[0022](0022-every-operation-verifies-its-own-output.md) with a new `Expected` variant (§4). Applies
[0004](0004-native-engines.md)'s 2026-10-20 wipe-on-free amendment to a second engine, by a route
the owner chose (§7). Uses [0019](0019-how-split-builds-its-outputs.md)'s reachability argument for
what qpdf writes.

The same owner decision moves DECISIONS.md rule 19's S9 instance, and #344 decision 4's S9 gate, to
v2 only. **Those changes are the owner's, decided independently of this record's status** (decision
57, item 9): they stand while this record is Proposed, and they stand if it is rejected. §11
summarises them.

### Round 0

Both reviewers ran round 0 on `e9fad74` on 2026-10-21, reasoning only, without measurements:

| reviewer | model | on | stop condition |
|---|---|---|---|
| security-reviewer | Opus | `e9fad74` | **not triggered**: every class found closes by a refusal or a disclosure; three need the slice to build the closure |
| code-reviewer | Opus | `e9fad74` | **not triggered**: no document-level class that cannot be closed |

The stop condition was: a document-level class that neither a refusal nor a disclosure can close,
which would send the decision back to the owner. Neither review met it. Their findings are folded in
below, under decision 57, which adopts the code-reviewer's three blocking corrections, every
should-fix, and the security-reviewer's four High fixes, and decides ten open points. Shapes the
security review recorded privately are worded here at the level of the rule. The reviews themselves
are kept where rule 24 keeps every round.

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
pixels from one lossless image, so no reader divergence on that page survives into the output. That
is expected by construction, and measured by the five-reader twin render (*Fixtures and census*). And
the person chose the region on those same pixels (§2 rule 7), so what they saw is what is written.

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
| **`qpdf_oh_new_stream` is not parse-free.** `newStream` takes `nextObjGen`, which calls `getObjectCount`, which calls `fixDanglingReferences`, which resolves the whole cross-reference table the first time it runs. That can parse and repair the file mid-operation. `new_name` and `new_dictionary` touch only the object cache | `QPDF.cc` 386–390; `QPDF_objects.cc` 1240–1258 and 1892–1898 (12.4.1). Found by the code review |
| `page_frames` (S2) reads every frame **through qpdf**, natively and on the web. PDFium vouches only for a `/MediaBox` inherited from the page tree, and only natively | `core/burrow-ops/src/redact/mod.rs`; `qpdf/page_frames_tests.rs` |
| **`MAX_MARKED_WORK` is computed by burrow's content walk** and checked before PDFium's read-back load. v1 runs no walk, so it has nothing to apply. The render path's guard is `estimate::before_page_load`, then the measured memory check | `pdfium/read_back.rs` lines 11–17; DECISIONS.md rule 21's *Does not cover* |
| **No Flate implementation exists in burrow's Rust.** Encoding is qpdf's, through zlib, on the write; decoding for verification is qpdf's stream-data read. Each is one engine call that nothing can checkpoint | `Cargo.lock` (no Flate crate); `qpdf/extract.rs` |
| qpdf refuses to decode a stream past **256 MiB** per filter | CLAUDE.md, *Limits*; ADR 0029 (#175) |
| PDFium's bitmaps are **4 bytes per pixel** (BGRx), not RGB | `FPDFBitmap_Create` |
| The web asks for `max_pixels` = **4,194,304**. The core default is 268,435,456 | `apps/web/src/components/tool-host.ts`; `burrow_types::Limits::DEFAULT`; ADR 0027 §1 |
| The web PDFium bridge already exposes `FPDFBitmap_FillRect` | `apps/web/src/worker/bridge-pdfium.js` |

## Decision

### 1. The pipeline: which engine does what, and in which heap

One region on one page per run, as ADR 0029 and #136 D2 already set. **One deadline covers the whole
run**, across both workers: the render worker is given what is left of it, not a fresh one.

| step | engine | heap (web / native) | what happens |
|---|---|---|---|
| 1. open | qpdf | redaction worker / caller's thread | Redaction's open: `Limits`, password, page count, the document-level refusals of §2 and §5, and the frames (`page_frames`). **The whole cross-reference table is resolved here, inside the trap** (*Resolution at open*, below). Any repair refuses, `[engine-repaired-input]` |
| 2. render | PDFium | render worker's `pdfium.wasm` / the engine thread (ADR 0011) | The page at **150 dpi**, with `FPDF_ANNOT` alone (the flags the selection picture uses, ADR 0029's S4 amendment; §2 rule 9), no form-fill handle, on opaque white. It is laid out in unrotated user space over the page's effective box, so that the page keeps its `/Rotate`. **The same render route as the selection picture** (§2 rule 7) |
| 3. bind | PDFium side | the same | The unpainted render's digest is taken **inside the render worker's linear memory** and compared with the selection picture's record (§2 rule 7). No JavaScript-owned copy of the unpainted pixels is made |
| 4. paint | PDFium | the same | The region, given in display points with a top-left origin (ADR 0029's frame amendment), is mapped to bitmap pixels **by PDFium's own page-to-device transform** (`FPDF_PageToDevice`, decision 57 item 5), widened outward to whole pixels and then by one more pixel on each side (§2 rule 8). Every pixel in that rectangle is filled with opaque black by `FPDFBitmap_FillRect`, **in PDFium's heap**. Until this step, no pixel of this render leaves the render worker's linear memories |
| 5. hand-off | | render worker → redaction worker | The painted bitmap, converted to RGB, and its digest cross, with the fields §8 lists and nothing else. It does not pass through the page's code as content (§8). The redaction worker digests what it received and refuses on a mismatch. The render worker is then zeroed and terminated (§7) |
| 6. cross-check | burrow | redaction worker / caller's thread | Before allocating, the redaction worker re-checks `max_pixels`, and that the buffer's length is exactly *W*·*H*·3, by checked arithmetic. Then **burrow's own arithmetic** maps the region to pixels from qpdf's frame, and every pixel of that rectangle must be the paint. Otherwise it refuses, `[paint-disagrees]`. Two sources, at run time |
| 7. replace | qpdf | the same | **In place**: the page dictionary keeps its object identity, so every reference to the page (outline, destinations, other pages' links) still resolves to it. Every key outside §3's allowlist is removed. `/Resources` becomes a new dictionary naming one image XObject. `/Contents` becomes one new stream holding one line that draws it. The catalogue loses `/StructTreeRoot` and `/MarkInfo` (§2 rule 1). burrow does not edit any other page. **`[engine-repaired-input]` is checked again after the constructors**, then qpdf writes the document |
| 8. verify | qpdf, fresh | the same | §4, on the emitted bytes |
| 9. emit | | redaction worker | One reply, after verification: R8, R9 and R10 (§8) |

**burrow never reads the redacted page's content stream in v1.** No geometry walk, no lexer, no font
resolver runs over it. PDFium parses it in order to draw it, and that is the only reader it has. That
is why none of ADR 0029's content refusals applies to v1 (§10).

The image is 8-bit `/DeviceRGB`, `/FlateDecode`, with no `/DecodeParms`, `/Decode`, `/SMask`, `/Mask`,
`/Interpolate`, `/Intent` or `/OC`. That is the plainest shape every reader decodes alike. **qpdf
encodes it, through zlib, inside the write**: one engine call. The one line is fixed text, with only
the image's size and origin in points varying. The slice writes it down exactly, and verification
compares it byte for byte.

**Resolution at open.** Because `qpdf_oh_new_stream` can resolve the cross-reference table the first
time it runs (*Facts*), a resolution, and any repair it makes, must not first happen at step 7, outside
the trap, where a repair would change the page tree after every check had run on the old one. So the
open forces qpdf's whole-file resolution **inside the trap**, and asserts that it ran. qpdf's C API
offers no direct entry point for this, so the slice names the route: a trapped call that reaches it,
or a burrow shim in the qpdf build ADR 0004 already extends. After it, `new_stream` finds the table
resolved and parses nothing, which is the argument its entry in `qpdf-untrapped-accepted.toml` rests
on. `new_dictionary` and `new_name` touch only the cache and take `qpdf_oh_new_integer`'s argument. The
repair refusal is read again after the constructors, so that the argument is checked, not assumed.

### 2. The v1 rules

Each rule states what refuses, what is dropped and what is disclosed. Rule ids in brackets are
proposals for the slice. A refusal answers with its rule, as every redaction refusal does. Every
signal here is keyed on what the operation can see (ADR 0029 §5), and each has an evasion fixture
(*Fixtures and census*).

**A walk that stops at pages.** Rules 1, 3 and 5 each walk a set of objects. Each walk **stops at, and
does not enter, every page dictionary, every page-tree node and the catalogue**, whatever key leads
there (an annotation's `/P`, a destination, a structure element's `/Pg`). Otherwise a walk from one
page reaches every page.

**Rule 1. The structure tree is dropped from the output, and that is disclosed.** The catalogue's
`/StructTreeRoot` and `/MarkInfo` are removed (decision 57, item 1; a document claiming tags it no
longer has misleads assistive technology), and so is the redacted page's `/StructParents` (by §3's
allowlist). This holds for every document, tagged or not, and costs the whole document its tags, not
only the redacted page.

- **The tree's elements** are the objects reachable from the input's `/StructTreeRoot` through the
  tree's own entries: its kids, its parent tree, its ID tree, its role and class maps, its attribute
  and namespace objects, PDF 2.0's additions among them. The walk stops at pages (above), and at what
  the tree points at without owning it: the content streams and objects that marked-content and
  object references name. **Membership is by reachability, not by a declared `/Type`**, since a
  structure element need not declare one. The slice writes the entry list down, with a fixture for
  each entry.
- **If any element is still reachable from the output's trailer**, by any kept referrer, the
  redaction **refuses**, `[structure-tree-kept]`. Removing the root would leave that element, and
  everything it reaches, in the output.
- Verification (§4, V4) asserts that **no member of that set** is in the output, by the parallel walk
  V5 uses. A check keyed on `/Type` would miss an untyped element.
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

**Rule 2a. Refuse any XFA form,** `[xfa-form]` (decision 57, item 2). An `/XFA` key on the
`/AcroForm`, of any value or type, or a `/NeedsRendering` key on the catalogue, refuses. **Key
presence, never the parsed value**: a reader that renders XFA draws pages that PDFium does not, and
carries data that no rule here reads.

**Rule 3. Refuse if anything the output keeps reaches an annotation of the redacted page,**
`[annotation-dependent-kept]` (the #239 rule id, kept). Round 0 found the narrower wording (another
page's annotation, through `/IRT`, `/Parent` or `/Popup`) was not what rule 5 relied on, and the owner
adopted the widening (decision 57, item 1):

- **An annotation survivor refuses.** Let *A* be the redacted page's annotations and everything
  reachable from them (their appearance streams, popups, replies and rich text), by the walk that
  stops at pages. If any member of *A* is reachable from the output's trailer, by any path, the
  redaction refuses. That covers another page's annotation, an annotation another page's `/Annots`
  also lists, an `/Annots` array another page shares (#240), actions, the catalogue, names, the
  trailer and fields.
- **An appearance stream shared with something kept counts as an annotation survivor.** So it
  refuses too.
- Annotations on the redacted page are **flattened into the picture** where `FPDF_ANNOT` draws them
  (rule 9), and dropped with `/Annots`. A form widget is refused first, by rule 2.

**Rule 4. The outline, names, page labels, `/Info`, XMP, embedded files and JavaScript are
disclosed, not scrubbed.** All of them sit on the catalogue or the trailer, describe the whole
document, and are carried as qpdf reads them. This is a **class**, not a list. Anything else on the
catalogue or the trailer is carried too and falls under the same disclosure: layer names (optional
content configuration), article threads, document-level actions, associated files, application data,
document parts, output intents and signature data, for example. Associated files and application data
can hold previews of pages, and the disclosure names previews (§9, item 4).

Since #239 the catalogue *can* be reached, so scrubbing any of these is now possible. Not doing it is
the owner's decision for v1, not a wall (§*Conditions for revisiting*).

- **One exception refuses: page thumbnails in the document's XMP metadata,** `[metadata-thumbnail]`
  (decision 57, item 3). A thumbnail of the redacted page is a picture of the region. If the
  catalogue's metadata stream carries a thumbnail property, the redaction refuses. The signal is the
  property's presence, never its parsed value, and a metadata stream that cannot be decoded within
  `Limits` refuses too. **Census-conditional:** the census reports how often it fires. **If it refuses
  more than 1% of real documents, the rule becomes a strip instead**: the XMP thumbnail is removed on
  output, and the refusal goes. That change gets its own review, as a scrub is a write.
- A signature anywhere in the document no longer validates after the write. That is disclosed (§9,
  item 6).

**Rule 5. Objects the redacted page shares are disclosed, or refused.** Precisely: let *S* be every
object reachable from the input page's dictionary, by the walk that stops at pages. After the
replacement, the **survivors** are the members of *S* still reachable from the output's trailer:
through another page, a page-tree node's inherited `/Resources`, the `/AcroForm` default resources,
the names tree's appearances or templates, or anything else kept. Rules 1 and 3 already refuse the
survivors that are tree elements or annotations.

- **A drawing only the redacted page used** is a survivor that is a content stream or a Form XObject
  which the redacted page drew and **no kept page draws**. It can hold the region's text in a form
  that the output carries but never shows. Examples are an inherited form that only this page drew,
  and a page that is not in the page tree sharing this page's content. **Census-conditional**
  (decision 57, item 4): **if refusing it costs under 1% of real documents, it refuses**,
  `[drawing-survives]`. **Otherwise it is disclosed**, and the disclosure names drawings (§9, item 5).
- **Fonts and images are disclosed.** A subset font with a `/ToUnicode` map reveals which characters
  the page used. An image reveals itself. Neither is drawn anywhere by the output's redacted page.
- The operation **reports the survivors**: a count by kind (content streams, forms, fonts, images,
  other).
- Verification asserts that the survivors in the output are exactly the set reported (§4, V5).
- **The disclosure is conditional on that report being non-empty.** A page that shares nothing does
  not carry a caveat about sharing.

**Rule 6. 150 dpi, lossless Flate, and refuse above the pixel ceiling,** `[page-too-large-to-flatten]`.
The bitmap is ⌈w·150/72⌉ × ⌈h·150/72⌉ pixels over the page's effective box. It is checked against the
caller's `max_pixels` at `Stage::Pixels`, **before anything is allocated**, as every raster is (ADR
0027 §1), and against **redaction's own ceiling** of 89,478,485 pixels, the most whose RGB bytes
qpdf's 256 MiB decode ceiling lets verification read back (§6). Above either, the redaction refuses.
Nothing is rendered at a lower resolution instead. "Lossless" is a verification claim, not a hope:
the decoded image equals the painted bitmap (§4, V3).

- **The web keeps `max_pixels` = 4,194,304** for redaction, as for every tool (decision 57, item 7).
  So the web refuses Tabloid and A3. The census reports how many real pages that refuses. The number
  is reported, and does not move the ceiling.

**Rule 7. Preview and output come from the same render.** Read here as follows. **The picture the
person chooses the region on is the render the output is made from.** The selection picture is
rendered at 150 dpi with the run's flags and the run's render route (the page scales it only to
display it). Its record binds four things: the **input file's digest**, the **page index**, the
bitmap's width and height, and the **picture's digest**. A digest of pixels alone would bind the
picture, not which file and page it came from.

At run time PDFium renders the page again and the render worker compares the run's input digest,
page index, size and pixel digest with the record. **On any mismatch it refuses,**
`[render-not-reproducible]`. Only then does it paint, still in its own heap. So the unpainted pixels
of the run never leave the render worker. What the person saw is what is written, apart from the
paint. **The record is required natively too**, not only on the web.

- **The digest's canonical form** is SHA-256 over the width and height, as two 32-bit big-endian
  integers, followed by the pixels as RGB, three bytes each, rows top to bottom, with no padding. The
  4-byte BGRx bitmap is converted before the digest, by one function both sides share.
- **Selection and run use one route**: one orientation, the same progressive or one-shot call, the
  same flags, the same fill. PDFium is expected to be deterministic for one build, one size and one
  set of flags. **That is a claim until the census measures it** (*Fixtures and census*), across
  progressive and one-shot rendering, pauses, the native font mapper and the bitmap fill.
- Rule 7 holds **within one build**. Whether wasm, native x64 and native arm64 PDFium produce the same
  pixels is unmeasured, and it matters only to goldens and the native–web differential (§4).

If the owner meant the picture *after* the run instead, that is ADR 0029's D9 question (*Open
questions*, item 1).

**Rule 8. The region's edge changes meaning, and the copy says so.** Under ADR 0029, a glyph whose
conservative box meets the region is removed whole, and ink crossing the edge refuses. **Under v1 the
edge is a pixel edge.** Every pixel the region touches is painted, and **one more pixel on each side**,
because hinting and hairlines can put ink up to about a pixel beyond where the arithmetic places it.
A glyph, a line of text, an image or a signature crossing the edge **stays visible outside it**, and
part of a word can stay legible. No content refusal stands in for this. The page tells the person to
mark generously (§9, item 2), and the operation's rustdoc states the pixel-edge semantics.

What lies outside the painted rectangle is **not detectable at run time**: no check can say whether
something there is what the person meant to cover.

**Rule 9. Annotations on the redacted page: what reaches the picture, and what the person is told.**
The owner kept this rule (decision 56, item 3), with the reviewers' measurements and an added
disclosure.

- **The render's flags are `FPDF_ANNOT`, and nothing else**, and no form-fill handle exists. That is
  the selection picture's setting (ADR 0029's S4 amendment), and the selection render uses the same
  flags and also no form-fill handle. There is no `FPDF_PRINTING`, so the screen appearance is drawn,
  not the print one. There is no `FPDF_LCD_TEXT` either, so no colour-fringed pixels depend on the
  display.
- **The picture holds exactly what PDFium draws with those flags.** For each annotation that is
  expected to be
  - the normal appearance, `/AP /N`, in the state `/AS` selects;
  - **the appearance PDFium generates** where `/AP` is missing, for the subtypes it generates one
    for. As ADR 0029's S4 amendment records, a generated appearance is not held to `/Rect`;
  - **nothing** for an annotation flagged Hidden or NoView;
  - **nothing** for a form widget. No form-fill environment exists, and rule 2 refuses widgets first.

  An annotation flagged for print only, and not NoView, is drawn as the screen draws it. The rollover
  and down appearances (`/R`, `/D`) are never drawn. Popup notes are drawn only where PDFium draws
  them closed or open by their flags, and **an open popup PDFium draws shows the comment's text in the
  picture**.
- **That list is a reading of a dependency, and it is measured on the pinned PDFium before it is
  relied on** (DECISIONS.md rule 2's amendment). The measurements: which subtypes get a generated
  appearance, and how far it reaches; how PDFium draws a Popup, closed and open; an `/AP /N` that is a
  dictionary of states with no `/AS`; each of the flags NoView, Print, Hidden and ToggleNoView; an
  annotation under optional content; and an annotation whose `/Rect` lies outside the page's box.
- **Nothing of any annotation on the redacted page survives in the bytes**, except where rule 3
  refuses and rule 5 reports. That covers its `/Contents`, rich text, author, subject, popup,
  appearance streams and replies. `/Annots` goes with §3's allowlist. V2 and V5 assert it.
- **Nothing can appear in, or vanish from, the output relative to what the person chose on.** The
  selection picture and the output are one render, bound by its record (rule 7). An annotation that
  another viewer would draw differently, or would not draw at all, is drawn as PDFium drew it in the
  picture the person saw.
- **Disclosed** (§9, item 1): the visible marks of comments on this page are part of the picture,
  including any pop-up note shown open. Their text, author and replies are not kept as comments.
  Comments hidden on screen are gone.
- **Not v1's:** how the v2 rewriter handles generated appearances. #412 is parked as v2.

### 3. What the redacted page keeps

**The page dictionary's keys after the replacement are `/Type`, `/Parent`, `/MediaBox`, `/CropBox`,
`/Rotate`, `/Resources` and `/Contents`, and nothing else.**

- `/MediaBox` is the box PDFium rendered: the page's effective box, the crop clipped to the media box,
  written explicitly on the page.
- **`/CropBox` is always written, equal to `/MediaBox`** (decision 57, item 8). A crop inherited from
  the page tree would otherwise reach the replaced page and cut the picture. Writing it removes that
  case, so `[page-attribute-inherited]`'s `/CropBox` half is not needed for v1. **That is an
  acceptance**, and the census counts it as one.
- `/Rotate` is the page's **effective** rotation, inherited or not, written explicitly. V1 compares
  the effective rotation read from the output with the input's.

This is ADR 0029 §2's allowlist, narrowed. Every other key goes. That includes `/Annots`, `/B`, `/AA`,
`/Thumb`, `/StructParents`, `/PieceInfo`, `/Metadata`, `/Group`, the other boxes, `/Tabs`, `/VP`,
`/AF`, `/UserUnit`, and every key nobody listed. The frame check (§4, V1) holds the display frame
equal to the input's.

### 4. Verification (ADR 0022)

**`Expected::PageFlattened`**, a proposed name. Not `Redacted`, for the reason ADR 0029's #134
amendment gives against `RegionCleared`'s alternatives. It asserts the following, every item
re-derived from the emitted bytes through a fresh qpdf:

| | asserts |
|---|---|
| V1 | `verify::output`'s half: the input's page count and its **effective** rotation vector. Also, every page's display frame read from the output (`page_frames`) equals the input's |
| V2 | The flattened page's dictionary has exactly §3's seven keys, with `/CropBox` equal to `/MediaBox`. `/Resources` has exactly one entry, `/XObject`, naming exactly one image. `/Contents` is one stream whose decoded bytes equal the line burrow wrote, **byte for byte** |
| V3 | The image dictionary has exactly the keys §1 lists, and its `/Width` and `/Height` are the render's. Its decoded length is checked to be exactly *W*·*H*·3 **before it is read**, and the read is capped there. **Its decoded bytes have the digest bound to the painted bitmap** (§1 step 5), and **every pixel in the painted rectangle is the paint**, compared exactly |
| V4 | The catalogue has no `/StructTreeRoot` and no `/MarkInfo`, and **no member of rule 1's element set** is in the output, by V5's parallel walk |
| V5 | The survivors (§2 rule 5) present in the output are **exactly** the set the operation reported. Nothing reachable only from the input page is present. qpdf renumbers, so this is a parallel walk that maps each input object to its output object from the trailer down, not a comparison of object numbers |
| V6 | Every other page's reachable graph equals the input's as qpdf reads it, under the same mapping: unparsed dictionaries equal, and stream data equal, **raw where qpdf preserved the filters and decoded where it did not** |

**V6 is not "byte-identical to the input", and the claim must not be worded so.** qpdf renumbers every
object, and re-encodes streams whose filters it generalizes. Whether to set qpdf's decode level to
`none`, so that more of the bytes survive as written, is the slice's choice, and the wording follows
whatever it measures. **V6 has a cost**: a decoded comparison decodes every hostile stream of the
document twice. The slice costs it on the census, checks the deadline between objects (each decode is
still one uninterruptible call), and compares raw bytes wherever the filters were preserved.

**What it leaves undetectable, stated here because the variant's rustdoc must state it:**

- **Page identity across the two engines.** V1 to V6 are qpdf's view. That qpdf's page *N* is the
  page PDFium rendered as *N* is held by §5's checks, not by verification. The residue is two pages
  with identical frames that the two engines order differently: ADR 0022's #111 class, with
  `five-pages-or-six.pdf` as its example.
- **The region-to-pixel arithmetic, where the two sources agree wrongly.** PDFium's transform paints
  and burrow's arithmetic checks (§1 steps 4 and 6), so a wrong conversion on one side refuses at run
  time. Both read the frame the two engines agreed on (§5). If both are wrong in the same way, the
  wrong place is painted and passes. The rotation by box-origin by crop-offset matrix (*Fixtures and
  census*) is the gate on that.
- **The survivor set is same-source.** The walk that decides it also checks it. V5 checks that the
  output agrees with the report, not that the report was right. The independent reading is a
  test-time decoded scan (spike 0006's I2) over every census output. It gates S9 under rule 19.
- **Whether a survivor, or a document-level carrier, holds the region's content.** These are
  disclosures because no read-back can turn them into assertions.
- **Anything outside the painted rectangle** that the person meant to cover: rule 8.
- **That another viewer showed the original page the way PDFium did**, and content other readers draw
  on other pages that PDFium does not. The output's redacted page is PDFium's picture. That is the
  point, and it is also a limit.
- **Pixel identity across engine builds.** Goldens of the image's pixels are per engine build. The
  native–web differential compares structure, and compares pixels only where the census shows
  identity.
- **The heaps.** That is not a property of bytes. Canaries hold it (§7).

The DoD's fake-engine test applies. Fakes return an image with one region pixel unpainted, an image
one row short, a decoded length one byte over, an extra page key, a `/CropBox` unequal to the
`/MediaBox`, a second `/Resources` entry, a content stream one byte off, a dropped tree still present
by an untyped element, and a survivor the report left out. **The mutation that deletes each check
makes its test fail.**

### 5. Page identity

The page qpdf replaces must be the page PDFium rendered. If the two engines disagree about which page
is page *N*, qpdf replaces one page with a picture of another. The page that was meant to be redacted
is then written out intact, elsewhere in the document. V1 to V6 would pass.

**`page_frames` does not hold this today.** It reads frames through qpdf only (*Facts*, above). v1
therefore requires all of the following, and refuses otherwise:

- **PDFium's page count equals qpdf's**, `[page-identity-mismatch]`;
- **every page's frame and rotation, as PDFium reads it, equals qpdf's**, not only the rendered
  page's, `[page-identity-mismatch]`. PDFium reports 32-bit floats and qpdf 64-bit, so the comparison
  is **qpdf's value rounded to 32 bits equals PDFium's**, and the census counts every disagreement.
  On the web this is the first time PDFium vouches for a frame, so `[media-box-unverified]`'s web
  half can be lifted. That is an acceptance change, and it needs a census;
- **a page tree whose shape lets two readers disagree about which object is page *N*** refuses,
  `[page-tree-ambiguous]` (decision 57, item 1): any warning qpdf raises while enumerating pages;
  any page or node reachable by more than one path through the page tree; a node count the tree does
  not bear out; a node whose kind is not exactly one of page or intermediate node;
- **resolution happens at open, inside the trap** (§1), so the page tree every check reads is the
  one the write uses. `[engine-repaired-input]` stays, and is read again after the constructors. A
  repaired file is where the two engines' page trees part. `[reference-to-nothing]` and
  `[reference-unreadable]` stay with it, for the same reason.

**What the frame comparison adds is limited.** On a document whose pages all share one size and
rotation, which is most documents, comparing frames adds nothing beyond the count. There, identity
rests on the structural refusals above. That is said, not smoothed over.

The census renders each output's page *N* in PDFium and compares it with the painted bitmap: a
test-time check that the replaced page is the one drawn. **Open:** whether PDFium exposes an
object-number accessor, which would compare page *object* identity rather than frames.

### 6. Limits

`max_input_bytes` and `max_pages` stay exact, checked at open as today. Every other limit, with its
strength stated as `burrow_types::Limits`' rustdoc requires:

**`max_pixels` is exact, on the 150 dpi bitmap, before `FPDFBitmap_Create`, and again before the
redaction worker allocates.** Natively, redaction's own ceiling of **89,478,485 pixels** also applies:
the RGB bytes of a bigger image exceed the 256 MiB qpdf will decode, so verification could not read
it back (89,478,485 × 3 = 268,435,455). At 150 dpi a page of *w* × *h* points needs about 4.34·*w*·*h*
pixels:

| page | bitmap | pixels | RGB bytes | web (4,194,304) | native (89,478,485) |
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
twice the area of US Letter. Native flattens up to about 20.6 million square points (about 4,541 pt on
a side for a square), under its default `max_pixels` of 268,435,456. Of the 369 pages in the committed
fixtures' `page-frames.tsv`, one exceeds the web ceiling: the OCR scan. The 99-document census is the
slice's to run (rule 2), and it reports the pages each ceiling refuses (§2 rule 6).

**`max_duration_ms` is cooperative, and one deadline covers both workers** (§1). The render is
progressive (ADR 0027's 2026-09-17 amendment), checkpointed between pauses. **PDFium's page load,
qpdf's write (which encodes the image through zlib), and each stream decode in verification are each
one engine call that nothing can interrupt**, so overshoot of up to one such call remains, as
everywhere. ADR 0027 §2a measured one path-heavy page at 34 s, so the render, not the write, is where
a deadline will be spent. V6 checks the deadline between objects.

**`max_memory_bytes` is detected, not bounded.** A length-based estimate is made before the render:
PDFium's bitmap at 4 bytes per pixel, the RGB copy across the hand-off at 3, the encoder's output (up
to about 3), and verification's decoded copy at 3, so about 13 bytes per pixel: about 55 MB at the
web's ceiling, and about 1.16 GB at native's. A measured check follows after. **What PDFium's
rasteriser allocates for itself is bounded by nothing** (ADR 0027's *What is bounded*), as for every
render.

**The page load's posture is the render tool's, and no more.** It is guarded by
`estimate::before_page_load` before `FPDF_LoadPage` and by the measured memory check after. DECISIONS.md
rule 21's `MAX_MARKED_WORK` does **not** apply: it is computed by burrow's content walk, which v1 does
not run (*Facts*). So no count-based ceiling stands in front of this load, which rule 21 already
records for every render under its *Does not cover*. Round 0's security review asked that rule 21
cover both loads; it cannot without reading the content v1 does not read, and the residue is this
paragraph.

### 7. Heap posture (#199)

**The rule, unchanged in its words:** at the reply, no heap holds what the redaction removed. Under
flatten, that is the redacted page's decoded content, and the region's unpainted pixels (#136 D4:
rendered pixels are decoded content).

- **qpdf's heap.** In v1 qpdf never decodes the redacted page's content. The page is replaced before
  the write, and the old streams are unreachable and not written, unless they are survivors. It
  holds the input, which #199 already covers, and the painted bitmap, which carries nothing removed.
  ADR 0004's wipe-on-free stays.
- **The render worker** (decision 57, item 6). ADR 0004's hook cannot reach `pdfium.wasm`, which
  arrives already linked. The owner chose route (b): **zero both of the render worker's linear
  memories, `pdfium.wasm`'s and `burrow_wasm_render_bg.wasm`'s, then terminate the worker.**
  **Termination is the control**: a terminated worker's memories are discarded with it. Zeroing is
  defence in depth for the time between the hand-off and the termination. Building PDFium from source
  with the hook (route (a)) is not taken, and stays a condition for revisiting. The conditions on (b):
  - zeroing takes a **fresh view** of each memory after the last engine call, since a memory can grow,
    and **no engine call runs after it**;
  - each memory is asserted **not shared** (no `SharedArrayBuffer`), so no other agent holds a view;
  - the unpainted render's digest (§1 step 3) is computed **inside a linear memory**, with no
    `SubtleCrypto` call, `slice` or `postMessage` of unpainted pixels, so no JavaScript-owned copy
    exists to miss;
  - **no file-system copy** of a decoded stream is made in the module's virtual file system;
  - the bridge's own JavaScript copies are listed and each is zeroed or never made;
  - **the canary** plants a text and a pixel pattern in the region and searches **both linear
    memories and the JavaScript heap** at the reply, with a full scan. **Each wipe has its own
    mutation**, which skips it and turns the canary red.
- **The redaction worker's JavaScript.** The painted bitmap reaches it, and carries nothing removed.
  It does not reach the page before verification (D9, and §8).
- **The selection picture**, which is unpainted and is the person's own input, keeps #136 D1's
  posture: terminate on file change, before a run, on cancel and on `pagehide`.
- **Native.** PDFium's heap is the process heap, and it is not wiped. Native posture is M3/M4's, as
  ADR 0004's amendment already says of qpdf, and the native API's rustdoc says so.

### 8. Workers, modules, and R8 to R10

- **PDFium stays in the render worker; qpdf stays in the redaction worker.** No worker holds both
  engines.
- **What crosses the boundary, and nothing else.** From the redaction worker to the render worker: the
  input bytes, the page index, the region, the deadline that is left, and the selection record (§2 rule
  7). From the render worker back: PDFium's page count, every page's frame and rotation (§5), the
  painted RGB bitmap, its width, height and digest, and a refusal if there was one. R8's spec extends
  to the render worker and to each of these fields.
- **The hand-off does not go through the page's code as content.** The bitmap travels on a dedicated
  channel between the two workers, transferred rather than copied, and the page never holds it before
  verification.
- **R8** is unchanged: one reply carrying output, after verification, from the worker that wrote the
  document. **The painted bitmap is an input to the write, not output.**
- **R9's words are "nothing is written outside the wasm heap before verification passes"**, and the
  painted bitmap leaves a wasm heap before verification. The reading this record argues: R9 names the
  ways a page keeps a file (OPFS, the File System Access API, a `blob:` URL), and none of them is used.
  The bitmap moves from one worker's heap to the other's, carries nothing the redaction removes, and
  is never handed to the page. **The R9 spec extends to the render worker**: its stubs are armed there
  too, and its one transfer is the only exit. Round 1 is asked to confirm the reading. If it does not
  hold, R9's wording is amended by the owner, not read around.
- **R10** concerns the bytes verified and the bytes delivered. **On the web it is bound at run time by
  #206's digest once it merges.** Its final hop needs #206's web half, re-scoped to the runtime digest
  only, since the second reading it also carried is v2 by decision 55. This record does not say that
  R10 is met; the owner records that. The hand-off digest in §1 binds the *pixels* to the output. It
  does not discharge R10. A v1 twin of `redact_verify_same_bytes` holds R10 over the flatten write.
- **Bundles and budgets.** The paint, the digest and their refusal strings are redaction-only code in
  a bundle every tool loads (`burrow_wasm_render_bg.wasm`). DECISIONS.md rule 22 applies: under 0.5% of
  the page's total, or a separate bundle that only `/redact-pdf` loads. The slice measures which. The
  gates this touches are named in *Fixtures and census*.
- **The v2 rewriter stays callable behind a cargo feature, and is excluded from the shipped module**
  (decision 57, item 10). Its tests run with the feature on. Visitors do not download it.
- **New qpdf exports** (`qpdf_oh_new_stream`, `qpdf_oh_new_dictionary`, `qpdf_oh_new_name`, and the
  resolution route of §1) are listed in `EXPORTED_FUNCTIONS`, held by `check-wasm-exports`, argued in
  `qpdf-untrapped-accepted.toml`, and their cost to `qpdf.wasm`, which every tool loads, is measured.
- **The new `burrow-core` entry point** gets rustdoc and a fuzz target. **Its uniffi binding is
  deferred** to M3/M4, and that is said here rather than left out.

### 9. What the page may claim, and the copy

ADR 0029 §7's voice holds: every limit carries its reason, and the product is "Not Only PDF", never
"burrow". **What changes:**

- **The page may say that the marked area is covered in black in a picture of the page, and that
  every PDF reader decodes the same pixels.** #136's list said "it never says the area is blank" and
  "never says pictures or shapes are redacted". Under v1 both are true of the marked area, to the
  nearest pixel, and verification asserts it (V3).
- **It still never says** that the secret is gone, permanent, secure or compliant. It never says
  anything about what lies outside the marked area, that metadata is scrubbed, that other pages look
  the same in every reader, or that the rest of the document is unchanged byte for byte (V6).

The disclosures, as a draft. Lane S1's copy and `redact-copy.ts`'s test take their wording from this
list:

1. **The page becomes a picture.** Its text can no longer be selected, searched or read aloud. Its
   links are gone. The comments on it are no longer comments. The marks you could see are part of
   the picture, and a pop-up note that was shown open shows its text in the picture. Their text,
   author and replies are not kept as comments, and comments hidden on screen are gone.
2. **Only the area you marked is covered, to the nearest pixel.** A word, a line or a signature that
   crosses the edge stays visible outside it. Mark generously.
3. **The document's accessibility tags are removed**, on every page, not only this one.
4. **What sits outside the pages is not changed:** the title and keywords, the document's XMP
   metadata, attached files, bookmarks, named destinations, page labels, layer names, any JavaScript,
   and application data that can hold previews of pages. Check File → Properties, the bookmarks panel
   and the attachments panel. If one of them holds what you removed, it is still in this file.
5. *(Only when §2 rule 5's report is non-empty.)* **Parts of this page that other parts of the
   document also use stay in the file**, such as fonts and images (and drawings, if the census sends
   rule 5's drawings to disclosure), with whatever they contained.
6. **Form fields on other pages, and their values, are not changed. A digital signature in the
   document will no longer validate.**
7. **The file usually grows**, because the page is now an image.

### 10. ADR 0029, section by section

| 0029 | for v1 |
|---|---|
| *Context* | Kept as history. Its central measurement, that text survives in many places besides the glyphs, is why flatten drops the page's content wholesale |
| *The constraint that shapes every decision below* (the catalogue wall) | **Superseded as fact**: #239 made the trailer, and through it the catalogue, reachable. v1 writes to the catalogue only to remove `/StructTreeRoot` and `/MarkInfo` |
| §1 rewrite content streams and fonts | **v2** |
| §2 page-key allowlist | **Kept, narrowed** to §3's seven keys |
| §3 per-channel table | **Superseded for v1** by §2 here. Content-stream channels (rows 1–9, 18–22 of spike 0006) are handled by not carrying the content. The page's `/Thumb` and page metadata go by §3. Annotations are flattened or refused (rules 2, 3 and 9). Optional content hidden on the page is not drawn and not carried; layer names are disclosed (rule 4). Images and vector paths in the region are painted. **Every content refusal in the table is v2's**, because burrow does not read the content |
| §3 rows kept for v1 | The rows that keep qpdf's object graph and page tree the ones PDFium reads: `[engine-repaired-input]` (#224), `[reference-to-nothing]` and `[reference-unreadable]` (#227), the 2026-10-14 object-syntax rows (names outside the allowlist, vertical tab, a value qpdf drops on write), `[page-frame-unreadable]`, `[user-unit-not-one]`, `[rotate]`, `[rotate-not-whole]`, `[page-tree-depth]` and `[page-out-of-range]` (#224's frame rows). `[annotation-dependent-kept]` is reused and widened by rule 3, and covers #240's shared `/Annots` array |
| §3 rows changed for v1 | `[acroform-field]` narrowed (rule 2). `[structure-tree]` replaced by the drop (rule 1). `[media-box-unverified]`'s web half lifted by §5. `[page-attribute-inherited]` is not needed: `/CropBox` and `/Rotate` are written explicitly (§3), and inherited resources are survivors (rule 5). `[annotation-in-region]` (#136 D3) cannot arise as D3 framed it, since nothing in the region is silently kept; whether it is retired for v1 is the owner's (*Open questions*) |
| §4 `/Contents` per stream | **v2** |
| §5 refusal keyed on what the operation can see | **Kept as the rule.** It binds every rule here: each signal needs an evasion fixture it fires on |
| §6 what verification asserts | **Superseded for v1** by §4 here. The rule that "verified" is never worded as "the secret is gone" is kept |
| §6, the 2026-09-27 PDFium second opinion (#206) | **v2** |
| §7 copy | **Superseded for v1** by §9 here. Its voice is kept |
| §8 a removal nothing observed | **Kept.** V3 observes the paint. V5 observes that the page's own objects went, against an input set that had them |
| *Consequences*, *Conditions*, *Alternatives* | v2's, except where §*Conditions for revisiting* here repeats them |
| Amendments: inline-image extent; shared Form XObject; Type 3; `/ToUnicode`; widths; marked content; strokes; #164 to #166; #228, #229, #242, #278, #290; #344 | **v2** |
| Amendments: #224, #227, #239, #240, the 2026-10-14 object-syntax rows | **Kept** for the rows the *§3 rows kept* line names; their content-reading parts are v2's |
| Amendment: #175, the geometry walk reads the deadline | **v2**: v1 runs no walk. Its sentence on qpdf's decode ceiling still sets §6's native ceiling |
| Amendment: #176, `expect_after` judged against a real run | **Kept** as a rule for v1's fakes (§4) |
| Amendments: #191, one policy over a handle trait, and its two halves | **Kept** in shape: v1 runs native and web through one policy, and the native–web differential holds it. What the differential compares changes with §4 |
| Amendment: redaction's own wasm module | **Kept**: the qpdf half stays in its own module. §8 covers the render half |
| Amendment: the region's frame | **Kept**: display points, top-left |
| Amendment: the order of steps, and any failure discards the document | **Kept** |
| Amendment: #134, redaction is an operation and verifies | **Superseded** by §4 here: the variant changes |
| Amendment: S4, `flags` a default | **Kept, and extended**: the flatten render uses the selection picture's flags. **Its sentence that the selection picture "is still not a preview" is superseded for v1 by rule 7**: the selection picture is the render the output is made from |

### 11. DECISIONS.md, amended by the same decision

These changes are the owner's (decision 55), and **stand independently of this record's status**
(decision 57, item 9).

- **Rule 19:** its principle stands, that a same-source check may gate a merge and never the launch.
  Its S9 instance (the PDFium ink read-back, after #344) and rule 17's 2026-10-17 sentence ("the
  PDFium ink read-back still gates S9 via #206's ink half") **now apply to v2 only**. For v1, the
  same-source assertions are **the closed list in §4's *What it leaves undetectable***, kept current
  by this record's revisions. Today that is the survivor set, and the region-to-pixel arithmetic only
  where PDFium and burrow agree wrongly. Their independent readings gate S9.
- **#344 decision 4** (owner, 2026-10-15): #206's ink half asserting PDFium's tight character boxes
  against the region, as an S9 gate on the web path, **applies to v2 only**. v1 does not rely on
  `/FontBBox`, or on any glyph box, at all. ADR 0029 gets a short dated amendment saying the same,
  because its #344 amendment names that gate. The ROADMAP #344 row moves with the other content
  rows in the ROADMAP restructure, which is another lane's.

## Fixtures and census

Both round-0 reviews asked for fixtures and census items. They are merged here. **Every rule gets a
fixture it fires on, a near-miss twin it does not fire on, and an asserted mutation** (DECISIONS.md
rule 7): the mutation is shown to apply before the suite runs, and it turns the fixture's test red.
The slice builds from this list.

### Fixtures, by rule

| rule | fixture (fires) | near-miss twin (does not fire) | asserted mutation |
|---|---|---|---|
| §1 resolution at open | a file whose resolution repairs | the same file, well formed | skip the forced resolution: the repair surfaces after the constructors, and the re-check refuses; skip the re-check too: the test goes red |
| §1 step 6 `[paint-disagrees]` | a fake transform one pixel off | the true transform | delete the cross-check |
| §1 step 6 length check | a hand-off buffer one byte short, and one byte long | exact length | delete the checked multiplication |
| rule 1 `[structure-tree-kept]` | a tree element kept by another page's object; an **untyped** element kept by a document-level object; each tree entry in rule 1's list | the same trees with no kept referrer | key membership on `/Type`: the untyped fixture goes `Ok` and red |
| rule 1 drop | a tagged document claiming tags | an untagged one | keep `/MarkInfo` |
| rule 2 `[form-field-on-page]` | a widget with no field; a field kid with `/FT` only on an ancestor; a `/Fields` entry whose `/P` names the page; a signature field | a field whose widgets are all on other pages | drop each clause in turn |
| rule 2a `[xfa-form]` | `/XFA` as an array, as a stream, as a null; `/NeedsRendering` on the catalogue | neither key | key on the parsed value |
| rule 3 `[annotation-dependent-kept]` | an annotation listed on two pages; an appearance shared with a kept annotation; an action that names a page annotation; a shared `/Annots` array | the same shapes on two unrelated pages | narrow to `/IRT`, `/Parent`, `/Popup` |
| rule 4 `[metadata-thumbnail]` | catalogue XMP carrying a page thumbnail; an undecodable metadata stream | XMP without one | delete the check |
| rule 5 `[drawing-survives]` (or its disclosure) | an inherited form only this page draws; an off-tree page sharing this page's content; `/AcroForm` default resources holding the page's font | a form another kept page draws | let the walk enter pages: the report fills with every page's objects |
| rule 5 walk | a destination or `/P` back to another page | none | remove the stop at pages |
| rule 6 ceilings | a page at exactly 4,194,304 px and at one more; at exactly 89,478,485 px and at one more | the ceiling minus one | check after allocation |
| rule 7 binding | the same pixels from another file; another page with the same pixels; a different size | the recorded file, page and size | drop each field of the record |
| rule 8 edge | a hairline one pixel outside the region | ink two pixels outside | remove the extra pixel |
| rule 9 annotation flags | each of NoView, Print, Hidden, ToggleNoView; `/AP /N` as states with no `/AS`; an annotation under optional content; a `/Rect` outside the box; a Popup open and closed | an ordinary annotation | add `FPDF_PRINTING`, or a form-fill handle |
| §3 keys | an inherited `/CropBox` smaller than the media box | a declared one | omit `/CropBox` |
| §5 `[page-identity-mismatch]` | engines disagreeing on count; on one page's frame; on rotation | agreeing | compare the rendered page only |
| §5 `[page-tree-ambiguous]` | a page reachable by two paths; a count the tree does not bear out; a node of the wrong kind; a node of two kinds | a well-formed deep tree | delete each clause |
| §6 frames as floats | a frame value where 64-bit and 32-bit readings differ | an exactly representable one | compare without rounding |
| §4 V1–V6 lies | each fake listed in §4 | an honest fake | delete each check |
| §4 A7 transform matrix | every rotation (0, 90, 180, 270) by box origin (zero, offset, negative) by crop offset, checked against `FPDF_PageToDevice` | — | a sign flip in burrow's arithmetic |
| §7 heap canary | a dense page with planted text and pixels in the region | — | skip each wipe in turn; skip the termination |
| §8 R8 / R9 | an extra posted field; a stub called in the render worker | the listed fields | emit a chunk; call a stub |

**Carried over** from v2 and still applying, extended where v1 changes them: `page-frames.tsv`; the
fuzz targets `redact_page_frames`, `redact_references` and `redact_verified_output`; `secret_leak.rs`;
the R8 and R9 specs; the heap canary (with its new PDFium half); `test-redaction-refusals-are-reached.sh`;
the census harness. **New:** a v1 twin of `redact_verify_same_bytes`, and fuzz targets for the flatten
operation, for its write half, and for the survivor and structure walks. **v2 only:** `outcomes.tsv`
as it is (its goldens split into v1 and v2), `redaction_defences.rs`, the geometry and rewriter fuzz
targets, and the differential ledger.

**Gates the design touches**, each checked by the slice: `check-redaction-not-in-base` (the render
bundle gains paint, digest and refusal strings); `production-build.test.ts`'s held `redact` operation;
`check-pdfium-is-render-only`'s partition; the R8 and R9 specs, which today cover only the redaction
worker; rule 22's line for `burrow_wasm_render_bg.wasm`; `check-wasm-exports` and `EXPORTED_FUNCTIONS`.

### Census

The 99 real documents plus the fixtures, **every page flattened**, under both the web's and native's
ceilings, and run twice to measure determinism. The bar for every new refusal is **1% of real
documents** (DECISIONS.md rule 2), registered before the run. It reports:

- **every new refusal against the 1% bar**: `[form-field-on-page]`, `[xfa-form]`,
  `[annotation-dependent-kept]` (widened), `[structure-tree-kept]`, `[metadata-thumbnail]`,
  `[drawing-survives]`, `[page-tree-ambiguous]`, `[page-identity-mismatch]`, `[paint-disagrees]`,
  `[render-not-reproducible]`, and `[page-too-large-to-flatten]` per ceiling. Over the bar: the number
  goes to the owner, and the rule is not adjusted, except where an outcome below is already decided;
- **at least one real document moving from refused to `Ok`** for each acceptance: the tree drop (rule
  1), forms elsewhere (rule 2), the web's inherited media box (§5), the written `/CropBox` (§3), and
  the content refusals as a whole;
- the survivor distribution by kind, with the walk that stops at pages;
- PDFium's 32-bit frames against qpdf's 64-bit ones: every disagreement;
- how many files resolution at open repairs;
- render determinism three ways (progressive and one-shot, with pauses, through the native font
  mapper), and pixel identity across wasm, native x64 and native arm64;
- render time, write time and V6 time per page;
- a five-reader render of every output page (rule 17's twin render), and a decoded I2 scan of every
  output (§4);
- a PDFium render of each output's page *N* against the painted bitmap (§5);
- shared `/Annots` arrays and annotations on the real set.

**The census decides four outcomes, already worded:**

| | if the census shows | then |
|---|---|---|
| decision 3, `[metadata-thumbnail]` | it refuses **1% or fewer** of real documents | it refuses |
| | it refuses **more than 1%** | the XMP thumbnail is **stripped on output** instead, and the refusal goes |
| decision 4, `[drawing-survives]` | refusing costs **under 1%** of real documents | it refuses |
| | refusing costs **1% or more** | it is **disclosed**, and §9 item 5 names drawings |
| decision 7, the web ceiling | any number of pages refused at 4,194,304 px | the ceiling **stays**; the count is reported |
| decision 8, `/CropBox` = `/MediaBox` | any number of pages whose crop was inherited | it is **always written**, and each such page counts as an acceptance, with at least one real one shown |

## Consequences

**What becomes easier.**

- **The reader-divergence class has nothing to act on for the redacted page.** The 21 ship-blocker
  rows that concern how content is read become v2's (the ROADMAP restructure is another lane's).
- **Five-reader agreement on the redacted page is expected by construction**, since all five decode
  one lossless image. It is measured by DECISIONS.md rule 17's 2026-10-19 five-reader twin render,
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
- **New qpdf C entry points, and a resolution at open** (§1, §8). Each constructor needs an argued
  entry in `engines/qpdf-untrapped-accepted.toml`; `new_stream`'s argument rests on the resolution at
  open, and is re-checked after the constructors.
- **The web refuses Tabloid and A3** at 150 dpi, and native refuses above 89,478,485 pixels (§6).
- **Native heaps are not wiped** (§7).
- **Rule 17 tier: acceptance.** v1 accepts much of what ADR 0029 refuses: every content refusal, the
  tree, forms elsewhere, the web's inherited media box and an inherited crop. So the slice gets round 0
  and both reviewers. Its census shows at least one real file moving from refused to `Ok` for each
  acceptance, and measures every new refusal against rule 2's 1% bar (*Fixtures and census*).

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
- **PDFium built from source for wasm**, which would make route (a) in §7 available.

## Alternatives considered

**Keep ADR 0029's rewriter as v1.** This is v2. It is rejected for v1 on the rate of discovery in
*Context*, not on any single defect. Its strength stands: it keeps the page's text everywhere outside
the region.

**Flatten every page.** It closes no document-level carrier that rule 4 leaves open, and it costs every
page its text for a region on one. It is rejected as disproportionate.

**Copy the bitmap out of PDFium and paint it in Rust or JavaScript.** It is simpler to write. It is
rejected because the unpainted region would then exist in a second heap and cross a worker boundary,
and the owner's decision paints in PDFium's heap.

**Paint by burrow's arithmetic alone.** One source would then both paint and check. Decision 57 (item
5) paints by PDFium's transform and checks by burrow's.

**JPEG, or a lower resolution.** It is smaller. It is rejected because lossy encoding means V3 cannot
assert the paint exactly, and the record would be back to "probably blank".

**300 dpi.** Letter becomes 8.4 Mpx, refused on the web, at four times the bytes. It is rejected for
v1, and it is a condition for revisiting.

**A black rectangle over the original content.** This is concealment. Spike 0006 measured two readers
reading the text out from under it. It is rejected, as it was in ADR 0029.

**Scrub the document-level carriers.** Possible since #239. It is the owner's decision to disclose for
v1, and each scrub would be a decision of its own. The XMP thumbnail strip is the one scrub the census
can trigger (rule 4).

## Open questions, for round 1 and the owner

Decided by decision 57 since round 0: the web's `max_pixels` (item 7); rule 3's widening (item 1);
`/MarkInfo` (item 1); rule 5's drawings (item 4); §7's wipe route (item 6); whether the v2 rewriter
ships (item 10); and rule 9, kept (decision 56, item 3). Still open:

1. **D9 ("no after-picture of the result").** The output's pixels are exact (V3), so a picture of the
   result can no longer mislead the way ADR 0029's S4 amendment feared. Whether to show one is worth
   revisiting. It is not decided here. Until it is, the painted bitmap does not reach the page.
2. **Rule 7's reading.** This record reads it as one render for selection and output, bound by its
   record. The owner confirms the reading.
3. **R9's reading** for the painted bitmap (§8).
4. **V6's wording**, and qpdf's decode level.
5. **`[annotation-in-region]`** (#136 D3) for v1.
6. **§5's residue**, and whether page-object identity can be had from PDFium.
7. **The §3 rows kept for v1** (§10): confirm that the list is complete.
