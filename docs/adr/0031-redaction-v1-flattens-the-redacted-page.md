# 0031. Redaction v1 flattens the redacted page

Date: 2026-10-21

## Status

**Proposed**, 2026-10-21, from the owner's decision of that date (decision 55, item 1). Revised the
same day after round 0, under the owner's decision 57 (*Round 0*, below), and again on 2026-10-27
after round 1, under the owner's decisions 61, 64 and 66 (*Round 1*, below), and a second time that
day under decision 68, item 2 (*Round 1*, last paragraph). **Round 2 is the final
round** (DECISIONS.md rule 15; decision 64, item 1); it ran on `ac8f993`, and this record was revised
after it under the owner's decisions 69 and 70 (*Round 2*, below). This record becomes *Accepted* only
after the owner's reading of the record. No flatten code is written before this line changes.

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

### Round 1

Both reviewers ran round 1 on `3b99350` on 2026-10-26, reasoning against the code at `b8bf0ba` and
the vendored qpdf 12.4.1, without reproducers:

| reviewer | model | on | findings |
|---|---|---|---|
| security-reviewer | Opus | `3b99350` | no Critical; 4 High, 4 Medium, 6 Low; R9's reading confirmed, with four conditions |
| code-reviewer | Opus | `3b99350` | 1 Blocking, 10 Should-fix, 7 Nits |

The owner decided the points the reviews could not (decision 61: the survivor walk's start and stops,
which survivors refuse, and the every-object scan; decision 66: the page's orientation, and the page's
own content stream). This revision fixes every High and the Blocking finding in the text. Each
Medium, Low, Should-fix and Nit is either fixed in the text or, where only a measurement or slice
code can close it, listed under *Open for the build PRs* with the fixture or test it becomes
(decision 64, item 1). **After round 2, only a Critical or High finding (security-reviewer) or a
Blocking finding (code-reviewer) blocks the core build.** As in round 0, shapes the security review recorded privately are worded here by class.

**Decision 68, item 2** (owner, 2026-10-27) then confirmed the three points the revision had applied
as its own reading: tiling patterns refuse (§2 rule 5); objects reached only through page *N*'s
annotation appearances are in the refusing set (§2 rule 5); and the rewritten page's coordinates start
at zero, **with every destination into page *N* translated by the origin shift** (§3, *Destinations
into page N*; V7). The second revision of 2026-10-27 states the three as decided and adds the
translation, its fixtures and V7.

### Round 2

Both reviewers ran round 2, the final round, on `ac8f993` on 2026-10-27, reading the fix diff
`3b99350..ac8f993` against round 1's findings:

| reviewer | model | on | findings |
|---|---|---|---|
| security-reviewer | Opus | `ac8f993` | no Critical; 1 High, 2 Medium, 5 Low; every round-1 High closed |
| code-reviewer | Opus | `ac8f993` | 2 Blocking, 5 Should-fix, 6 Nits |

The High was that rule 5's split between survivors that refuse and survivors that are disclosed
rested on a survivor's *kind*, which the record did not define, and which a build would naturally
read from what the object declares. **Decision 69** (owner, 2026-10-27) decides it: a stream's kind is
the role the walk reached it through, never its declaration, and only three roles are disclosed (§2
rule 5). **Decision 70** (owner, 2026-10-27) confirms that, decides when a destination array counts
as a disclosed survivor (§2 rule 5), and decides both Blocking findings: an article bead's `/R` into
page *N* stays translated and is named in V6's exception and in V7, and V7 holds every destination
not into page *N* byte-equal to its input (§4). Disclosure 5's overclaim (a Medium) and the wording
findings are fixed in the text. The other Medium, and every finding that only slice code or a
measurement can close, are listed under *Open for the build PRs* with the fixture or test each
becomes. Per rule 15, this revision is checked by the code-reviewer
alone; there is no further round of both reviewers.

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

Added for round 1, rechecked on 2026-10-27 against the same tree:

| fact | where |
|---|---|
| burrow renders in **display orientation**: it passes `rotate = 0` to `FPDF_RenderPageBitmap_Start`, so PDFium applies the page's `/Rotate` itself. The selection picture depends on this: its frame is the display frame, "after `/Rotate`" | `core/burrow-engines/src/pdfium/ffi.rs:533`; `apps/web/src/components/region-picture.ts:9-13` |
| `page_frames` reports the **display frame**: the box's width and height, swapped for `/Rotate` 90 and 270 | `core/burrow-engines/src/pdfsyntax/region.rs:121-130` (`displayed_size`); `core/burrow-engines/src/redact/frames.rs:157-166` |
| `FPDF_PageToDevice` returns **integer** device coordinates, rounded, and takes the same start, size and rotate arguments as the render | `fpdfview.h:1072-1081` (the pinned PDFium headers) |
| A loaded page's width changes with its rotation. What the no-load `FPDF_GetPageSizeByIndexF` reports for a rotated page is not stated | `fpdfview.h:733`; `fpdfview.h:790-802`; the bridge reads the no-load size without `FPDF_LoadPage` (`bridge-pdfium.js:85-92`) |
| The bridge's one route out of PDFium's memory is `__burrow_pdfium_copy_out`, a `HEAPU8.slice`: a **JavaScript-owned copy**, wiped by `handOut` | `apps/web/src/worker/bridge-pdfium.js:240-247` |
| `sha2` is a dev- and build-dependency only. No shipped module computes SHA-256 today | `core/burrow-engines/Cargo.toml:47` (dev), `:59` (build); `core/burrow-ops/Cargo.toml:43` (dev) |
| qpdf treats a **null-valued key as absent**: `hasKey` is "not null", and iteration and unparse skip nulls | `QPDF_Dictionary.cc:64`, `:102`, `:142` (12.4.1) |
| `qpdf_oh_new_array` and `qpdf_oh_new_real_from_string` / `_from_double` are untrapped, like `new_stream`. `qpdf_oh_append_item` runs inside `do_with_oh_void`'s trap. None of the six constructors is in the wasm export list | `qpdf-c.cc:1495`, `:1502`, `:1544`, `:1610` (12.4.1); `engines/build-wasm.sh:287-305` |
| burrow's own code records `qpdf_oh_new_array` as **not meeting** the untrapped list's non-parsing bar | `core/burrow-engines/src/qpdf/ffi.rs:640-645` |
| qpdf's whole-file resolution runs **once**, behind the flag `fixed_dangling_refs`. The writer calls it again in `prepareFileForWrite`, where the flag makes it a no-op | `QPDF_objects.cc:1240-1249`; `QPDFWriter.cc:2286` (12.4.1) |
| #227's reference pass **already lexes every object in the file**: after each `obj` and `trailer` keyword, and inside each object stream qpdf would decode. Its rows (`[reference-to-nothing]`, `[reference-unreadable]`, `[name-read-differently]`, `[syntax-read-differently]`, `[null-value]`) are whole-file | `core/burrow-engines/src/redact/references.rs:47-58`; ADR 0029's 2026-10-14 amendments |
| The resource-graph walk's ceilings: depth 32, and 4,096 resource dictionaries | `core/burrow-engines/src/redact/sharing.rs:63`, `:81` |
| Of the 369 `PAGE` rows in `page-frames.tsv`, 27 are refusals. **342 pages are framed** | `tests/redaction/page-frames.tsv` |

## Decision

### 1. The pipeline: which engine does what, and in which heap

One region on one page per run, as ADR 0029 and #136 D2 already set. **One deadline covers the whole
run**, across both workers: the render worker is given what is left of it, not a fresh one.

