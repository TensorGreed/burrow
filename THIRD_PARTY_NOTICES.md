# Third-party notices

burrow is distributed under `MIT OR Apache-2.0`. It also incorporates third-party
software, listed here with its license. Every entry must be on the allowlist in
[ADR 0008](docs/adr/0008-widened-licence-allowlist.md), enforced by
[`deny.toml`](deny.toml) for Rust crates and by
[`engines/licenses.toml`](engines/licenses.toml) for native engines.

## Required credit lines

Two bundled components impose **affirmative notice obligations** on executable-only
distribution — which is what burrow ships. These lines are mandatory, not courtesy, and
must appear here, on the website credits page, and in the open-source licences screen of
each app:

> This software is based in part of the work of the FreeType Team.

> This software is based in part on the work of the Independent JPEG Group.

And HarfBuzz's copyright notice together with **both** of its disclaimer paragraphs,
reproduced in full from
[`docs/adr/licences/harfbuzz-14.3.1-COPYING.txt`](docs/adr/licences/harfbuzz-14.3.1-COPYING.txt)
— which we commit ourselves because **PDFium's package ships no HarfBuzz licence file at
all**.

The first is required by the FreeType Project License §2, the second by the Independent
JPEG Group license, LEGAL ISSUES condition (2), and the third by HarfBuzz's
`MIT-Modern-Variant` terms. FTL §3 additionally forbids using the FreeType name for
promotional purposes, and HarfBuzz's licence is notice-only with no such restriction.

**A release that omits these is a license violation, not an oversight.** They apply as
soon as a build containing PDFium is distributed; no such build exists yet.

This file is maintained by the `license-auditor` agent and must be updated in the same
pull request that adds or removes a dependency. It is a distribution requirement, not
bookkeeping: the MIT, BSD, and Apache-2.0 licenses all require that their notices travel
with the binary.

Regenerate the Rust portion with:

```bash
cargo deny list --layout crate --format json > /tmp/deny-list.json
```

## Rust dependencies

`zeroize` was the **first third-party crate that shipped in a binary**. It is no longer
the only one: `wasm-bindgen` and its runtime dependencies (`cfg-if`, `once_cell`,
`wasm-bindgen-shared`, and `unicode-ident` beneath it) are **normal** dependencies of
`burrow-wasm` and so are in the wasm module's link graph, subject to dead-code
elimination. Everything else in the table below is compile-time, build-script, or
test-only, as marked. Rows are marked individually; do not read the table as uniformly
compile-time.

