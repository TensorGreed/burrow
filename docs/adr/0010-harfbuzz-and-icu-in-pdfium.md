# 0010. HarfBuzz and ICU inside PDFium: licences, and detecting components we did not declare

Date: 2026-09-10

## Status

Proposed — amends [0008](0008-widened-licence-allowlist.md).

The allowlist additions below take effect only when this is Accepted.
`tools/check-engine-licences.py` deliberately **fails** until then, so PR #14 cannot merge
on a licence surface nobody has signed off.

## Context

[ADR 0008](0008-widened-licence-allowlist.md) widened the allowlist after the first audit
of PDFium's WebAssembly package. M1 PR 1 added the two native Linux artifacts, and
auditing each separately — as [ADR 0004](0004-native-engines.md) requires — turned up two
components ADR 0008 got wrong.

Both were found by inspecting symbols in the shipped binaries, not by reading licence
files. That is the important part: **the licence files were not sufficient, and in one
case not even present.**

### HarfBuzz is linked, and no licence text ships with it

| Artifact | Evidence |
|---|---|
| `libpdfium.so` (arm64) | **557** demangled HarfBuzz symbols — `hb_blob_create`, `hb_font_create`, `hb_face_builder_add_table`, `hb_icu_get_unicode_funcs` |
| `pdfium.wasm` | `HB_FONT_FUNCS` string present; names are stripped, so symbol-level proof is impossible there |

None of the three artifacts ships a harfbuzz licence file. The 13–14 files in each
`licenses/` directory cover pdfium, abseil, agg23, fast_float, freetype, icu, lcms,
libjpeg_turbo, libopenjpeg, libpng, simdutf, zlib and (Linux only) llvm-libc — and no
harfbuzz. Upstream's `steps/08-licenses.sh` derives the set from `build.ninja` and has no
case for it.

So we would have been distributing HarfBuzz under a notice-requiring licence **without
the notice**, at a version we had not recorded. ADR 0008's Consequences section already
flagged HarfBuzz as one of five engines it had not audited; this is that bill arriving.

### ICU is linked — ADR 0008 says it is not

ADR 0008 recorded `linked = false` for ICU, reasoning that `args.gn` sets
`pdf_enable_xfa = false`. **That reasoning was wrong.** ICU does not arrive through XFA;
it arrives through **HarfBuzz's `hb-icu` integration**, which is why the symbol
`hb_icu_get_unicode_funcs` appears in the same binary.

| Artifact | Evidence |
|---|---|
| `libpdfium.so` (arm64) | **489** version-suffixed ICU 78 symbols — `ubidi_setPara_78`, `u_charType_78`, `ucase_toFullFolding_78`, `udata_openChoice_78` |
| `pdfium.wasm` | **18** RTTI type-name strings — `N6icu_7810UnicodeSetE`, `N6icu_7813UnicodeStringE`, `N6icu_7814UnicodeMatcherE` — plus 4 `icudt78`/`ICUDATA` markers. RTTI names exist only if the classes were compiled in |

ICU's licence file then contains a section — *"ICU License - ICU 1.8.1 to ICU 57.1"* —
whose identifier `ICU` is not on ADR 0008's allowlist.

## Decision

### 1. HarfBuzz: admit `MIT-Modern-Variant`, and carry the notice ourselves

**Version determined, not guessed.** PDFium's `DEPS` on branch `chromium/8044` pins
`harfbuzz_revision` to `886fc1e645388080b72f6d9b06347533a0018045`, and `meson.build` at
that commit reads `version: '14.3.1'`.

**Licence text fetched from upstream at that exact commit** and committed at
[`licences/harfbuzz-14.3.1-COPYING.txt`](licences/harfbuzz-14.3.1-COPYING.txt), sha256
`ba8f810f…ad64`, with the URL and a re-fetch command recorded in
[`licences/PROVENANCE.md`](licences/PROVENANCE.md). We commit it precisely because the
artifact does not.

**SPDX identifier: `MIT-Modern-Variant`** — added to the allowlist. This was *verified*,
not assumed: the file calls itself "Old MIT", and its text from the grant clause onward is
byte-for-byte identical to SPDX's canonical `MIT-Modern-Variant` text after
normalisation. It is OSI-approved.

Against ADR 0008's admission test: permissive ✓, no copyleft ✓, no field-of-use or
non-commercial restriction ✓, obligations limited to reproducing the copyright notice and
two disclaimer paragraphs ✓.