| step | engine | heap (web / native) | what happens |
|---|---|---|---|
| 1. open | qpdf | redaction worker / caller's thread | Redaction's open: `Limits`, password, page count, the document-level refusals of §2 and §5, and the frames (`page_frames`). **The whole cross-reference table is resolved here, inside the trap** (*Resolution at open*, below). Any repair refuses, `[engine-repaired-input]` |
| 2. render | PDFium | render worker's `pdfium.wasm` / the engine thread (ADR 0011) | The page at **150 dpi**, with `FPDF_ANNOT` alone (the flags the selection picture uses, ADR 0029's S4 amendment; §2 rule 9), no form-fill handle, on opaque white. **It is rendered as displayed** (decision 66, item 1): `rotate = 0` to the render call, so PDFium applies the page's `/Rotate`, exactly as burrow renders today (`pdfium/ffi.rs:533`). The bitmap covers the **display frame**, *d*w × *d*h points (the effective box with width and height swapped for `/Rotate` 90 and 270), at *W* × *H* pixels (§2 rule 6). **The same render route as the selection picture** (§2 rule 7), whose frame is the display frame already (`region-picture.ts:9-13`). The written page carries `/Rotate 0` (§3), so the picture is not rotated a second time |
| 3. bind | PDFium side | the same | The unpainted bitmap is copied **from `pdfium.wasm`'s memory into a buffer `burrow_wasm_render_bg.wasm` allocated**, by one view-to-view `set` from a `HEAPU8.subarray` taken fresh after the last engine call: a view, never a `slice`. The bridge's existing `__burrow_pdfium_copy_out` slices (`bridge-pdfium.js:247`) and is **not** used for these pixels. In the Rust module's memory the pixels are converted to the canonical RGB form and digested (SHA-256, a new runtime dependency: §8), and the digest is compared with the selection picture's record (§2 rule 7). No JavaScript-owned copy of the unpainted pixels is made. The unpainted pixels therefore exist in **both** of the render worker's linear memories, and §7 zeroes both |
| 4. paint | PDFium | the same | The region arrives in display points with a top-left origin (ADR 0029's frame amendment). The render module converts it to user space with burrow's region conversion (`pdfsyntax/region.rs`), and **PDFium's own page-to-device transform** maps the corners to pixels (`FPDF_PageToDevice`, decision 57 item 5), with the same start, size and `rotate = 0` as the render. `FPDF_PageToDevice` returns rounded integers (*Facts*), so the corners are widened **by one pixel on each side to cover that rounding, and by one more for §2 rule 8**. Every pixel in that rectangle is filled with opaque black by `FPDFBitmap_FillRect`, **in PDFium's heap**. The painted bitmap is copied into the Rust module's memory by the same view-to-view route and converted to RGB there. Until the hand-off, no pixel of this render leaves the render worker's linear memories |
| 5. hand-off | | render worker → redaction worker | The painted RGB bitmap and its digest cross, with the fields §8 lists and nothing else, **transferred port to port** on a channel between the two workers (§8). It does not pass through the page's code. The redaction worker digests what it received and refuses on a mismatch. The render worker is then zeroed and terminated (§7) |
| 6. cross-check | burrow | redaction worker / caller's thread | Before allocating, the redaction worker **re-derives *W* = ⌈*d*w·150/72⌉ and *H* = ⌈*d*h·150/72⌉ from qpdf's display frame**, by checked arithmetic, and refuses unless the render worker's width and height equal them; it re-checks `max_pixels`, and that the buffer's length is exactly *W*·*H*·3. Then **burrow's own arithmetic** maps the region from display points to pixels at the displayed scale, *W*/*d*w across and *H*/*d*h down, from the top-left, **without converting to user space**. It widens outward to whole pixels and by rule 8's one pixel, and every pixel of **that** rectangle, burrow's own, must be the paint. Otherwise it refuses, `[paint-disagrees]`, and the census counts every such refusal. Two sources at run time: the paint goes through burrow's display-to-user conversion and PDFium's transform; the check goes through neither |
| 7. replace | qpdf | the same | **In place**: the page dictionary keeps its object identity, so every reference to the page (outline, destinations, other pages' links) still resolves to it. Every key outside §3's allowlist is removed. `/Rotate` becomes 0, and `/MediaBox` and `/CropBox` the display frame (§3). `/Resources` becomes a new dictionary naming one image XObject. `/Contents` becomes one new stream holding one line that draws it. The catalogue loses `/StructTreeRoot` and `/MarkInfo` (§2 rule 1). **Every destination into page *N* is translated by the origin shift** (§3, *Destinations into page N*; decision 68, item 2): the coordinates of those destination arrays, wherever they are held, and the `/R` of every article bead whose `/P` is page *N* (decision 70, item 2), and nothing else. burrow edits no other page's dictionary, except destination coordinates held inside it (a direct destination in another page's `/AA`, for example). **`[engine-repaired-input]` is checked again after the constructors**, then qpdf writes the document |
| 8. verify | qpdf, fresh | the same | §4, on the emitted bytes |
| 9. emit | | redaction worker | One reply, after verification: R8, R9 and R10 (§8) |

**burrow never reads the redacted page's content stream in v1.** No geometry walk, no lexer, no font
resolver runs over it. PDFium parses it in order to draw it, and that is the only reader it has. That
is why none of ADR 0029's content refusals applies to v1 (§10).

The image is 8-bit `/DeviceRGB`, `/FlateDecode`, with no `/DecodeParms`, `/Decode`, `/SMask`, `/Mask`,
`/Interpolate`, `/Intent` or `/OC`. That is the plainest shape every reader decodes alike. **qpdf
encodes it, through zlib, inside the write**: one engine call. The one line is fixed text, with only
the image's size in points, *d*w and *d*h, varying; its origin is always (0, 0), because §3 writes the
display frame with its origin there. The slice writes it down exactly, and verification compares it
byte for byte.

**Resolution at open.** Because `qpdf_oh_new_stream` can resolve the cross-reference table the first
time it runs (*Facts*), a resolution, and any repair it makes, must not first happen at step 7, outside
the trap, where a repair would change the page tree after every check had run on the old one. So the
open forces qpdf's whole-file resolution **inside the trap**, and asserts that it ran. qpdf's C API
offers no direct entry point for this, so the slice names the route: a trapped call that reaches it,
or a burrow shim in the qpdf build ADR 0004 already extends. **"Asserts that it ran" means reading
qpdf's resolved-once flag, `fixed_dangling_refs`** (*Facts*), set after the open's resolution. With
the flag set, neither `new_stream` nor the write's own call in `prepareFileForWrite` can resolve
again. A check that only observed that some call returned would not say that. After it, `new_stream`
finds the table resolved and parses nothing, which is the argument its entry in
`qpdf-untrapped-accepted.toml` rests on.

**The constructors step 7 needs**, each listed in §8: `qpdf_oh_new_stream`, `qpdf_oh_new_dictionary`,
`qpdf_oh_new_name`, and, because §3 writes `/MediaBox` and `/CropBox` with computed, usually
non-integer values, `qpdf_oh_new_array`, `qpdf_oh_new_real_from_string` (not `_from_double`, which writes a fixed
number of decimal places; *Open for the build PRs*), and
`qpdf_oh_append_item`. All but `append_item` are untrapped. `new_dictionary`, `new_name`, `new_array`
and the real constructor touch only the object cache, and are argued against the list's
non-parsing bar as `qpdf_oh_new_integer` is. **burrow's own code says `new_array` does not meet that
bar** (`qpdf/ffi.rs:640-645`). The slice either argues it does, and corrects that comment in the same
change, or writes the boxes by a route that needs no new array. The repair refusal is read again after
the constructors, so that the argument is checked, not assumed.

**The destination translation** (§3) needs no new entry point beyond those. It replaces coordinates
in the existing arrays in place with `qpdf_oh_set_array_item`, which runs inside `do_with_oh_void`'s
trap (`qpdf-c.cc:1592-1598`, 12.4.1), is already declared (`qpdf/ffi.rs:633-651`, for `/Widths`) and
already exported (`engines/build-wasm.sh:299`); the new values come from the real constructor above,
and a null from `qpdf_oh_new_null`, already accepted (`engines/qpdf-untrapped-accepted.toml:294-323`).
So whatever route the slice takes for the boxes, it needs a real constructor for the destinations.

### 2. The v1 rules

Each rule states what refuses, what is dropped and what is disclosed. Rule ids in brackets are
proposals for the slice. A refusal answers with its rule, as every redaction refusal does. Every
signal here is keyed on what the operation can see (ADR 0029 §5), and each has an evasion fixture
(*Fixtures and census*).

**A walk that stops at pages.** Rules 1, 3 and 5 each walk a set of objects. Each walk **stops at, and
does not enter, the members of the page tree and the catalogue**, whatever key leads there (an
annotation's `/P`, a destination, a structure element's `/Pg`). Otherwise a walk from one page
reaches every page.

- **Membership is by identity, never by what an object declares** (decision 61, item 1). The members
  are the objects, compared by object number and generation, that are either in qpdf's page list or
  reached from the catalogue's `/Pages` through `/Kids`, plus the catalogue the trailer's `/Root`
  names. §5's `[page-tree-ambiguous]` already refuses where those two readings of the tree differ.
  **Every other object is entered, whatever its `/Type` says.** An object that declares itself a page
  or a page-tree node and is not a member is walked like any other. An off-tree "page", such as a
  template, is walked through too. Both fail closed.
- **Each walk is bounded** (non-negotiable 3). It is iterative, not recursive, and it keeps a visited
  set, so it enters each object at most once and its work is at most the object count qpdf resolved
  at open. It checks the deadline between objects. A walk that passes its ceiling refuses,
  `[object-walk-too-large]`; the ceiling's value is measured on the census (*Open for the build
  PRs*). The same holds for V5's parallel walk, V6's mapping, and the destination enumeration of §3
  and V7.

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

- an `/Annots` entry on the page with a `/Subtype` of `Widget`, written as a name or as a string
  (PDFium accepts both), whether or not a field claims it;
- an `/Annots` entry that is a field or a field's kid: a `/FT` on itself or on any `/Parent`
  ancestor, or a place in the `/AcroForm /Fields` tree;
- an object in the `/Fields` tree whose `/P` names the page.

Signature fields count as fields. **Fields whose widgets are all on other pages are not refused**.
They, their values and the whole `/AcroForm` are carried unchanged, and that is disclosed (§9). This
narrows ADR 0029's `[acroform-field]`, which refuses every document with a form. **It is an
acceptance change**, and the census must show it on real files.

**Rule 2a. Refuse any XFA form,** `[xfa-form]` (decision 57, item 2). An `/XFA` key on the
`/AcroForm`, or a `/NeedsRendering` key on the catalogue, that **qpdf reports present, with any
non-null value of any type**, refuses. **Key presence, never the parsed value**: a reader that renders
XFA draws pages that PDFium does not, and carries data that no rule here reads. A key whose value is
null is not seen: qpdf treats it as absent (*Facts*), and so does the specification. Refusing it would
need a raw-byte reading, which this rule does not have; that is said rather than claimed.