| Crate | License | Used by | Notes |
|---|---|---|---|
| `thiserror` | MIT OR Apache-2.0 | `burrow-types` | Derive macro for error enums; compile-time only |
| `thiserror-impl` | MIT OR Apache-2.0 | `thiserror` | Proc-macro implementation |
| `syn`, `quote`, `proc-macro2` | MIT OR Apache-2.0 | `thiserror-impl`, `wasm-bindgen-macro`, `wasm-bindgen-macro-support` | Proc-macro support; compile-time only |
| `unicode-ident` | (MIT OR Apache-2.0) AND Unicode-3.0 | `proc-macro2`, `wasm-bindgen-shared` | Unicode identifier tables. Via `wasm-bindgen-shared` this is a **normal** dependency, not only a proc-macro one, so it is in the wasm link graph |
| `sha2` | MIT OR Apache-2.0 | `burrow-engines` | Verifies vendored engine checksums; **build/dev only** |
| `digest` | MIT OR Apache-2.0 | `sha2` | Digest traits; build/dev only |
| `block-buffer` | MIT OR Apache-2.0 | `digest` | Build/dev only |
| `crypto-common` | MIT OR Apache-2.0 | `digest` | Build/dev only |
| `generic-array` | MIT | `block-buffer` | **MIT only, not dual-licensed** |
| `typenum` | MIT OR Apache-2.0 | `generic-array` | Build/dev only |
| `version_check` | MIT OR Apache-2.0 | `generic-array` | Build/dev only |
| `cfg-if` | MIT OR Apache-2.0 | `cpufeatures`, `wasm-bindgen` | **No longer build/dev only**: `wasm-bindgen` uses it in `describe.rs` and `externref.rs`, so it is in the wasm module's link graph |
| `cpufeatures` | MIT OR Apache-2.0 | `sha2` | CPU feature detection; build/dev only |
| `libc` | MIT OR Apache-2.0 | `cpufeatures` | Build/dev only |
| `zeroize` | Apache-2.0 OR MIT | `burrow-types`, `burrow-engines` | **Runtime; ships in the binary.** Wipes password buffers on drop. `default-features = false`, `features = ["alloc"]` — no transitive dependencies |
| `wasm-bindgen` | MIT OR Apache-2.0 | `burrow-wasm` | **Runtime; ships in the wasm module.** The JS↔Rust boundary for the engine bridge. `default-features = false`, `features = ["std"]` — deliberately no `js-sys` and no `web-sys` |
| `wasm-bindgen-macro` | MIT OR Apache-2.0 | `wasm-bindgen` | Proc macro expanding `#[wasm_bindgen]`; compile-time only |
| `wasm-bindgen-macro-support` | MIT OR Apache-2.0 | `wasm-bindgen-macro` | Macro implementation; compile-time only |
| `wasm-bindgen-shared` | MIT OR Apache-2.0 | `wasm-bindgen`, `wasm-bindgen-macro-support` | Schema version and identifier validation shared by the crate and its macro. A **normal** dependency of `wasm-bindgen`, so it is in the wasm link graph, not compile-time only |
| `once_cell` | MIT OR Apache-2.0 | `wasm-bindgen` | **Runtime; in the wasm module's link graph.** `once_cell::unsync::Lazy` in `wasm-bindgen`'s `rt` module |
| `bumpalo` | MIT OR Apache-2.0 | `wasm-bindgen-macro-support` | Arena allocator used while encoding the macro's output (`encode.rs`); compile-time only |
| `rustversion` | MIT OR Apache-2.0 | `wasm-bindgen` | Build script detecting the compiler version; **build only** |

The `sha2` group is a **build- and dev-dependency of `burrow-engines` only**. It runs in
`build.rs` to re-verify the vendored engine libraries, and in the link tests. It ships in
no binary.

The `wasm-bindgen` group is a dependency of **`burrow-wasm` only**; it is absent from the
iOS, Android, and native graphs. Four of its seven crates (`wasm-bindgen-macro`,
`wasm-bindgen-macro-support`, `bumpalo`, `rustversion`) are proc-macro or build-script
crates that run on the host and are never codegen'd into the module. `js-sys` and
`web-sys` are **not** dependencies, by design: every bridge import in
`bindings/burrow-wasm/src/bridge.rs` takes and returns integers or byte slices, so
`deny.toml`'s `[[bans.features]]` ban on `web-sys`'s `RequestInit`, `WebSocket`,
`XmlHttpRequest` and `EventSource` has no crate to apply to here.

The Rust standard library is distributed under `MIT OR Apache-2.0`.

### Test-only dependencies

Dev-dependencies of `burrow-engines`. These build only under `cargo test`; none of them
ships in any distributed artifact. They are listed anyway because the licences still
apply to anyone redistributing the source tree.

**`cargo-deny` does not see these.** Verified against the repository's own `deny.toml` on
2026-09-10: with the workspace's dev-dependency graph, `cargo deny list` reports 23
crates and none of `proptest`, `serde`, `serde_json` or their transitives appear, with
`exclude-dev` either unset or explicitly `false`. The licences below were therefore read
by hand from each crate's manifest in `~/.cargo/registry`.