`src/ms-use/COPYING` (HarfBuzz's only sub-licence outside `test/`) is plain **MIT**,
already allowlisted, and **not linked** — zero `use_machine` symbols. Recorded because it
is distributed inside the tree, not because it executes.

**Notice obligation.** Reproducing the copyright notice and both disclaimer paragraphs is
mandatory, and it joins the FreeType FTL §2 and IJG condition (2) obligations on all four
surfaces ADR 0008 names.

#### Duplicate-symbol risk when we add our own HarfBuzz

ADR 0004 selects HarfBuzz `hb-subset` as our font-subsetting engine for **M2**. PDFium
already contains a full HarfBuzz. Statically linking a second copy alongside it will mean
**two definitions of every `hb_*` symbol in one binary**, and the outcomes are all bad:

- Static archives resolve first-wins, so which HarfBuzz answers a call depends on link
  order — silently, and differently per platform.
- Two copies means two allocator states and two sets of internal tables. Passing an
  `hb_face_t` created by one to the other is undefined behaviour, not a type error.
- On the web (option 1, [ADR 0006](0006-wasm-linking-strategy.md)) PDFium is a *separate
  module*, so its HarfBuzz is isolated by the module boundary — the collision is a native
  problem only. That asymmetry is itself a hazard: it would not reproduce in the browser.

Options when M2 arrives, none free: use PDFium's copy through whatever surface it exposes
(it exposes none publicly); build our `hb-subset` with a symbol prefix or a version
script; or link it dynamically and accept the loading cost. **This must be resolved before
`hb-subset` is linked**, not after a symbol clash is debugged, and it belongs to M2 and to
the M3/M4 mobile builds where static linking is the norm. Recorded here so it is a known
cost rather than a surprise.

### 2. ICU: admit `ICU`, and do not argue the section away

The tempting argument is that *"ICU License - ICU 1.8.1 to ICU 57.1"* cannot apply,
because the linked ICU is **78.2** — far outside that range — and ICU 78 is
`Unicode-3.0`, already allowlisted.

**We are not making that argument.** Reasons:

- The section sits under ICU's own heading *"This section contains third-party software
  notices … components included within ICU libraries"*. By the file's own framing it
  describes something shipped, not merely history.
- Deciding a licence is inapplicable is exactly the judgement ADR 0008 reserves to an ADR,
  and doing it to avoid adding a licence would be an exception dressed as an analysis.
- It costs us nothing to admit it. The licence is X11-style and passes the admission test
  outright.

So: **`ICU` is added to the allowlist**, understood as covering legacy ICU code that may
persist in the tree. Against the admission test: permissive ✓, no copyleft ✓, no
field-of-use restriction ✓, obligations limited to notice and a name-use restriction ✓.

#### Every third-party section of ICU's LICENSE, with evidence

Audited section by section against both artifacts. `native` counts demangled symbols in
`libpdfium.so`; `wasm` counts strings in `pdfium.wasm`, whose names are stripped.

| # | Section | SPDX | native | wasm | Linked? |
|---|---|---|---|---|---|
| — | UNICODE LICENSE V3 (headline) | `Unicode-3.0` | 489 | 18 | **Yes** |
| 53 | ICU License — ICU 1.8.1 to 57.1 | **`ICU`** | — | — | Legacy; admitted rather than argued away |
| 88 | cjdict.txt (Chinese/Japanese word break) | `BSD-3-Clause` | 0 | 0 | **No** — no `ubrk_`/`BreakIterator` |
| 294 | laodict.txt | bespoke permissive | 0 | 0 | **No** — same |
| 336 | burmesedict.txt | bespoke permissive | 0 | 0 | **No** — same |
| 378 | Time Zone Database | public domain | 0 | 1 | **No** — no `ucal_`/`zoneinfo64` |
| 403 | Google double-conversion | `BSD-3-Clause` | 0 | 0 | **No** |
| 434 | nlohmann/json | `MIT` | 0 | 0 | **No** |
| 462 | `aclocal.m4` (ICU4C only) | `GPL-2.0` + Autoconf exception | 0 | 0 | **No** — build script, not compiled |
| 497 | `config.guess`, `config.sub` (ICU4C only) | `GPL-3.0` + Autoconf exception | 0 | 0 | **No** — build script, not compiled |
| 527 | `install-sh` (ICU4C only) | MIT-old-style (MIT 1991) | 0 | 0 | **No** — build script |
| 544 | `sorttable.js` (ICU4J only) | X11 | 0 | 0 | **No** — ICU4J; we use ICU4C |

**The GPL sections remain a non-issue, and this is the third time it has been checked.**
They cover four ICU4C autotools files, each carrying the Autoconf exception, none
compiled. The break-iterator dictionaries — the components with the most unusual terms —
are **not linked**, which is what the evidence column is for.

Only the headline `Unicode-3.0` and the legacy `ICU` section describe code in a shipped
artifact.

### 3. Correct ADR 0008's record

ADR 0008's manifest table says ICU is not linked, with `pdf_enable_xfa = false` as the
reason. Its body is not rewritten — [ADR 0001](0001-record-architecture-decisions.md)
makes ADRs append-only — but a correction notice is added to its Status pointing here.
`engines/licenses.toml` records `linked = true` for ICU in **all three** artifacts, with
the `hb-icu` path and the symbol evidence.

The wasm module was re-audited specifically, because **it is the shipped artifact**: on
the web, `pdfium.wasm` is what a user downloads. Both HarfBuzz and ICU are present there
too.

### 4. Detect components we have not declared

Every finding in this ADR and in ADR 0008 came from a human running `nm` and noticing
something. That is not repeatable, and it is how ICU stayed mis-recorded through a whole
ADR cycle.

`tools/detect-engine-components.py` scans each engine artifact — native `.so` and `.wasm`
— for component fingerprints (`hb_*`, ICU's versioned symbols, FreeType, libjpeg, libpng,
zlib, AGG, libopenjpeg, lcms) and **fails CI when a component it detects is not declared
in `engines/licenses.toml`**. It runs alongside `check-engine-licences.py`, so the two
answer different questions:

| Tool | Question |
|---|---|
| `check-engine-licences.py` | Is every **declared** licence allowed? |
| `detect-engine-components.py` | Is every **detected** component declared? |

The second is the one that would have caught HarfBuzz on day one.

Its own test deliberately removes a declaration and asserts the detector fails, because a
detector that silently passes is worse than none — a lesson this project has already
learned twice.

## On acceptance — the exact change

Flipping this to Accepted requires **one edit**, so there is no ambiguity about what
"accepted" authorises. In `tools/check-engine-licences.py`, add two entries to `ALLOWED`:

```python
    # Added by ADR 0010, for components bundled inside PDFium.
    "MIT-Modern-Variant",   # HarfBuzz 14.3.1 -- verified against SPDX's canonical text
    "ICU",                  # ICU's legacy 1.8.1-57.1 section; X11-style
```

Nothing else. The manifest already records the determined facts, the licence texts are
already committed with provenance, and the detector already passes. After that edit
`tools/check-engine-licences.py` returns 0 and PR #14's licence gate goes green.

Until then it fails naming exactly these two identifiers — which is the intended
behaviour, not an oversight.

## Consequences

PDFium becomes licence-clean and PR #14 can go green once this is Accepted. The allowlist
grows by two: `MIT-Modern-Variant` and `ICU`, both permissive, both OSI-recognised forms,
neither threatening our `MIT OR Apache-2.0` licensing.

We now carry a licence notice for a component whose own artifact omits it. That is a
maintenance burden with a sharp edge: **the committed HarfBuzz text is pinned to
`886fc1e6`/14.3.1, and a PDFium bump can change the bundled HarfBuzz without any signal
from us.** The component detector catches an *undeclared* component; it does not catch a
declared one whose version moved. Re-deriving `harfbuzz_revision` from `DEPS` is part of a
PDFium bump from now on.

A third affirmative notice obligation joins FTL and IJG, and the count of surfaces that
must carry them is unchanged at four — of which only `THIRD_PARTY_NOTICES.md` exists.
That gap is now the largest single thing standing between us and a distributable build.

The M2 duplicate-HarfBuzz problem is recorded but unsolved, and it will cost something
whichever way it goes.

The detector will produce false positives — a string that looks like a fingerprint, a
symbol that survives dead-code elimination — and every one costs a human a few minutes.
That is the right trade against a missed component, but the fingerprints will need
maintenance and the tool should stay easy to read rather than clever.

Finally, the honest limit: `pdfium.wasm` has its names stripped, so detection there is
string-based and weaker than the native symbol table. HarfBuzz's presence in the wasm
rests on one string plus the inference from ICU (which arrives *through* HarfBuzz). If a
future build strips more aggressively, the wasm detector could go quiet while the
component is still present. Native detection is the stronger signal and should be treated
as authoritative for what PDFium contains.

## Alternatives considered

**Argue the ICU 1.8.1–57.1 section inapplicable at version 78.** Defensible on the facts
and it would keep the allowlist shorter by one. Rejected as a matter of process: it is an
exception reached by argument, in a policy that reserves exceptions to an ADR, to avoid
admitting a licence that passes the test anyway. The saving is not worth the precedent.

**Record HarfBuzz as plain `MIT`.** ADR 0004's own table already guessed "MIT (Old
Style)", and it is close enough to pass any casual review. Rejected: the text is a
distinct SPDX licence with distinct obligations, and the whole point of a manifest is that
it records what is true rather than what is close.

**Drop PDFium over the notice gap.** The gap is real — a shipped binary whose licence
notice is absent. Rejected for ADR 0008's reasons, which have not changed; and supplying
the missing notice ourselves is both cheap and exactly what the licence asks for.

**Only detect components on the native artifact.** Simpler, and the native symbol table is
far richer. Rejected because the wasm module is the artifact users actually download, so
leaving it unscanned would mean the shipped thing is the least examined thing.