**Rule 3. Refuse if anything the output keeps reaches an annotation of the redacted page,**
`[annotation-dependent-kept]` (the #239 rule id, kept). Round 0 found the narrower wording (another
page's annotation, through `/IRT`, `/Parent` or `/Popup`) was not what rule 5 relied on, and the owner
adopted the widening (decision 57, item 1):

- **An annotation survivor refuses.** Let *A* be the redacted page's annotations themselves and the
  annotation-owned objects reachable from them: their annotation dictionaries, their appearance
  dictionaries and appearance streams (every state, every `/AP` entry), their popups, their replies
  and their rich-text streams, by the walk that stops at pages. **The resources those appearance
  streams name are not in *A*.** They are in rule 5's set *S*, and rule 5 classifies them by the role
  the walk reached them through: an image or form XObject among them refuses there, and a font's
  dictionary and its embedded program are disclosed (decisions 61, item 2, and 69). Without that
  narrowing, an annotation whose appearance uses a font a kept page also uses would refuse here,
  through rule 3, against the owner's decision that fonts are disclosed. If any member of *A* is
  reachable from the output's trailer, by any path, the redaction refuses. That covers another
  page's annotation, an annotation another page's `/Annots` also lists, an `/Annots` array another
  page shares (#240), actions, the catalogue, names, the trailer and fields.
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
  property's presence, never its parsed value. **The property is identified by its namespace URI and
  local name, never by the prefix a packet binds**, since any prefix can be bound to that namespace.
  It is looked for in every character encoding an XMP packet may use. A metadata stream that cannot be
  decoded within `Limits`, or a packet that cannot be read as XMP, refuses too. The evasion fixtures
  are by class: a rebound prefix, each permitted encoding, and an unreadable packet (*Fixtures*).
  **Census-conditional:** the census reports how often it fires. **If it refuses more than 1% of real
  documents, the rule becomes a strip instead**: the XMP thumbnail is removed on
  output, and the refusal goes. That change gets its own review, as a scrub is a write.
- A signature anywhere in the document no longer validates after the write. That is disclosed (§9,
  item 6).

**Rule 5. Objects the redacted page used are refused or disclosed, by the role they were reached
through.** Precisely:

- **The set *S* starts from everything the page has, including everything it inherits** (decision
  61, item 1). Its roots are the page dictionary's own values, **and the value of every inheritable
  key (`/Resources`, `/MediaBox`, `/CropBox`, `/Rotate`) on every page-tree node above the page**,
  whether or not the page's own key overrides it. Taking every ancestor's value, rather than the one
  a reader would resolve, fails closed: a null or unreadable entry that PDFium and qpdf could resolve
  differently cannot hide a dictionary from *S*. From those roots, *S* is everything reachable by the
  walk that stops at pages. Without this, page *N*'s `/Parent` is a page-tree node, the walk stops
  there, and an inherited `/Resources` is never in *S*.
- After the replacement, the **survivors** are the members of *S* still reachable from the output's
  trailer: through another page, a page-tree node's inherited `/Resources`, the `/AcroForm` default
  resources, the names tree's appearances or templates, or anything else kept. Rules 1 and 3 already
  refuse the survivors that are tree elements or annotation-owned objects.
- **A stream's kind is the role the walk reached it through, never what it declares** (decision 69).
  This is H-2's identity principle (*A walk that stops at pages*; decision 61, item 1) applied to
  kinds. Page-tree membership is decided by object identity, never by `/Type`; a stream's kind is
  decided by the key path that led the walk to it, never by its `/Type`, `/Subtype`, `/PatternType`,
  `/ShadingType` or `/FunctionType`. A declaration is a value the file chooses, and readers read it
  differently: PDFium accepts a byte string where a name is expected (#125), so a name-typed reading
  of `/Subtype` would class a drawn image as something else. A key path is what the walk itself
  followed. **The walk records, for each stream, every role it reached that stream through**, on every
  arrival, including an arrival at an object the visited set has already entered, so that the visited
  set cannot hide a second role.
- **Disclosed: a stream reached solely in one of three roles** (decision 69). Each role is named by the
  last steps of its key path. Here a *resource dictionary* is any dictionary the walk reached as the
  value of a `/Resources` key, and a *font* is a value in a resource dictionary's `/Font` dictionary.
  1. **An embedded font program**: the value of `/FontFile`, `/FontFile2` or `/FontFile3` in a font
     descriptor, where the font descriptor is the `/FontDescriptor` of a font, or of an element of a
     font's `/DescendantFonts` (ISO 32000-2, 9.8 and 9.9).
  2. **A CMap stream**: a font's `/ToUnicode` (9.10.3); a font's `/Encoding`, where that value is a
     stream (an embedded CMap, 9.7.5.3); and the `/UseCMap` of a stream already in this role.
  3. **An ICC profile**: element 1 of a two-element array whose element 0 is the name `/ICCBased`
     (8.6.5.5), where the array is in a *colour-space slot*. The slots are: a value in a resource
     dictionary's `/ColorSpace`; the `/ColorSpace` of a value in a resource dictionary's `/XObject` or
     `/Shading`, or of the `/Shading` of a value in its `/Pattern`; the `/CS` of page *N*'s `/Group`, or
     of the `/Group` of a value in a resource dictionary's `/XObject`; and, for a colour space that is
     itself in a slot, the base of `/Indexed` (element 1), the alternate of `/Separation` or `/DeviceN`
     (element 2), the underlying space of `/Pattern` (element 1), and an ICC profile's `/Alternate`.
     Also **the `/DestOutputProfile` of an output intent in page *N*'s own `/OutputIntents`** (a page
     key since PDF 2.0; 14.11.5). The catalogue's `/OutputIntents` is not reached: the walk stops at
     the catalogue. A family name element 0 does not hold as a name, such as `(ICCBased)`, is not
     this role, so that stream refuses: the reading fails closed.

  **The same disclosed kind reached through several paths stays disclosed** (decision 71, item 1):
  streams reached by several paths that all end in roles of one of these three kinds are in that
  role. For example: one font program that two font descriptors name, or one ICC profile two colour
  spaces use; a CMap that is one font's `/ToUnicode` and another CMap's `/UseCMap`; an ICC profile
  in a `/ColorSpace` resource and in an output intent's `/DestOutputProfile`.
- **Refused, `[drawing-survives]`: every other stream the walk reached from page *N* that survives.**
  Every stream reached in a role outside the three above, whatever it declares, and **every stream
  reached in more than one role**, which is what decision 69's "reached more than one way" means
  (decision 71, item 1): in roles of two of the three kinds, or in one of them and in any other
  role. That includes:
  - **page *N*'s own content stream**, or any stream of its `/Contents` (decision 66, item 2), and
    page *N*'s `/Thumb`;
  - **every value of a resource dictionary's `/XObject`, image or form** (decision 61, item 2),
    **including those reached only through page *N*'s annotations' appearance resources** (decision
    68, item 2), and an image's `/SMask` and `/Mask`. Decision 61 names those reachable from page
    *N*'s original resources; decision 68 confirmed the wider set, which fails closed;
  - **every stream value of a resource dictionary's `/Pattern`**, which is a tiling pattern's content
    (decision 68, item 2), and every shading stream and function stream;
  - **a graphics state's soft mask** (`/SMask` and its `/G`), and **an appearance stream** (rule 3
    refuses it first);
  - **a Type 3 font's glyph procedures**, the values of its `/CharProcs`;
  - streams beside a font program that are not one: a font descriptor's `/CIDSet`, a CIDFont's
    `/CIDToGIDMap` where it is a stream, and the `/Metadata` of any stream above. **`/CIDSet` and a
    stream `/CIDToGIDMap` refuse if the census measures their cost under 1% of real documents;
    otherwise they join the disclosed allowlist, by name, and the figure is reported to the owner**
    (decision 71, item 2). The `/Metadata` of a stream refuses either way;
  - **roles that could be read either way, made refusals rather than guessed** (decision 69;
    confirmed by decision 71, item 2): a font program under the per-class font descriptors of a font descriptor's
    `/FD`; a `/ToUnicode` or a stream `/Encoding` on an element of `/DescendantFonts` rather than on
    the font the resource dictionary names; and an ICC profile reached through a `/DeviceN` space's
    `/Attributes` (its `/Colorants` and `/Process` spaces), or through any colour space not in a slot
    listed above;
  - **whether or not a kept page draws it.** v1 reads no page's content, so "drawn by a kept page" is
    not something it can compute. Reachability and role are the predicates, and both are computable.
- **Disclosed: every survivor that is not a stream**, with **fonts** named (decision 61, item 2, an
  accepted residual): font dictionaries and descriptors, colour-space arrays, graphics states, shading
  dictionaries (confirmed by decision 71, item 2) and marked-content property lists, which can carry text of their own. A subset font with
  a `/ToUnicode` map reveals which characters the page used.
- **A destination array counts toward disclosure 5 only when one of its items is something other
  than a page reference, a name, a number or null** (decision 70, item 1). A *destination array* is an
  array the walk reached solely as a destination: a link annotation's `/Dest`, or the `/D` of a `/GoTo`
  action; an array also reached in any other role is not one. A *page reference* is an indirect
  reference to a page object, a leaf of the page tree, by identity (§2).
  So one destination array that page *N*'s link and the outline share is a survivor, reported and
  checked by V5, and it does not make disclosure 5 appear; a twin holding a string or a dictionary
  item does. **Whether any other non-stream survivor counts is unchanged**: every one counts.
- **Census** (decisions 61, item 2, and 69): the census reports what `[drawing-survives]` costs, by
  producer and by role, **including what the wider refusal of decision 69 costs**: the documents that
  refuse only because of a role the earlier text disclosed (Type 3 glyph procedures, shading and
  function streams, soft masks, `/CIDSet` and `/CIDToGIDMap` streams, the ambiguous roles above, and
  streams reached in more than one role). It measures **both readings of "reached more than one
  way"** (decision 71, item 1): in more than one role, which is the rule, and through more than one
  path at all, the literal reading. It reports **`/CIDSet` and stream `/CIDToGIDMap` as their own
  figure**, which decides whether they refuse or join the allowlist (decision 71, item 2). **If `[drawing-survives]` refuses more than 1% of real
  documents, the work stops** and the owner is brought the figure, the producers and the roles. The
  rule is not adjusted to fit. Decision 57, item 4's census condition, which disclosed drawings above
  1%, is superseded.
- The operation **reports the survivors**: a count by role (font programs, CMaps, ICC profiles,
  survivors that are not streams, and destination arrays that do not count).
- Verification asserts that the survivors in the output are exactly the set reported, that each
  disclosed stream is in one of the three roles and no other, and that every count is the reported
  one (§4, V5).
- **The disclosure is conditional on that report counting a survivor that counts.** A page whose
  objects nothing else keeps, or whose only survivors are destination arrays that do not count, does
  not carry a caveat about them.

**Rule 6. 150 dpi, lossless Flate, and refuse above the pixel ceiling,** `[page-too-large-to-flatten]`.
The bitmap is *W* × *H* = ⌈*d*w·150/72⌉ × ⌈*d*h·150/72⌉ pixels over the page's **display frame**,
*d*w × *d*h points: the effective box, with width and height swapped for `/Rotate` 90 and 270. *W* and
*H* come from qpdf's 64-bit frame, and the redaction worker re-derives them (§1 step 6). A frame
whose 32-bit reading would round to a different *W* or *H* refuses there, or at §5's comparison. It
is checked against the
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
- **Selection and run use one route**: one orientation, **the display orientation** (`rotate = 0`,
  PDFium applying `/Rotate`; decision 66, item 1), the same progressive or one-shot call, the same
  flags, the same fill. PDFium is expected to be deterministic for one build, one size and one
  set of flags. **That is a claim until the census measures it** (*Fixtures and census*), across
  progressive and one-shot rendering, pauses, the native font mapper and the bitmap fill.
- **What the comparison defends against, and what it does not.** It runs inside the render worker,
  which is parsing the hostile file. It catches a render that is not reproducible. It is **not** a
  defence against a PDFium the file has compromised, which could report any digest. Against that, the
  control is the redaction worker's own check of the paint (§1 step 6, V3), made in another worker
  with another engine.
- Rule 7 holds **within one build**. Whether wasm, native x64 and native arm64 PDFium produce the same
  pixels is unmeasured, and it matters only to goldens and the native–web differential (§4).

If the owner meant the picture *after* the run instead, that is ADR 0029's D9 question (*Open
questions*, item 1).

**Rule 8. The region's edge changes meaning, and the copy says so.** Under ADR 0029, a glyph whose
conservative box meets the region is removed whole, and ink crossing the edge refuses. **Under v1 the
edge is a pixel edge.** Every pixel the region touches is painted, and **one more pixel on each side**,
because hinting and hairlines can put ink up to about a pixel beyond where the arithmetic places it.
The paint side adds a further pixel for `FPDF_PageToDevice`'s rounding (§1 step 4); the check holds
burrow's own rectangle, with rule 8's pixel, to be entirely paint (§1 step 6), so the extra pixel has
an independent check.
A glyph, a line of text, an image or a signature crossing the edge **stays visible outside it**, and
part of a word can stay legible. No content refusal stands in for this. The page tells the person to
mark generously (§9, item 2), and the operation's rustdoc states the pixel-edge semantics.

What lies outside the painted rectangle is **not detectable at run time**: no check can say whether
something there is what the person meant to cover.

**Rule 9. Annotations on the redacted page: what reaches the picture, and what the person is told.**
Proposed per decision 56, item 3, which asked for the rule and its question to be added to the
round-0 package. Round 1 did not dispute it; it is for round 2 and the owner's reading of the record.

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

- **`/Rotate` is written as 0**, explicitly (decision 66, item 1), so that no `/Rotate` inherited from
  the page tree can reach the replaced page. The picture is already in display orientation (§1 step
  2), and a page that kept its rotation would be rotated twice.
- `/MediaBox` is **the display frame PDFium rendered**: `[0 0 dw dh]`, where *d*w × *d*h is the
  page's effective box (the crop clipped to the media box) with width and height swapped for
  `/Rotate` 90 and 270. It is written explicitly on the page.
- **`/CropBox` is always written, equal to `/MediaBox`** (decision 57, item 8). A crop inherited from
  the page tree would otherwise reach the replaced page and cut the picture. Writing it removes that
  case, so `[page-attribute-inherited]`'s `/CropBox` half is not needed for v1. **That is an
  acceptance**, and the census counts it as one.
- **The page's user space changes with it** (decision 68, item 2): its coordinates start at zero, in
  display orientation. A destination into this page still reaches it, because the page keeps its
  object identity (§1 step 7). **The position a destination names inside the page is translated**
  (*Destinations into page N*, below), so that it lands on the same place in the picture.


This is ADR 0029 §2's allowlist, narrowed. Every other key goes. That includes `/Annots`, `/B`, `/AA`,
`/Thumb`, `/StructParents`, `/PieceInfo`, `/Metadata`, `/Group`, the other boxes, `/Tabs`, `/VP`,
`/AF`, `/UserUnit`, and every key nobody listed. The frame check (§4, V1) holds the display frame
equal to the input's: the output's `/MediaBox` under `/Rotate 0` against the input's box **with width
and height swapped for 90 and 270**, which is what `page_frames`' display frame already is (*Facts*).

#### Destinations into page N

**The map.** Page *N*'s input frame is read through qpdf, as `page_frames` reads it: the display box
is the `/CropBox` clipped to the `/MediaBox`, or the `/MediaBox` where there is no crop
(`redact/frame.rs:63-81`), with lower-left corner (*x*0, *y*0), unrotated width *w* and height *h*, and
the page's effective `/Rotate` *r*. `/UserUnit` is 1, since any other value refuses
`[user-unit-not-one]` (`redact/frame.rs:113-123`; kept for v1, §10), so the map has no scale. The
**origin shift** is the display box's lower-left corner, which is the `/MediaBox`'s only where the page
has no `/CropBox` or its crop covers the media box. For a point (*x*, *y*) of the input page's user
space, let *u* = *x* − *x*0 and *v* = *y* − *y*0. Its place (*X*, *Y*) on the written page, under
`/Rotate 0` and `/MediaBox [0 0 dw dh]` (B1), is the inverse of the region conversion's four cases
(`pdfsyntax/region.rs:203-219`):