| Crate | License | Used by | Notes |
|---|---|---|---|
| `proptest` | MIT OR Apache-2.0 | `burrow-engines` | Property tests; **test only** |
| `bitflags` | MIT OR Apache-2.0 | `proptest` | Test only |
| `regex-syntax` | MIT OR Apache-2.0 | `proptest` | Test only |
| `unarray` | MIT OR Apache-2.0 | `proptest` | Test only |
| `num-traits` | MIT OR Apache-2.0 | `proptest` | Test only |
| `autocfg` | Apache-2.0 OR MIT | `num-traits` | Build script of a test-only crate |
| `rand` | MIT OR Apache-2.0 | `proptest` | Test only |
| `rand_chacha` | MIT OR Apache-2.0 | `rand` | Test only |
| `rand_core` | MIT OR Apache-2.0 | `rand` | Test only |
| `rand_xorshift` | MIT OR Apache-2.0 | `proptest` | Test only |
| `ppv-lite86` | MIT OR Apache-2.0 | `rand_chacha` | Test only |
| `zerocopy`, `zerocopy-derive` | BSD-2-Clause OR Apache-2.0 OR MIT | `ppv-lite86` | Test only; we take MIT or Apache-2.0 |
| `getrandom` | MIT OR Apache-2.0 | `rand_core` | Test only |
| `r-efi` | MIT OR Apache-2.0 OR LGPL-2.1-or-later | `getrandom` (UEFI target only) | **Disjunctive**: we take MIT. The LGPL arm is one option among three and is not exercised |
| `wasip2` | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | `getrandom` (`wasm32-wasip2` only) | Test only |
| `wit-bindgen` | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | `wasip2` | Test only |
| `serde` | MIT OR Apache-2.0 | `burrow-engines` | Reads `tests/conformance/expectations.json`; **test only** |
| `serde_core` | MIT OR Apache-2.0 | `serde` | Test only |
| `serde_derive` | MIT OR Apache-2.0 | `serde` | Proc macro; test only |
| `serde_json` | MIT OR Apache-2.0 | `burrow-engines` | Test only |
| `itoa` | MIT OR Apache-2.0 | `serde_json` | Test only |
| `memchr` | Unlicense OR MIT | `serde_json` | Test only |
| `zmij` | MIT | `serde_json` | Float formatting; **MIT only, not dual-licensed**; test only |
| `syn` 2.x | MIT OR Apache-2.0 | `serde_derive`, `zerocopy-derive` | Test only. A **second** major version of `syn` alongside the 3.x the error derives use |

### Fuzzing workspace (`fuzz/`)

`fuzz/` is a separate cargo workspace (`exclude = ["fuzz"]` in the root `Cargo.toml`) with
its own `fuzz/Cargo.lock`. Its binaries are never distributed — they are built by
`cargo +nightly fuzz run` and nothing else.

**`cargo-deny` does not cover this workspace.** The `deny` job in
`.github/workflows/ci.yml` invokes `cargo-deny-action` at the repository root with no
`manifest-path`, so `fuzz/Cargo.toml` is outside the graph it builds. Running the root
`deny.toml` against `fuzz/` by hand is what produced the table below.

| Crate | License | Used by | Notes |
|---|---|---|---|
| `libfuzzer-sys` | `(MIT OR Apache-2.0) AND NCSA` | `burrow-fuzz` | NCSA admitted by [ADR 0012](docs/adr/0012-ncsa-for-libfuzzer.md); see the note below. Fuzz harness only, never shipped |
| `arbitrary` | MIT OR Apache-2.0 | `libfuzzer-sys` | Fuzz only |
| `cc` | MIT OR Apache-2.0 | `libfuzzer-sys` | Compiles the vendored libFuzzer C++; build script, fuzz only |
| `jobserver` | MIT OR Apache-2.0 | `cc` | Fuzz only |
| `shlex` | MIT OR Apache-2.0 | `cc` | Fuzz only |
| `find-msvc-tools` | MIT OR Apache-2.0 | `cc` | Fuzz only |
| `getrandom` | MIT OR Apache-2.0 | `jobserver` | Fuzz only |
| `r-efi` | MIT OR Apache-2.0 OR LGPL-2.1-or-later | `getrandom` (UEFI target only) | Disjunctive; we take MIT |
| `libc` | MIT OR Apache-2.0 | `cc`, `getrandom` | Fuzz only |
| `zeroize` | Apache-2.0 OR MIT | `burrow-types`, `burrow-engines` | Same crate as above, resolved into this lock file too |

`libfuzzer-sys` vendors 55 C++ sources from LLVM's libFuzzer under
`libfuzzer-sys-0.4.13/libfuzzer/`, compiled into the fuzz binary by `cc`. The artifact
contradicts itself:

- Its `Cargo.toml` declares `license = "(MIT OR Apache-2.0) AND NCSA"`, and its `README.md`
  says "All files in the `libfuzzer` directory are licensed NCSA".
- Every one of those 55 files actually carries
  `SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception` — LLVM relicensed away from
  NCSA — and the crate ships **no** NCSA licence text at all, only `LICENSE-APACHE` and
  `LICENSE-MIT` for the Rust wrapper.

`Apache-2.0 WITH LLVM-exception` is allowed; NCSA was not. Resolved by
[ADR 0012](docs/adr/0012-ncsa-for-libfuzzer.md), which **admits NCSA to the allowlist** on
its own merits — it is a permissive BSD-family licence — rather than granting the crate a
`deny.toml` exception on the strength of our own reading of its source headers. The
discrepancy is recorded here so it is not rediscovered from scratch.

## Native engines

None linked yet. Planned for M1 onward, recorded in
[ADR 0004](docs/adr/0004-native-engines.md). Each must be license-verified before it is
vendored, and listed here with the exact version and source:

Audited for the first time by [spike 0001](docs/spikes/0001-wasm-engines.md). The full
component-level manifest, including everything these engines bundle, is
[`engines/licenses.toml`](engines/licenses.toml) — 20 components, 16 confirmed linked.
The table below is the summary; the manifest is authoritative.

Note that PDFium's package ships a top-level `LICENSE` that is the **packager's MIT
license**, not PDFium's BSD-3-Clause. Recording "MIT" from that file would misdescribe an
artifact containing nine other licenses.