| *r* | *X* | *Y* | written frame |
|---|---|---|---|
| 0 | *u* | *v* | *w* × *h* |
| 90 | *v* | *w* − *u* | *h* × *w* |
| 180 | *w* − *u* | *h* − *v* | *w* × *h* |
| 270 | *h* − *v* | *u* | *h* × *w* |

**For 90 and 270 the axes swap**: the written page's horizontal coordinate comes from the input's
vertical one, and the vertical from the horizontal.

**Each explicit destination form** (ISO 32000-2, 12.3.2.2), with the page element unchanged:

| form | written |
|---|---|
| `[page /XYZ left top zoom]` | (*left*, *top*) mapped as a point. For 0 and 180, the new *left* comes from the old *left* and the new *top* from the old *top*. **For 90 and 270, the new *left* comes from the old *top*, and the new *top* from the old *left*.** A null stays null **in the slot its axis moved to**: a null old *top* on a quarter-turned page is a null new *left*. *zoom* is unchanged |
| `[page /FitR left bottom right top]` | both corners mapped as points, then ordered: the new *left* and *right* are the smaller and larger *X*, the new *bottom* and *top* the smaller and larger *Y* |
| `[page /FitH top]`, `[page /FitBH top]` | for 0, *top* − *y*0; for 180, *h* − (*top* − *y*0). **For 90 and 270, null**, below |
| `[page /FitV left]`, `[page /FitBV left]` | for 0, *left* − *x*0; for 180, *w* − (*left* − *x*0). **For 90 and 270, null**, below |
| `[page /Fit]`, `[page /FitB]` | unchanged: no coordinate |

**`/FitH` and `/FitV` on a quarter-turned page are written with a null coordinate.** The format
defines the coordinate on a user-space axis, which on a 90 or 270 page is the displayed horizontal
for `/FitH` and the displayed vertical for `/FitV`. Readers already differ on what the input means
there: whether "the width" is the page's or the display's, and whether the coordinate is a scroll
position across or down. No single `/Rotate 0` form reproduces every reading, so the writer keeps the
form, which still reaches and fits the page, and writes the coordinate as null, the format's own
"leave unchanged". The only thing lost is the scroll position, and only on those pages.

**Which destinations are into page *N*: by identity**, as §2's walk decides membership (H-2, decision
61 item 1). A destination is into page *N* when its first element is an indirect reference whose
object number and generation are page *N*'s in qpdf's page list. Never by what the referenced object
declares. A first element that is an integer, a page index that the format allows only in remote
destinations, is not into page *N* and is left unchanged.

**Where they are looked for**, in the output's graph, every site the format defines for a
destination into this document:

- the `/Dest` of every link annotation, and the `/A` and `/AA` actions of every annotation, on every
  page but *N* (page *N*'s go with `/Annots`, §3's allowlist);
- every page's `/AA` but *N*'s; every form field's `/AA`; the catalogue's `/AA`;
- every outline item's `/Dest` and `/A`, walked through `/First` and `/Next`;
- the catalogue's `/OpenAction`, as an array or as an action;
- in every action those sites hold, and in every action reached through `/Next` (a dictionary or an
  array of them), the `/D` of each `/S /GoTo`;
- **every value of the catalogue's `/Dests` dictionary, and every value in the `/Names` `/Dests`
  name tree**, as an array or as a dictionary whose `/D` is one;
- every article bead (`/Threads`) whose `/P` is page *N*: its `/R` is a rectangle in the page's user
  space and is mapped as `/FitR`'s is.

Each enumeration is a walk of §2's kind: iterative, with a visited set, the deadline checked between
objects, and `[object-walk-too-large]` past the ceiling. A destination array that is one indirect
object reached from several sites is **translated once**, by object identity; a direct array is its
own value, translated where it is held. `/GoToR`, `/GoToE` and `/Launch` name other files and are not
edited. A `/GoTo` that names a structure element by `/SD` keeps that element reachable, and rule 1
refuses it, `[structure-tree-kept]`.

**Named destinations resolve at their definitions, not at their referrers.** A `/Dest` or `/D` that
is a name or a string is never edited. Every value in both dictionaries that is into page *N* is
translated, the name tree **walked in full through `/Kids` and `/Names`, never searched by
`/Limits`**. So whichever entry a reader's lookup lands on, including a name defined twice or defined
in both dictionaries, it is a translated one wherever it is into page *N*.

**What the writer cannot translate, and what it does:**

| case | written | why |
|---|---|---|
| a null coordinate | **null**, in the slot its axis maps to | null is the format's "unchanged", and stays meaningful after the map |
| a destination into page *N* that is malformed: a form name not in the table, the wrong number of operands, an operand that is neither a number nor a permitted null, a non-finite number | **unchanged**, counted | readers already disagree on how to read it, so no translation is the right one; see below |
| a name or string that resolves to nothing, or a name-tree node that is not readable as one (a non-dictionary kid, a `/Names` that is not an array) | **unchanged**: a referrer is never edited, and entries inside a node burrow cannot read are not translated | every entry burrow can read is translated; a lenient reader that reads the others lands at their old position, which is navigation, not content (below) |
| an integer page element | **unchanged** | not into page *N* by identity |