> **Licence-clean, notices not yet on every required surface.** The component set is
> fully determined — `engines/licenses.toml`, 23 components, 18 confirmed linked, checked
> in CI by `tools/check-engine-licences.py` and `tools/detect-engine-components.py` — and
> every licence is on the allowlist as amended by
> [ADR 0010](docs/adr/0010-harfbuzz-and-icu-in-pdfium.md).
>
> What is still missing is **two of the four surfaces** ADR 0008 requires the credit lines
> to appear on. As of M1 PR 4c:
>
> - [x] This file.
> - [x] **The website credits page** — `/credits`, linked from the footer of every page,
>       generated at build time from `engines/licenses.toml` so it cannot drift from the CI
>       licence gate, and asserted against the built output by
>       `apps/web/src/credits.test.ts` (issue #16).
> - [ ] Android open-source licences screen (M3).
> - [ ] iOS open-source licences screen (M4).
>
> **No build containing these engines has been distributed**, so nothing is in violation
> today. That changes the moment one is, and two of four is still short of the bar.

| Engine | License (as audited) | Status |
|---|---|---|
| PDFium | BSD-3-Clause AND Apache-2.0, bundling **FTL**, **IJG**, **LicenseRef-AGG-2.3**, **libpng-2.0**, BSD-2-Clause, MIT, Zlib, Unicode-3.0, `Apache-2.0 WITH LLVM-exception` (Linux only) — plus **HarfBuzz (licence undetermined, no notice text shipped)** and **ICU (linked, contrary to ADR 0008)** | vendored, audit **blocked** |
| qpdf | Apache-2.0 (dual with Artistic-2.0 at our option; we take Apache-2.0) | vendored, built from source |
| zlib | Zlib | vendored 1.3.2, built from source |
| libjpeg-turbo | IJG AND BSD-3-Clause AND Zlib | vendored 3.2.0, built from source |
| HarfBuzz (`hb-subset`) | MIT (Old Style) | not yet vendored |
| mozjpeg | BSD-3-Clause / IJG | not yet vendored |
| libwebp | BSD-3-Clause | not yet vendored |
| libavif | BSD-2-Clause | not yet vendored |
| jbig2enc | Apache-2.0 — **verify, and verify Leptonica (BSD-2-Clause) too** | not yet vendored |

HarfBuzz, jbig2enc, mozjpeg, libwebp and libavif have **not** been audited. Expect at
least one to need another ADR; jbig2enc is the most likely.

## npm dependencies

None yet; the web app is scaffolded in this milestone. Astro, Svelte, and Vite are all
MIT.

## Fonts

Any bundled font must be OFL-1.1 or a permissive alternative. There are **two classes**, and
they carry different obligations:

**Distributed** — a font file the website ships. Its full licence text must sit alongside it.
`apps/web/fonts.toml` is the manifest for this class: where each face came from, pinned to an
upstream commit, what was done to it, and the digest of what ships.
`apps/web/src/fonts.test.ts` holds the shipped bytes to that record.

**Embedded** — a font that arrives *inside* a committed document, as a subset in a test
fixture's PDF. OFL-FAQ 1.10 makes embedding the one case in which an OFL font may travel
without the licence text, so no `OFL.txt` sits beside those files; the notice travels here
instead. There is no manifest and **no automated gate** for this class — see the Noto Serif
CJK entry below, and [#154](https://github.com/TensorGreed/burrow/issues/154), which exists
because the hand-maintained control has already missed one.

### Atkinson Hyperlegible Next 2.001 — OFL-1.1

Copyright 2020-2024 The Atkinson Hyperlegible Next Project Authors
(https://github.com/googlefonts/atkinson-hyperlegible-next)

Designed by the Braille Institute, Applied Design Works, Elliott Scott, Megan Eiswerth and
Letters From Sweden. Licensed under the SIL Open Font License, Version 1.1; the full text
ships at `apps/web/public/fonts/OFL.txt` and is served from the site, and the copyright,
license description and license URL are also embedded in the font file's own name table
(name IDs 0, 13 and 14), which is how a `.woff2` carries them once it is separated from the
directory it was downloaded in.

**Modified: subsetted.** The shipped file is the upstream variable font with its weight axis
clipped to 400-700 and its character set reduced to Latin, produced with fontTools 4.60.1
under `SOURCE_DATE_EPOCH=0` — `apps/web/fonts.toml` records the exact commands, and without
that variable they are not reproducible, because `varLib.instancer` stamps `head.modified`
with the current time. OFL section 3 forbids a Modified Version from using a Reserved Font
Name, and subsetting is modification; the upstream copyright line declares none, so the
family name is retained lawfully. **Re-check that on any version bump** — it is a one-line
change upstream and would otherwise pass unnoticed.

It is deliberately **not** listed in `engines/licenses.toml` and does not appear on the
website's `/credits` page. That page exists because FTL section 2, the IJG conditions and
MIT-Modern-Variant impose affirmative acknowledgement obligations binding "documentation
accompanying the distribution" (ADR 0008). OFL 1.1 imposes no such obligation, and adding a
font to a page whose reason for existing is a different clause would blur why that page is
mandatory. `apps/web/fonts.toml` gives the full reasoning.

### Noto Serif CJK SC 2.002, six-glyph subset embedded in a test fixture — OFL-1.1

Copyright © 2017-2023 Adobe (https://www.adobe.com/). Trademarks held by Google.
Licensed under the SIL Open Font License, Version 1.1:
https://github.com/notofonts/noto-cjk/blob/main/Serif/LICENSE

**Not shipped in any burrow binary, and not a dependency.** A six-glyph Type 1 subset
(`/FontFile`, `/BAAAAA+NotoSerifCJKsc-Regular`) embedded by LibreOffice in
`tests/redaction/fixtures/producer-vertical-writing.pdf`, a real-producer fixture for
vertical (`tb-rl`) text. It is test data: no build step reads it, nothing links it, and it
is absent from every artifact we distribute.

Source: `fonts-noto-cjk` `1:20230817+repack1-3`, supplying
`/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc` at version 2.002. Established from
the installed font's own `name` table (ID 0 the copyright, IDs 13/14 the licence) and
verified against upstream `notofonts/noto-cjk` `Serif/LICENSE`, not only against package
metadata. The Debian `copyright` file records `Files: *` as SIL-1.1; its GPL-3+ stanza covers
`debian/*` packaging scripts only and none of that reaches the PDF. Upstream's
`Serif/README-third_party.md` still calls the fonts Apache-2.0 in its prose while its own
header says OFL-1.1 — both are on the allowlist, so the stale sentence changes nothing, but
cite `Serif/LICENSE`.

**Do not read the version off the PDF.** Its Type 1 header says `NotoSerifCJKsc-Regular
001.003`, which is LibreOffice's conversion stamp and not a Noto CJK release number. That is
benign here only because every Noto CJK release is allowlisted — ≥1.002 is OFL-1.1, earlier
was Apache-2.0 — and would not be benign for a family that changed to a forbidden licence at
some version. The provenance pins the *system* font by path and version for that reason.

**No Reserved Font Name is engaged.** OFL 1.1 defines an RFN as a name "specified as such
after the copyright statement(s)", and neither upstream's LICENSE nor the font's own name ID
0 specifies one — Adobe reserves "Source" for Source Han Serif; Google's Noto re-release
deliberately reserves nothing. Clause 3 is therefore inert for both the `/BaseFont` subset
tag and the internal `/FontName`, which still reads `NotoSerifCJKsc-Regular`. **Re-check on
any version bump**: declaring an RFN is a one-line upstream change and would otherwise pass
unnoticed. Same standing obligation as the Atkinson entry.

**The OFL text is not required to travel with it, and does not.** OFL-FAQ 1.10: the one case
in which an OFL font may be distributed without the licence text is when it is embedded in a
document. FAQ 1.11-1.12 place a format-converted six-glyph subset inside a PDF firmly in
"embedding" — our artifact satisfies every clause of that description at once, since the
format was altered (OpenType/CFF → Type 1), six glyphs are present, and the names are
`cid28987`-style CID references. FAQ 1.13 confirms the document's own licence is unaffected,
so the fixture stays `MIT OR Apache-2.0`.

The distinction that would bite next time: FAQ 1.15 makes dropping an **unmodified** font
file into a container *bundling*, not embedding, and bundling does carry clause 2's
notice-and-licence requirement. Committing a `.ttf` or `.otf` to this repository would
engage that. A PDF with a six-glyph Type 1 subset inside it does not.

**One residual, recorded rather than smoothed over.** LibreOffice's OpenType-to-Type 1
conversion discarded the font's `name` table, so the embedded program carries **no copyright
notice and no licence reference** — verified by string search and by decrypting the eexec
portion, which holds only `/Private`, `/Subrs`, `/CharStrings` and six `/cid*` glyphs. A
literal reading of OFL clause 2 ("each copy contains the above copyright notice and this
license") is therefore not satisfied by the bytes; the position rests on SIL's own published
interpretation that clause 2 governs distribution rather than embedding. That reading is
sound, and this entry is where the notice travels instead, so the obligation is discharged
either way.

Deliberately **not** in `engines/licenses.toml` and not on the website `/credits` page: it is
not an engine, has no vendor tree and no symbols, and OFL 1.1 imposes no affirmative
acknowledgement obligation of the kind that makes that page mandatory for FTL, IJG and
MIT-Modern-Variant.

### Liberation Serif 2.1.5 and Liberation Sans 2.1.5, subsets embedded in a test fixture — OFL-1.1

Digitized data copyright © 2010 Google Corporation. Copyright © 2012 Red Hat, Inc.
Ascender Corporation. Licensed under the SIL Open Font License, Version 1.1:
http://scripts.sil.org/OFL

**Not shipped in any burrow binary, and not a dependency.** `/FontFile2` TrueType subsets
(`/BAAAAA+LiberationSans-Bold`, `/CAAAAA+LiberationSerif`) embedded by LibreOffice in
`tests/redaction/fixtures/producer-writer.pdf`. Test data, as above.

**This entry exists because the record was wrong.** `tests/redaction/PROVENANCE.md` stated
that no third-party font was embedded in those fixtures and that "the fonts are the
producers' own bundled faces". Liberation is Ascender, Red Hat and Google — a face LibreOffice
*bundles* is not a face LibreOffice *wrote*. Read out of the embedded `name` tables, not
inferred. Both provenance and this file are corrected, and
[#154](https://github.com/TensorGreed/burrow/issues/154) is the control.

**Clean, and clean only by the version.** 2.1.5 is OFL-1.1, and these subsets retain name IDs
0, 13 and 14, so OFL clause 2 is satisfied literally — unlike the Noto Type 1 subset above.
**Liberation 1.x was GPLv2 with a font exception**, which this repository's allowlist forbids,
so the version is load-bearing rather than incidental.

### `GlyphLessFont` 1.0, embedded in a test fixture — Apache-2.0

Copyright © 2020 Google Inc. Licensed under the Apache License, Version 2.0:
https://www.apache.org/licenses/LICENSE-2.0

**Not shipped in any burrow binary, and not a dependency.** A 572-byte `/FontFile2`
embedded by tesseract in `tests/redaction/fixtures/producer-ocr-scan.pdf`: the invisible
face an OCR layer draws its recognised text with. Test data.

**Established by byte identity against a version-pinned source, because the font itself
says nothing.** Its `name` table carries no copyright string and no licence string — read,
not assumed — so the file cannot answer for itself and an earlier version of this entry
recorded it as *undetermined*. ADR 0008 forbids anything unclear, so undetermined could not
stand on a committed fixture. The route that resolved it:

1. `tesseract --version` on the producing host reports **5.3.4**, which is the version
   `tests/redaction/PROVENANCE.md` records for `producer-ocr-scan.pdf`.
2. Tesseract does not ship the font as a file; it generates it from
   `src/api/pdf_ttf.h`, a `static const uint8_t pdf_ttf[]` array produced by
   `bin2cpp pdf.ttf pdf_ttf cpp17`. Fetched at tag **5.3.4**, that file's header reads
   `(C) Copyright 2020, Google Inc.` and `Licensed under the Apache License, Version 2.0`.
3. The array decodes to **573 bytes**, sha256
   `dbbbba44717f3c6dfdb4ab8dd5d231ba16ef002d75cedb820824ce6063c0a5ee`. The fixture's
   embedded font is **572 bytes**, sha256
   `c7845420925a23d88ed830a63957b8af85a66a8daf8d9fc90e843673b2ef1a59`, and the two are
   **byte-for-byte identical over all 572**. The extra byte is the array's trailing `0x00`,
   `bin2cpp`'s NUL terminator, which is not part of the font.
4. Tesseract's top-level `LICENSE` at tag 5.3.4 is the Apache License 2.0, and the Debian
   `tesseract-ocr` `5.3.4-1build5` copyright file records `Files: *` as Apache-2.0.

So the bytes in the fixture are the bytes tesseract distributes under Apache-2.0, and the
claim rests on a hash comparison rather than on the package's word or the font's silence.

**Both Apache-2.0 obligations are already discharged.** §4(a) requires recipients of the
Work to receive a copy of the licence: burrow carries `LICENSE-APACHE` at its root, since
the project is itself `MIT OR Apache-2.0`. §4(d) requires carrying a `NOTICE` file's
contents where one exists — **tesseract 5.3.4 has no `NOTICE` file** (checked: 404 at that
tag, and none under any of the usual spellings), so nothing is owed under it. No further
file is added beside the fixture.

**Re-check if the fixture is regenerated against a different tesseract.** The verification
above is pinned to 5.3.4; a newer release could change the font bytes, and the hash
comparison is what would catch it.