**Keep-unchanged, not refusal, and why a wrong destination is not a leak.** A destination names a
page and a position, and a zoom. After v1, every position on page *N* shows the painted picture and
nothing else: the page itself holds only the picture and its one line (§3's keys, held by V2), and
nothing it drew survives elsewhere (rule 5, V5), so a destination at the wrong place
cannot make any reader show it. Its numbers were the input's, and they are carried as the rest of the
outline and the names are (rule 4, disclosure 4). A wrong one misplaces the view on a page whose
every pixel is already the output; it reveals nothing. Refusing would cost whole documents for a
navigation defect, where non-negotiable 4's fail-closed argument is about leaks. So it is kept, and
it is measured: the census counts every destination into page *N*, by site and form, translated and
kept unchanged (*Census*), and V7 holds each kept one to its input.

**What the translation does not detect:** a destination at a site the list above does not name keeps
its input position. V7 reads by the same list, so it cannot see one; that is same-source (§4), and
navigation only. The census's independent net is a scan of every output for any array whose first
element is page *N*'s reference, wherever it is held, reporting each one V7 did not hold.

### 4. Verification (ADR 0022)

**`Expected::PageFlattened`**, a proposed name. Not `Redacted`, for the reason ADR 0029's #134
amendment gives against `RegionCleared`'s alternatives. It asserts the following, every item
re-derived from the emitted bytes through a fresh qpdf:

| | asserts |
|---|---|
| V1 | `verify::output`'s half: the input's page count, and the expected **effective** rotation vector as written: the input's, with page *N*'s entry 0, because §3 writes `/Rotate 0` (`verify::Expected` takes the vector the operation computes, `core/burrow-ops/src/verify.rs:85-102`). Also, every page's display frame read from the output (`page_frames`) equals the input's: page *N*'s output frame, `/MediaBox` under `/Rotate 0`, against the input's box with width and height swapped for 90 and 270 |
| V2 | The flattened page's dictionary has exactly §3's seven keys, with `/CropBox` equal to `/MediaBox`. `/Resources` has exactly one entry, `/XObject`, naming exactly one image. `/Contents` is one stream whose decoded bytes equal the line burrow wrote, **byte for byte** |
| V3 | The image dictionary has exactly the keys §1 lists, and its `/Width` and `/Height` are the *W* and *H* the redaction worker derived. qpdf learns a decoded length only by decoding, so **the read is capped at *W*·*H*·3 + 1 bytes, and refused unless it is exactly *W*·*H*·3**. **Its decoded bytes have the digest bound to the painted bitmap** (§1 step 5), and **every pixel in the painted rectangle is the paint**, compared exactly. The painted rectangle is **the one the redaction worker computed** (§1 step 6), never one the render worker reported |
| V4 | The catalogue has no `/StructTreeRoot` and no `/MarkInfo`, and **no member of rule 1's element set** is in the output, by V5's parallel walk |
| V5 | The survivors (§2 rule 5) present in the output are **exactly** the set the operation reported, with the role the walk recorded for each. **No surviving stream is outside the three disclosed roles, or in more than one role** (decision 71, item 1), by the roles the parallel walk records itself, never by what a stream declares; each destination array counted as not counting has only page references, names, numbers and nulls as items. Nothing reachable only from the input page is present. qpdf renumbers, so this is a parallel walk that maps each input object to its output object from the trailer down, not a comparison of object numbers. It is bounded as every walk is (§2) |
| V6 | Every other page's reachable graph equals the input's as qpdf reads it, under the same mapping: unparsed dictionaries equal, and stream data equal, **raw where qpdf preserved the filters and decoded where it did not**. **The one exception is the coordinates of destinations into page *N*, and the `/R` of each article bead whose `/P` is page *N*** (decision 70, item 2), which V7 holds instead. The exception is those numbers only: every other key and item of those arrays and beads is held by V6 |
| V7 | **Every destination into page *N*** (§3, *Destinations into page N*), enumerated from the output by §3's sites and identity rule and paired with the input's by V5's mapping, **equals the input's destination under §3's map**: the same form and page element, each coordinate equal to the mapped value as qpdf reads it back, exactly, null where §3 says null, and **unchanged where §3 keeps it unchanged**. Which input destinations are translated and which are kept is decided from the input, so a writer that skips one fails. The output has neither more nor fewer such destinations than the input has at the same sites, and their count, by site, form and outcome, is the one the operation reported. **Every article bead whose `/P` is page *N*** has its `/R` equal to the input's mapped as `/FitR`'s corners are (decision 70, item 2), and **every other article bead reached from the catalogue's `/Threads` is byte-equal to its input**, as qpdf unparses it, under V5's mapping, including a bead on a kept page that has no `/B`, which no page's graph reaches and V6 therefore does not hold. **Every destination at §3's sites that is not into page *N* is byte-equal to its input**, as qpdf unparses it, under V5's mapping (decision 70, item 3): V6 covers only the page graphs, so without this a translated destination into another page at a catalogue site (the outline, `/Dests`, the `/Names` tree, `/OpenAction`, the catalogue's `/AA`) would pass. Bounded as every walk is (§2) |

**V6 is not "byte-identical to the input", and the claim must not be worded so.** qpdf renumbers every
object, and re-encodes streams whose filters it generalizes. Whether to set qpdf's decode level to
`none`, so that more of the bytes survive as written, is the slice's choice, and the wording follows
whatever it measures. **V6 has a cost**: a decoded comparison decodes every hostile stream of the
document twice. The slice costs it on the census, checks the deadline between objects (each decode is
still one uninterruptible call), and compares raw bytes wherever the filters were preserved.

**What it leaves undetectable, stated here because the variant's rustdoc must state it:**

- **Page identity across the two engines.** V1 to V7 are qpdf's view. That qpdf's page *N* is the
  page PDFium rendered as *N* is held by §5's checks, not by verification. The residue is two pages
  with identical frames that the two engines order differently: ADR 0022's #111 class, with
  `five-pages-or-six.pdf` as its example. It is same-source, because §5's structural refusals read
  qpdf's tree. Its independent reading is the census render of each output's page *N* against the
  painted bitmap (§5). It gates S9 under rule 19.
- **The region-to-pixel arithmetic, where the two sources agree wrongly.** The paint goes through
  burrow's display-to-user conversion and PDFium's transform; the check maps display points to pixels
  by the displayed scale alone (§1 steps 4 and 6). They share no step but their input: the display
  frame, and *W* and *H* derived from it. So a wrong conversion on either side refuses at run time,
  and they can agree wrongly only where that shared frame is wrong, which §5's comparison of the two
  engines' frames holds. The rotation by box-origin by crop-offset matrix (*Fixtures and census*)
  tests the paint side against the check across every case.
- **The survivor set is same-source.** The walk that decides it also checks it. V5 checks that the
  output agrees with the report, not that the report was right. The independent reading is a
  test-time decoded scan (spike 0006's I2) over every census output. It gates S9 under rule 19.
- **The engines read page *N*'s content differently** (decision 61, item 3). The set *S*, rules 1, 3
  and 5 and V5 all read qpdf's object graph. The page PDFium drew is PDFium's reading. If PDFium
  resolves page *N*'s `/Contents` or `/Resources` to an object other than the one qpdf resolves, the
  stream PDFium drew is not in qpdf's *S*, and a kept referrer can carry it out, unreported, with V1
  to V7 passing. The every-object scan (§5) refuses the syntactic routes to that disagreement that
  have been measured. It cannot prove there is no other. Rendering qpdf's own rewrite of the input,
  instead of the original bytes, would remove some of those routes; that is recorded as a future
  option (*Conditions for revisiting*), not taken for v1. It is same-source under rule 19, and its
  independent reading is the census's decoded I2 scan of every output.
- **A destination into page *N* at a site §3 does not list** keeps its input position. V7 reads by the same list,
  so this is same-source; it is navigation, not content (§3). Its independent reading is the census's
  scan of every output for any array whose first element is page *N*'s reference.
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
`/MediaBox`, a `/Rotate` other than 0, a `/MediaBox` with width and height unswapped on a quarter-turn
page, a second `/Resources` entry, a content stream one byte off, a dropped tree still present by an
untyped element, a survivor the report left out, a stream outside the disclosed roles among the
survivors, and, for V7, an
`/XYZ` into page *N* left untranslated, a shared destination array translated twice, a `/FitR` with
its corners unordered, a quarter-turned page's `/XYZ` with its axes unswapped, a null written as 0, a
malformed destination "corrected", an article bead's `/R` into page *N* left untranslated, and a
destination into another page translated, placed once in the outline and once in the `/Names` `/Dests`
tree (decision 70, item 3). For V5: a stream disclosed whose recorded role is outside the three, and
one reached in two kinds of role. **The mutation that deletes each check makes its test fail**, and
for the misplaced translation, so does the mutation that widens the writer's identity rule to any
object declaring `/Type /Page`: V7 must refuse what that writer emits.

### 5. Page identity

The page qpdf replaces must be the page PDFium rendered. If the two engines disagree about which page
is page *N*, qpdf replaces one page with a picture of another. The page that was meant to be redacted
is then written out intact, elsewhere in the document. V1 to V7 would pass.

**`page_frames` does not hold this today.** It reads frames through qpdf only (*Facts*, above). v1
therefore requires all of the following, and refuses otherwise:

- **PDFium's page count equals qpdf's**, `[page-identity-mismatch]`;
- **every page's display size, as PDFium reads it, equals qpdf's display frame**, not only the
  rendered page's, `[page-identity-mismatch]`. qpdf's frame is compared as the display frame, **with
  width and height swapped for `/Rotate` 90 and 270** (decision 66, item 1). PDFium reports 32-bit
  floats and qpdf 64-bit, so the comparison is **qpdf's value rounded to 32 bits equals PDFium's**,
  and the census counts every disagreement. On the web this is the first time PDFium vouches for a
  frame, so `[media-box-unverified]`'s web half can be lifted. That is an acceptance change, and it
  needs a census;
- **the route, and its cost.** Every page other than *N* is read by the no-load size,
  `FPDF_GetPageSizeByIndexF`, which the bridge already calls without `FPDF_LoadPage` (*Facts*). A
  load builds the page's display list, one uninterruptible parse of a hostile page each, so v1 does
  not load every page. Whether the no-load size reflects `/Rotate` is **measured on the pinned build
  before it is relied on**; if it does not, the comparison swaps PDFium's side too, and says so. Page
  *N* is loaded anyway, for the render: **its rotation is compared exactly**, PDFium's
  `FPDFPage_GetRotation` (a new bridge export) against qpdf's. The residue, stated: on a page other
  than *N*, a rotation disagreement the size cannot show (a square page, or 180 degrees) is not
  seen. Each no-load read checks the deadline between pages, and the census costs the pass;
- **a page tree whose shape lets two readers disagree about which object is page *N*** refuses,
  `[page-tree-ambiguous]` (decision 57, item 1): any warning qpdf raises while enumerating pages;
  any page or node reachable by more than one path through the page tree; a node count the tree does
  not bear out; a node whose kind is not exactly one of page or intermediate node;
- **resolution happens at open, inside the trap** (§1), so the page tree every check reads is the
  one the write uses. `[engine-repaired-input]` stays, and is read again after the constructors. A
  repaired file is where the two engines' page trees part. `[reference-to-nothing]` and
  `[reference-unreadable]` stay with it, for the same reason;
- **the every-object scan** (decision 61, item 3). `[engine-repaired-input]` is qpdf's flag only, and
  the two engines can resolve one object differently without qpdf repairing anything. So #227's
  reference pass and the 2026-10-14 object-syntax rows, `[reference-to-nothing]`,
  `[reference-unreadable]`, `[name-read-differently]`, `[syntax-read-differently]` and
  `[null-value]`, **run over every object in the file in v1**, not only page *N*'s graph. That is
  their shape on `main` already (*Facts*): the pass lexes after every `obj` and `trailer` keyword and
  inside every object stream qpdf would decode, and resolution at open has already parsed the whole
  cross-reference table, so it adds no second pass. v1 must not narrow it. What it does not close is
  in §4's undetectable list: the engines reading page *N*'s content differently by a route no row
  measures.

**What the frame comparison adds is limited.** On a document whose pages all share one size and
rotation, which is most documents, comparing frames adds nothing beyond the count. There, identity
rests on the structural refusals above. That is said, not smoothed over.

The census renders each output's page *N* in PDFium and compares it with the painted bitmap: a
test-time check that the replaced page is the one drawn, and **the independent reading that gates S9
for page identity** under rule 19 (§11). **Open:** whether PDFium exposes an
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
a side for a square), under its default `max_pixels` of 268,435,456. Of the 342 framed pages in the
committed fixtures' `page-frames.tsv` (369 page rows, 27 of them refusals), one exceeds the web
ceiling: the OCR scan. The 99-document census is the
slice's to run (rule 2), and it reports the pages each ceiling refuses (§2 rule 6).

**`max_duration_ms` is cooperative, and one deadline covers both workers** (§1). The render is
progressive (ADR 0027's 2026-09-17 amendment), checkpointed between pauses. **PDFium's page load,
qpdf's write (which encodes the image through zlib), and each stream decode in verification are each
one engine call that nothing can interrupt**, so overshoot of up to one such call remains, as
everywhere. Since the render became progressive, the uninterruptible part is the page load: ADR 0027's
Amendment 2 measured it at **1,166 ms and 757 MiB** for 3 M paths, and **3,511 ms and 1,765 MiB** for
10 M. (Its §2a figure of 34 s was a one-shot render, before the progressive amendment.) So the load,
not the write, is where an overshoot will be spent. V6 and every walk check the deadline between
objects (§2).

**`max_memory_bytes` is detected, not bounded.** A length-based estimate is made before the render. It
spans **two heaps** on the web, so it is stated per heap, and natively it is their sum in one:

| heap | what, per pixel | bytes per pixel |
|---|---|--:|
| render worker | PDFium's bitmap (4), the Rust module's copy of it, unpainted then painted (4), the RGB conversion (3) | 11 |
| redaction worker | the RGB buffer received (3), qpdf's copy at `replace_stream_data` (3), the encoder's output (up to about 3), verification's decoded copy (3) | 12 |
| sum | | **about 23** |

That is about 46 MB and 50 MB in the two web workers at the web's ceiling, and about 2.06 GB in one
native heap at native's. **Two costs are not per pixel and are added by length:** the write decodes
and re-encodes other pages' streams with generalized filters, and V6 holds two decoded copies of a
stream at once, up to 256 MiB each. A measured check follows after. **What PDFium's rasteriser
allocates for itself is bounded by nothing** (ADR 0027's *What is bounded*), as for every render.

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
  the write, and the old streams are unreachable and not written: a content stream of page *N* that
  something kept would still reach refuses (rule 5). It holds the input, which #199 already covers,
  and the painted bitmap, which carries nothing removed. ADR 0004's wipe-on-free stays.
- **The render worker** (decision 57, item 6). ADR 0004's hook cannot reach `pdfium.wasm`, which
  arrives already linked. The owner chose route (b): **zero both of the render worker's linear
  memories, `pdfium.wasm`'s and `burrow_wasm_render_bg.wasm`'s, then terminate the worker.**
  **Termination is the control**: once the worker is terminated, its memories are unreachable from
  any script. Zeroing is defence in depth for the time between the hand-off and the termination.
  Building PDFium from source with the hook (route (a)) is not taken, and stays a condition for
  revisiting. The conditions on (b):
  - **on every path**: the reply, a refusal, an error, a trap and the deadline. Where zeroing cannot
    run, because the worker is stuck inside an uninterruptible page load, the redaction worker
    terminates it anyway, and termination is what holds;
  - zeroing takes a **fresh view** of each memory after the last engine call, since a memory can grow,
    and **no engine call runs after it**;
  - each memory is asserted **not shared** (no `SharedArrayBuffer`), so no other agent holds a view;
  - the unpainted pixels move from `pdfium.wasm`'s memory into `burrow_wasm_render_bg.wasm`'s by a
    **view-to-view `set` and nothing else** (§1 step 3), and the digest is computed **in the Rust
    module's memory**, with no `SubtleCrypto` call, `slice` or `postMessage` of unpainted pixels, so
    no JavaScript-owned copy exists to miss. **The canary has a mutation for this route**: replacing
    the view-to-view copy with the bridge's slicing `copy_out` must turn the JavaScript-heap canary
    red;
  - **no file-system copy** of a decoded stream is made in the module's virtual file system;
  - the bridge's own JavaScript copies are listed and each is zeroed or never made;
  - **the canary** plants a text and a pixel pattern in the region and searches **both linear
    memories and the JavaScript heap** at the reply, with a full scan. **Each wipe has its own
    mutation**, which skips it and turns the canary red.
- **The redaction worker's JavaScript.** The painted bitmap reaches it, and carries nothing removed.
  It does not reach the page before verification (D9, and §8).
- **The selection picture**, which is unpainted and is the person's own input, keeps #136 D1's
  posture: terminate on file change, before a run, on cancel and on `pagehide`. **It is the same
  pixels as the run's unpainted render** (rule 7), and it lives in the page's heap until one of those
  events. D1 exempts it as the person's input, and D4's rule applies to the run's copies, not to it.
  That is the residue, stated.
- **Native.** PDFium's heap is the process heap, and it is not wiped. Native posture is M3/M4's, as
  ADR 0004's amendment already says of qpdf, and the native API's rustdoc says so.

### 8. Workers, modules, and R8 to R10

- **PDFium stays in the render worker; qpdf stays in the redaction worker.** No worker holds both
  engines.
- **What crosses the boundary, and nothing else.** From the redaction worker to the render worker: the
  input bytes, the page index, the region, the deadline that is left, and the selection record (§2 rule
  7). From the render worker back: PDFium's page count, every page's display size and page *N*'s
  rotation (§5), the painted RGB bitmap, its width, height and digest, and a refusal if there was
  one. R8's spec extends
  to the render worker and to each of these fields.
- **The hand-off does not go through the page's code.** The bitmap travels on a dedicated channel
  between the two workers, transferred port to port rather than copied, and the page never receives
  a pixel buffer at all.
- **R8** is unchanged: one reply carrying output, after verification, from the worker that wrote the
  document. **The painted bitmap is an input to the write, not output.**
- **R9's words are "nothing is written outside the wasm heap before verification passes"**, and the
  painted bitmap leaves a wasm heap before verification. The reading this record argues: R9 names the
  ways a page keeps a file (OPFS, the File System Access API, a `blob:` URL), and none of them is used.
  The bitmap moves from one worker's heap to the other's, carries nothing the redaction removes, and
  is never handed to the page. **Round 1's security review confirmed this reading, on four
  conditions, which are now part of the rule:**
  1. R9's stubs are armed in **both** workers, the render worker as well as the redaction worker;
  2. the **one** transfer is port to port, between the two workers;
  3. the page **never receives a pixel buffer**;
  4. on a refusal, the redaction worker **drops the buffer** it received, and it is never written.

  The owner's reading of this record confirms it. If it does not hold, R9's wording is amended by the
  owner, not read around.
- **R10** concerns the bytes verified and the bytes delivered. **On the web it is bound at run time by
  #206's digest once it merges.** Its final hop needs #206's web half, re-scoped to the runtime digest
  only, since the second reading it also carried is v2 by decision 55. This record does not say that
  R10 is met; the owner records that. The hand-off digest in §1 binds the *pixels* to the output. It
  does not discharge R10. A v1 twin of `redact_verify_same_bytes` holds R10 over the flatten write.
- **SHA-256 becomes a runtime dependency.** Today `sha2` is a dev- and build-dependency only
  (*Facts*). Rule 7's digest needs it in `burrow_wasm_render_bg.wasm`, which every tool loads, and
  natively in the operation. So the license-auditor pass decision 59 (item 3) asks for on `sha2`,
  with its transitive runtime crates as resolved in `Cargo.lock`, extends to the render build; it gets
  a `THIRD_PARTY_NOTICES.md` entry; and it counts in rule 22's line for the render bundle.
- **Bundles and budgets.** The paint, the digest and their refusal strings are redaction-only code in
  a bundle every tool loads (`burrow_wasm_render_bg.wasm`). DECISIONS.md rule 22 applies: under 0.5% of
  the page's total, or a separate bundle that only `/redact-pdf` loads. The slice measures which. The
  gates this touches are named in *Fixtures and census*.
- **The v2 rewriter stays callable behind a cargo feature, and is excluded from the shipped module**
  (decision 57, item 10). Its tests run with the feature on. Visitors do not download it.
- **New qpdf exports**: `qpdf_oh_new_stream`, `qpdf_oh_new_dictionary`, `qpdf_oh_new_name`,
  `qpdf_oh_new_array`, `qpdf_oh_new_real_from_string` (not `_from_double`; §1), `qpdf_oh_append_item`, and
  the resolution route of §1. None is in the wasm export list today (*Facts*). Each is listed in
  `EXPORTED_FUNCTIONS` and held by `check-wasm-exports`; each untrapped one is argued in
  `qpdf-untrapped-accepted.toml` (§1, including `new_array` against `qpdf/ffi.rs`'s comment); and
  their cost to `qpdf.wasm`, which every tool loads, is measured.
- **New PDFium bridge exports**: `FPDF_PageToDevice` and `FPDFPage_GetRotation`, neither exported by
  the bridge today, and the view-to-view copy of §1 step 3.
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
   (Bookmarks and links into this page still go to it, moved to the same place on the picture. A
   few that are written in a form no reader agrees on, or kept in a part of the named-destination
   list that cannot be read, are left as they were, and may land elsewhere on the page. On a page
   that was turned sideways, a link that fits the page's width or height keeps the fit but not its
   scroll position.)
5. *(Only when §2 rule 5's report counts a survivor that counts.)* **Some of what this page used is
   still in the file, because the file keeps it for other reasons.** Of what is stored as streams of
   data, exactly these: the embedded font programs and character maps of the fonts this page used,
   which can show which characters this page used, and colour (ICC) profiles. Also the definitions
   that are not stored as streams of data, with whatever they contain: for example font,
   colour-space, graphics-state and shading definitions, marked-content properties, and links that
   hold more than a page and a position. Anything else this page used that the file would keep makes the redaction refuse:
   images and drawings, patterns, shadings and functions stored as streams, masks, drawn letter shapes
   (a Type 3 font's glyphs), the page's thumbnail and its own content.
6. **Form fields on other pages, and their values, are not changed. A digital signature in the
   document will no longer validate.**
7. **The file usually grows**, because the page is now an image.

### 10. ADR 0029, section by section

| 0029 | for v1 |
|---|---|
| *Context* | Kept as history. Its central measurement, that text survives in many places besides the glyphs, is why flatten drops the page's content wholesale |
| *The constraint that shapes every decision below* (the catalogue wall) | **Superseded as fact**: #239 made the trailer, and through it the catalogue, reachable. v1 writes to the catalogue only to remove `/StructTreeRoot` and `/MarkInfo`, to translate the coordinates of destinations into page *N* it holds, and to translate the `/R` of each article bead on page *N* in its `/Threads` (§3; decision 70, item 2), and nothing else |
| §1 rewrite content streams and fonts | **v2** |
| §2 page-key allowlist | **Kept, narrowed** to §3's seven keys |
| §3 per-channel table | **Superseded for v1** by §2 here. Content-stream channels (rows 1–9, 18–22 of spike 0006) are handled by not carrying the content. The page's `/Thumb` and page metadata go by §3. Annotations are flattened or refused (rules 2, 3 and 9). Optional content hidden on the page is not drawn and not carried; layer names are disclosed (rule 4). Images and vector paths in the region are painted. **Every content refusal in the table is v2's**, because burrow does not read the content |
| §3 rows kept for v1 | The rows that keep qpdf's object graph and page tree the ones PDFium reads, **over every object in the file** (§5): `[engine-repaired-input]` (#224), `[reference-to-nothing]` and `[reference-unreadable]` (#227), and the 2026-10-14 object-syntax rows `[name-read-differently]`, `[syntax-read-differently]` (vertical tab) and `[null-value]` (a value qpdf drops on write). The frame rows, each still raised by `page_frames` or the region conversion (`redact/frames.rs:107-118`, `pdfsyntax/region.rs:177-183`; `[page-out-of-range]` by `redact/steps.rs:140` and `:180`): `[page-frame-unreadable]`, `[user-unit-not-one]`, `[no-display-box]`, `[display-box]`, `[user-unit]`, `[rotate]`, `[rotate-not-whole]`, `[page-tree-depth]`, `[page-out-of-range]` and `[region-extent]`. The annotation-reading rows rules 2, 3 and 9 still raise: `[annots-too-long]` (`redact/steps.rs:1335`) and `[annotation-popup-unreadable]`. `[document-poisoned]` (`redact/mod.rs:473-482`), kept as the name of the refusal a poisoned document gives; no production step raises it yet, and v1 keeps that shape. `[annotation-dependent-kept]` is reused and widened by rule 3, and covers #240's shared `/Annots` array. The resource-graph rows `[resource-graph-cycle]`, `[resource-graph-depth]` and `[resource-graph-work]` are **not** kept as they are: v1's walks are bounded by §2's visited set and `[object-walk-too-large]`, and a cycle is no longer an error to a walk that enters each object once |
| §3 rows changed for v1 | `[acroform-field]` narrowed (rule 2). `[structure-tree]` replaced by the drop (rule 1). `[media-box-unverified]`'s web half lifted by §5. `[page-attribute-inherited]` is not needed: `/CropBox` and `/Rotate` are written explicitly (§3), and inherited resources are in rule 5's set *S*, which starts from every inheritable key on every ancestor; an inherited image or form that survives refuses there. **Lifting its `/Resources` and `/Rotate` halves are acceptances**, each with a census row (*Census*). `[annotation-in-region]` (#136 D3) cannot arise as D3 framed it, since nothing in the region is silently kept; whether it is retired for v1 is the owner's (*Open questions*) |
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
  same-source assertions are **the closed list in §4's *What it leaves undetectable***: this
  record's revisions while it is Proposed, and its dated amendments once it is Accepted. On 2026-10-27
  that list is five items, each with the independent reading that gates S9:
  1. **the survivor set**, which one walk decides and checks: the census's decoded I2 scan;
  2. **page identity across the two engines**, held by §5's refusals on qpdf's reading of the tree:
     the census render of each output's page *N* against the painted bitmap;
  3. **the engines reading page *N*'s content differently** (decision 61, item 3): the census's
     decoded I2 scan;
  4. **the region-to-pixel arithmetic**, only where the display frame both sides share is wrong in
     the same way (the two sides share no other step, §1 steps 4 and 6): the rotation by box-origin by
     crop-offset matrix, and the census render;
  5. **a destination into page *N* at a site §3 does not list**, which keeps its input position because V7 reads by
     the same list (decision 68, item 2; navigation, not content): the census's scan of every output
     for any array whose first element is page *N*'s reference.

  If this record is rejected, there is no v1 flatten list, and rule 19's instances are v2's alone.
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
| §1 resolution at open | a file whose resolution repairs | the same file, well formed | skip the forced resolution: the repair surfaces after the constructors, and the re-check refuses; skip the re-check too: the test goes red; assert on the call's return instead of `fixed_dangling_refs` |
| §1 step 6 `[paint-disagrees]` | a fake transform one pixel off; a fault planted in the paint side's display-to-user conversion; PDFium's rounding half a pixel the wrong way | the true transform | delete the cross-check; compute the check through user space |
| §1 step 6 size and length | a render worker reporting *W* or *H* one more than the frame gives; a hand-off buffer one byte short, and one byte long | exact size and length | trust the reported size; delete the checked multiplication |
| §2 walk membership (H-2 class) | an object off the page tree that declares itself a page and holds the page's content, kept by a referrer; an off-tree template page | a real page-tree member | recognise members by `/Type`: the fixture goes `Ok` and red |
| §2 walk bound `[object-walk-too-large]` | a graph one object over the ceiling | a graph at it | drop the visited set; drop the deadline check |
| rule 1 `[structure-tree-kept]` | a tree element kept by another page's object; an **untyped** element kept by a document-level object; each tree entry in rule 1's list | the same trees with no kept referrer | key membership on `/Type`: the untyped fixture goes `Ok` and red |
| rule 1 drop | a tagged document claiming tags | an untagged one | keep `/MarkInfo` |
| rule 2 `[form-field-on-page]` | a widget with no field; a widget whose `/Subtype` is a string; a field kid with `/FT` only on an ancestor; a `/Fields` entry whose `/P` names the page; a signature field | a field whose widgets are all on other pages | drop each clause in turn |
| rule 2a `[xfa-form]` | `/XFA` as an array, as a stream, as an integer; `/NeedsRendering` on the catalogue | neither key | key on the parsed value |
| rule 3 `[annotation-dependent-kept]` | an annotation listed on two pages; an appearance shared with a kept annotation; an action that names a page annotation; a shared `/Annots` array | the same shapes on two unrelated pages; **an annotation whose appearance uses a font a kept page uses** (rule 3 does not fire; rule 5 discloses the font) | narrow to `/IRT`, `/Parent`, `/Popup`; widen *A* to the appearance resources: the font twin refuses and goes red |
| rule 4 `[metadata-thumbnail]` | catalogue XMP carrying a page thumbnail; the same under a rebound prefix; in each encoding XMP permits; an undecodable metadata stream; an unreadable packet | XMP without one; a property of the same local name in another namespace | delete the check; key on the prefix |
| rule 5 `[drawing-survives]` | an image only this page draws, in an inherited `/Resources`; an inherited form; an image and a form a **kept page also draws**; page *N*'s `/Contents` stream shared with a kept page, and kept by an off-tree object; a tiling pattern in *S*; an image in an annotation appearance's resources, kept elsewhere; page *N*'s `/Thumb`, with no `/Subtype`, kept by an off-tree referrer | a font and its embedded program shared with a kept page (`Ok`, disclosed); `/AcroForm` default resources holding the page's font (`Ok`, disclosed) | start *S* from the page's own keys only: the inherited fixtures go `Ok` and red; exempt drawings a kept page draws |
| rule 5 roles (decision 69) | each kept by a kept page or an off-tree referrer: **a drawn image whose `/Subtype` is not a name, and a drawn form likewise** (each an `/XObject` resource value); **a tiling pattern whose `/PatternType` is not an integer**; **a Type 3 glyph procedure** (a `/CharProcs` value); **a mesh shading** (a shading stream); a stream reached both as a font program and as an `/XObject` value, **built in both arrival orders** (font program first, and `/XObject` value first), each fixture asserting which arrival the walk met first; a `/CIDSet` stream; each ambiguous role rule 5 lists | **a twin of the same stream bytes reached only as an embedded font program** (`/FontFile2` under a font's `/FontDescriptor`): `Ok`, disclosed; a `/ToUnicode` map and an ICC profile in a `/ColorSpace` resource, each kept elsewhere: `Ok`, disclosed | **classify by declared type** (`/Subtype`, `/PatternType`, `/ShadingType`) instead of role: the malformed-type fixtures go `Ok` and red; keep the first role the visited set records instead of every role: the font-program-first fixture goes `Ok` and red (the `/XObject`-first one refuses either way, which is why both orders are built) |
| rule 5 destination arrays (decision 70) | one indirect destination array, shared by page *N*'s link and an outline item, holding a string or a dictionary item: disclosure 5 counts it | the same array with only a page reference, a name, numbers and null: `Ok`, a survivor V5 checks, and **no disclosure 5** | count every destination array; count none |
| rule 5 walk | a destination or `/P` back to another page | none | remove the stop at pages: the report fills with every page's objects |
| rule 6 ceilings | 983 × 983 pt (2048 × 2048 = 4,194,304 px) and 983 × 983.5 pt (one row more); 2992.5 × 6888.25 pt (6235 × 14351 = 89,478,485 px) and one row more. Every side is exact in 32 and 64 bits, and **the fixture asserts that both readings give the same *W* and *H*** | the ceiling minus a row | check after allocation |
| rule 7 binding | the same pixels from another file; another page with the same pixels; a different size | the recorded file, page and size | drop each field of the record |
| rule 7 and §3 orientation (B1) | a `/Rotate 90` page, and a `/Rotate 270` one, whose output rendered back matches the selection picture's orientation, with `/Rotate 0` and a swapped `/MediaBox` | a `/Rotate 0` page | keep the effective `/Rotate` on the written page: the picture turns twice and the test goes red; leave `/MediaBox` unswapped |
| rule 8 edge | a hairline one pixel outside the region | ink two pixels outside | remove the extra pixel |
| rule 9 annotation flags | each of NoView, Print, Hidden, ToggleNoView; `/AP /N` as states with no `/AS`; an annotation under optional content; a `/Rect` outside the box; a Popup open and closed | an ordinary annotation | add `FPDF_PRINTING`, or a form-fill handle |
| §3 keys | an inherited `/CropBox` smaller than the media box | a declared one | omit `/CropBox` |
| §3 destinations, `/XYZ` | a link on another page with `/XYZ` into page *N*, on a page whose crop starts away from the media box's origin; the same with a null *left*, and with a null *top* | a `/XYZ` into another page (unchanged) | skip the translation; shift by the `/MediaBox`'s corner instead of the display box's: the crop fixture goes red; write a null as 0 |
| §3 destinations, `/FitR` | a `/FitR` into page *N* at `/Rotate` 0 and 180 (corners swap order at 180); `/FitH`, `/FitV` and their B forms at 0 and 180; an article bead's `/R` | the same into another page | drop the ordering: the 180 fixture goes red; map `/FitH` by *x*0 |
| §3 destinations, named | a name in the catalogue's `/Dests` and a string in the `/Names` `/Dests` tree, each into page *N*, as an array and as a `/D` dictionary, referred to by an outline item and by a `/GoTo`; a name defined twice in the tree; one name in both dictionaries; a leaf reachable only past a wrong `/Limits` | a named destination into another page; a name that resolves to nothing (unchanged) | search the tree by `/Limits` instead of walking it: the wrong-`/Limits` fixture goes red; translate at the referrer |
| §3 destinations, sites | an outline item's `/Dest`, and its `/A` with a `/Next` chain to a second `/GoTo`; a `/Next` cycle; `/OpenAction` as an array and as an action; another page's `/AA`, as a direct dictionary holding a direct destination, and a field's `/AA` on another page; one indirect destination array shared by a link and an outline item, into a page *N* that is cropped away from its media box's origin and turned a quarter | the same sites into another page | skip each site in turn; drop the visited set: the shared array is shifted twice and goes red |
| §3 destinations, rotated | `/Rotate 90` and `/Rotate 270` pages with `/XYZ` (one with a null *left*), `/FitR` and `/FitH` into them, each rendered back to the same displayed place | a `/Rotate 0` page | leave the axes unswapped; move a null with its old slot rather than its axis; translate `/FitH`'s coordinate instead of writing null |
| §3 destinations, untranslatable | a two-operand `/XYZ`, an unknown form name, a string operand, into page *N* (each unchanged, counted); an integer page element (unchanged) | well-formed twins (translated) | "correct" the malformed one; read the integer as a page index |
| §3 and V6, article beads (decision 70, item 2) | a thread of two beads, one on page *N* and one on a kept page, whose honest output **passes V6**, the kept page's bead unchanged and page *N*'s `/R` translated; V7's fake bead with `/R` untranslated | a thread on kept pages only (unchanged) | drop beads from V6's exception: the honest two-page thread refuses and goes red; skip the bead translation |
| V7 outside page *N* (decision 70, item 3) | the fake that translates a destination into another page, placed in the outline and in the `/Names` `/Dests` tree | honest output, every other destination byte-equal | delete the byte-equal check; **widen the writer's identity rule to `/Type /Page`**: V7 must refuse, and the test goes red if it does not |
| V7 beads outside page *N* (decision 70, item 3) | a thread of two beads, one on page *N* and one on a kept page that has no `/B`, so the second is reached only through the catalogue's `/Threads`; the fake that translates the second bead's `/R` | honest output, the kept page's bead byte-equal | delete the beads from V7's byte-equal clause; **widen the writer's identity rule to `/Type /Page`**: V7 must refuse, and the test goes red if it does not |
| V7 round trip | mapped values that are not exact in decimal, on native and on the web | values exact in decimal | compare within a tolerance instead of exactly |
| §5 `[page-identity-mismatch]` | engines disagreeing on count; on one page's display size; on page *N*'s rotation; a quarter-turn page whose sizes agree only unswapped | agreeing | compare the rendered page only; compare without the swap |
| §5 `[page-tree-ambiguous]` | a page reachable by two paths; a count the tree does not bear out; a node of the wrong kind; a node of two kinds | a well-formed deep tree | delete each clause |
| §5 frames as floats | a frame value where 64-bit and 32-bit readings differ | an exactly representable one | compare without rounding |
| §5 every-object scan | each object-syntax row's shape in an object outside page *N*'s graph | the same shape absent | narrow the scan to page *N*'s graph |
| §4 V1–V7 lies | each fake listed in §4 | an honest fake | delete each check |
| §4 transform matrix | every rotation (0, 90, 180, 270) by box origin (zero, offset, negative) by crop offset: the paint side (`FPDF_PageToDevice`) against burrow's display-space check | — | a sign flip in the display-to-user conversion; a missing swap in the check |
| §7 heap canary | a dense page with planted text and pixels in the region, on the reply, a refusal, an error and the deadline | — | skip each wipe in turn; skip the termination; replace the view-to-view copy with the slicing `copy_out` |
| §8 R8 / R9 | an extra posted field; a stub called in the render worker; a pixel buffer posted to the page; a buffer kept after a refusal | the listed fields | emit a chunk; call a stub; post the bitmap through the page |

**Carried over** from v2 and still applying, extended where v1 changes them: `page-frames.tsv`; the
fuzz targets `redact_page_frames`, `redact_references` and `redact_verified_output`; `secret_leak.rs`;
the R8 and R9 specs; the heap canary (with its new PDFium half); `test-redaction-refusals-are-reached.sh`;
the census harness. **New:** a v1 twin of `redact_verify_same_bytes`, and fuzz targets for the flatten
operation, for its write half, and for the survivor and structure walks. **v2 only:** `outcomes.tsv`
as it is (its goldens split into v1 and v2), `redaction_defences.rs`, the geometry and rewriter fuzz
targets including `redact_shared_contents`, and the differential ledger.

**Gates the design touches**, each checked by the slice: `check-redaction-not-in-base` (the render
bundle gains paint, digest and refusal strings); `production-build.test.ts`'s held `redact` operation;
`check-pdfium-is-render-only`'s partition; the R8 and R9 specs, which today cover only the redaction
worker; rule 22's line for `burrow_wasm_render_bg.wasm`, which now carries `sha2`; `check-wasm-exports`
and `EXPORTED_FUNCTIONS`, for qpdf's six constructors and PDFium's two new exports; and the
license-auditor pass and `THIRD_PARTY_NOTICES.md` entry for `sha2` as a runtime dependency (§8).

### Census

The 99 real documents plus the fixtures, **every page flattened**, under both the web's and native's
ceilings, and run twice to measure determinism. The bar for every new refusal is **1% of real
documents** (DECISIONS.md rule 2), registered before the run. It reports:

- **every new refusal against the 1% bar**: `[form-field-on-page]`, `[xfa-form]`,
  `[annotation-dependent-kept]` (widened), `[structure-tree-kept]`, `[metadata-thumbnail]`,
  `[drawing-survives]`, `[page-tree-ambiguous]`, `[page-identity-mismatch]`, `[paint-disagrees]`,
  `[render-not-reproducible]`, `[object-walk-too-large]`, and `[page-too-large-to-flatten]` per
  ceiling. Over the bar: the number
  goes to the owner, and the rule is not adjusted, except where an outcome below is already decided;
- **at least one real document moving from refused to `Ok`** for each acceptance: the tree drop (rule
  1), forms elsewhere (rule 2), the web's inherited media box (§5), the written `/CropBox` (§3),
  `[page-attribute-inherited]`'s `/Resources` half and its `/Rotate` half (§10), and the content
  refusals as a whole;
- **`[drawing-survives]`'s cost by producer and by role**: page *N*'s own content, images, forms,
  tiling patterns, and, as its own figure, **the cost of decision 69's wider refusal**: the documents
  that refuse only through a role the earlier text disclosed (Type 3 glyph procedures, shading and
  function streams, soft masks, `/CIDSet` and `/CIDToGIDMap` streams, the ambiguous roles of rule 5,
  and streams reached in more than one role). **Over 1% of real documents, the work stops and the figure
  comes back to the owner** (decision 69; *the census decides*, below);
- **streams reached more than one way, under both readings** (decision 71, item 1): reached in more
  than one role, which refuses, and reached through more than one path in one disclosed kind, which
  stays disclosed, each counted, so the difference between the rule and the literal reading is a
  figure;
- **`/CIDSet` and stream `/CIDToGIDMap`, as their own figure** (decision 71, item 2): under 1% of
  real documents they refuse; otherwise they join the disclosed allowlist by name, and the figure is
  reported to the owner;
- the disclosed survivor distribution by role, with the walk that stops at pages, and the destination
  arrays that do not count toward disclosure 5 (decision 70, item 1);
- the object counts the walks meet, from which `[object-walk-too-large]`'s ceiling is set;
- whether PDFium's no-load page size reflects `/Rotate`, and what the per-page identity pass costs
  (§5);
- PDFium's 32-bit frames against qpdf's 64-bit ones: every disagreement;
- how many files resolution at open repairs;
- render determinism three ways (progressive and one-shot, with pauses, through the native font
  mapper), and pixel identity across wasm, native x64 and native arm64;
- render time, write time and V6 time per page;
- a five-reader render of every output page (rule 17's twin render), and a decoded I2 scan of every
  output (§4);
- a PDFium render of each output's page *N* against the painted bitmap (§5);
- shared `/Annots` arrays and annotations on the real set;
- **every destination into page *N***, by site and form, translated and kept unchanged by reason
  (§3), and the scan for page-*N* arrays held outside §3's sites (§4).

**The census decides five outcomes, already worded:**

| | if the census shows | then |
|---|---|---|
| decision 3, `[metadata-thumbnail]` | it refuses **1% or fewer** of real documents | it refuses |
| | it refuses **more than 1%** | the XMP thumbnail is **stripped on output** instead, and the refusal goes |
| decisions 61, item 2, and 69, `[drawing-survives]`, with the wider refusal | it refuses **1% or fewer** of real documents | it refuses |
| | it refuses **more than 1%** | **the work stops**, and the owner is brought the figure and the producers. It still refuses; it is never moved to disclosure (decision 57, item 4's disclosure branch is superseded) |
| decision 71, item 2, `/CIDSet` and stream `/CIDToGIDMap` | their cost is **under 1%** of real documents | they refuse |
| | it is **1% or more** | they join the disclosed allowlist **by name**, and the figure is reported to the owner |
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
  are unchanged; the only edits outside page *N* are the coordinates of destinations into it and the
  `/R` of article beads on it (§3, V7; decision 70, item 2).
- **Links and bookmarks into the redacted page keep their place on it** (decision 68, item 2), by a
  translation that walks every destination site in the document. On a quarter-turned page a `/FitH`
  or `/FitV` loses its scroll position, and a malformed destination is left as it was (§3).
- **New qpdf C entry points, and a resolution at open** (§1, §8). Six constructors, five of them
  untrapped, each with an argued entry in `engines/qpdf-untrapped-accepted.toml`; `new_stream`'s
  argument rests on the resolution at open, and is re-checked after the constructors.
- **SHA-256 ships in the render bundle every tool loads**, and natively (§8).
- **Documents whose images or forms sit in a shared or inherited `/Resources` refuse** (rule 5), a
  common producer layout. So does a document whose page *N* used any other stream that something
  else keeps, outside the three disclosed roles (font programs, CMaps, ICC profiles): a shared Type 3
  font's glyph procedures, a shading or function stream, a soft mask (decision 69). The census
  measures what that costs; over 1%, the owner decides (decisions 61, item 2, and 69).
- **The web refuses Tabloid and A3** at 150 dpi, and native refuses above 89,478,485 pixels (§6).
- **Native heaps are not wiped** (§7).
- **Rule 17 tier: acceptance.** v1 accepts much of what ADR 0029 refuses: every content refusal, the
  tree, forms elsewhere, the web's inherited media box, an inherited crop, and inherited resources and
  rotation (`[page-attribute-inherited]`'s other halves). So the slice gets round 0
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
- **Rendering qpdf's rewrite instead of the original bytes** (decision 61, item 3: a future option,
  not taken for v1). PDFium would draw qpdf's canonical rewrite of the input, so the two engines would
  no longer read different cross-reference tables or escapes. It would not remove PDFium's own limits
  on names, so §5's every-object scan would still be needed. It would rebind rule 7's input digest to
  the rewrite and change the pipeline, so it is the owner's to take.
- **The census's `[drawing-survives]` figure over 1%** (rule 5): the owner decides, with the figure
  and the producers.

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

## Open for the build PRs

Under decision 64, item 1, a Medium or Low finding that only a measurement or slice code can close
becomes a fixture or test in the build PRs. The text above states each rule; what is open is the
measurement or the code that holds it. Each is listed with what it becomes:

| from | what stays open | becomes |
|---|---|---|
| security M-1 | whether PDFium's no-load page size reflects `/Rotate`; the per-page identity pass's cost | a measurement test on the pinned build (a `/Rotate 90` non-square page read without a load, against a loaded read), and a census row (§5) |
| security L-1 | the XMP thumbnail signal's namespace matching across every packet encoding | the rule 4 fixtures: rebound prefix, each encoding, unreadable packet, other-namespace twin |
| security L-6 | the route that reads qpdf's `fixed_dangling_refs` | the §1 resolution fixture's mutation "assert on the call's return instead of the flag" |
| code S3 | the untrapped-list arguments for `new_array` and the real constructor, `qpdf/ffi.rs:640-645`'s comment, and their `qpdf.wasm` cost | entries in `engines/qpdf-untrapped-accepted.toml`, the `check-wasm-exports` gate, and a rule 22 measurement |
| code S5 | `sha2` as a runtime dependency of the render bundle | the license-auditor pass, a `THIRD_PARTY_NOTICES.md` entry, and rule 22's render line |
| code S7 | the value of `[object-walk-too-large]`'s ceiling | the census's object counts, and the §2 walk-bound fixture, twin and mutations |
| code N3 | that both frame readings give the same *W* and *H* at each ceiling | the rule 6 fixtures' assertion |
| decision 68, item 2; round 2 security L-c and code S-1 | V7 compares exactly, so the write and the read-back must agree to the bit. **Write** each mapped value with `qpdf_oh_new_real_from_string`, from Rust's shortest round-trip decimal of the 64-bit value, written without an exponent; **`new_real_from_double` is not declared for this, or is excluded from it**, because it writes a fixed number of decimal places (`QUtil.cc:354-375`, 12.4.1) and so does not round-trip. **Read back** by the route burrow already reads numbers through, the handle's text parsed by Rust (`redact/frame.rs:383-386`), never qpdf's `atof` (`QPDFObjectHandle.cc:820-825`), whose result depends on the host's `LC_NUMERIC` and C library | the V7 round-trip fixtures, natively and on the web, with values that are not exact in decimal; a test that the written text has no exponent; and the mutation "write with `new_real_from_double`": the round-trip fixture goes red |
| round 2 security M-A | §2's walk bounds count objects, not entries, so one name-tree leaf or one direct `/Dests` dictionary with a very large number of entries is a single uninterruptible step | the walks, V5 to V7 and §3's enumeration count **entries** against `[object-walk-too-large]` and check the deadline every *k* entries; fixtures: one `/Names` leaf, and a direct catalogue `/Dests`, each just over the entry ceiling (refuses) and at it (`Ok`), and a deadline that falls mid-object (refuses on time); mutation: count objects again |
| round 2 security L-a | a translated destination array, or a bead's `/R`, that is also held by a kept page in another role (a form's `/Matrix` or `/BBox`, an annotation's `/Rect`); V6's exception would let the change pass | the exception holds only for an array the walk reached solely in the destination or bead role; any other array the writer would change **refuses, or is written as a fresh array** (the slice says which); fixtures: a destination array that is also a kept form's `/Matrix`, and a bead `/R` that is also a kept form's `/BBox`, each refusing (or each kept page's value byte-equal, if the slice writes fresh arrays); mutation: widen the exception to any array |
| round 2 security L-b | a destination coordinate that is an indirect reference to a number: `set_array_item` would replace the reference, and V5's set can then disagree with the report | an indirect operand is **malformed**, kept unchanged and counted (§3's table); fixture: a `/XYZ` into page *N* whose *left* is an indirect integer (unchanged, counted); mutation: resolve and translate it |
| round 2 code S-4 | a destination whose page element is an integer equal to page *N*'s index, which §3 leaves unchanged by identity, is neither counted nor disclosed | a census column counting such destinations at §3's sites, and a disclosure 4 clause if the census finds any; fixture: a local destination with an integer page element equal to *N*'s index (unchanged, counted) |

## Open questions, for round 2 and the owner

Decided by decision 57 since round 0: the web's `max_pixels` (item 7); rule 3's widening (item 1);
`/MarkInfo` (item 1); §7's wipe route (item 6); whether the v2 rewriter ships (item 10). Decided since
round 1: the survivor walk's start and stops (decision 61, item 1); which survivors refuse, superseding
decision 57 item 4 (decision 61, item 2); the every-object scan (decision 61, item 3); the page's
orientation (decision 66, item 1); page *N*'s own content stream (decision 66, item 2); that round 2
is final (decision 64, item 1); that tiling patterns refuse, that objects reached only through page
*N*'s annotation appearances are in the refusing set, and that the written page's coordinates start
at zero with destinations into it translated (decision 68, item 2). Decided since round 2: that a
surviving stream's kind is the role the walk reached it through, with three disclosed roles (decision
69); when a destination array counts toward disclosure 5, that article beads on page *N* stay
translated and are held by V6's exception and V7, and that V7 holds every other destination
byte-equal (decision 70); that "reached more than one way" means reached in more than one role, the
same disclosed kind through several paths staying disclosed, and that shading dictionaries stay
disclosed, ambiguous-role streams refuse, and `/CIDSet` and stream `/CIDToGIDMap` are decided by
their census cost (decision 71). R9's reading was confirmed by
round 1's security review, on the conditions §8 now states. Rule 9 is proposed (decision 56, item
3). Still open:

1. **D9 ("no after-picture of the result").** The output's pixels are exact (V3), so a picture of the
   result can no longer mislead the way ADR 0029's S4 amendment feared. Whether to show one is worth
   revisiting. It is not decided here. Until it is, the painted bitmap does not reach the page.
2. **Rule 7's reading.** This record reads it as one render for selection and output, bound by its
   record. The owner confirms the reading.
3. **R9's reading** for the painted bitmap (§8): confirmed by round 1, on four conditions; the
   owner's reading of the record confirms it.
4. **V6's wording**, and qpdf's decode level.
5. **`[annotation-in-region]`** (#136 D3) for v1.
6. **§5's residue**, and whether page-object identity can be had from PDFium.
7. **The §3 rows kept for v1** (§10): completed after round 1's code review; round 2 confirms it.
