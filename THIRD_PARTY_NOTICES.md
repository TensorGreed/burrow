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

And, once ADR 0010 is accepted, HarfBuzz's copyright notice together with **both** of its
disclaimer paragraphs, reproduced in full from
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

Only build-time and transitive dependencies so far; no runtime third-party code ships in
the current (M0) build beyond the Rust standard library.

| Crate | License | Used by | Notes |
|---|---|---|---|
| `thiserror` | MIT OR Apache-2.0 | `burrow-types` | Derive macro for error enums; compile-time only |
| `thiserror-impl` | MIT OR Apache-2.0 | `thiserror` | Proc-macro implementation |
| `syn`, `quote`, `proc-macro2` | MIT OR Apache-2.0 | `thiserror-impl` | Proc-macro support; compile-time only |
| `unicode-ident` | (MIT OR Apache-2.0) AND Unicode-3.0 | `proc-macro2` | Unicode identifier tables |
| `sha2` | MIT OR Apache-2.0 | `burrow-engines` | Verifies vendored engine checksums; **build/dev only** |
| `digest` | MIT OR Apache-2.0 | `sha2` | Digest traits; build/dev only |
| `block-buffer` | MIT OR Apache-2.0 | `digest` | Build/dev only |
| `crypto-common` | MIT OR Apache-2.0 | `digest` | Build/dev only |
| `generic-array` | MIT | `block-buffer` | **MIT only, not dual-licensed** |
| `typenum` | MIT OR Apache-2.0 | `generic-array` | Build/dev only |
| `version_check` | MIT OR Apache-2.0 | `generic-array` | Build/dev only |
| `cfg-if` | MIT OR Apache-2.0 | `cpufeatures` | Build/dev only |
| `cpufeatures` | MIT OR Apache-2.0 | `sha2` | CPU feature detection; build/dev only |
| `libc` | MIT OR Apache-2.0 | `cpufeatures` | Build/dev only |

The `sha2` group is a **build- and dev-dependency of `burrow-engines` only**. It runs in
`build.rs` to re-verify the vendored engine libraries, and in the link tests. It ships in
no binary.

The Rust standard library is distributed under `MIT OR Apache-2.0`.

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

> **This section is blocked pending [ADR 0010](docs/adr/0010-harfbuzz-and-icu-in-pdfium.md).**
> The component set is now fully determined — see `engines/licenses.toml`, 23 components,
> 18 confirmed linked — but two of their licences (`MIT-Modern-Variant` for HarfBuzz,
> `ICU` for ICU's legacy section) are not yet on the allowlist. ADR 0010 proposes
> admitting both; `tools/check-engine-licences.py` fails until it is Accepted.
>
> **No build containing these engines has been distributed**, so no notice obligation is
> currently unmet. That changes the moment one is.

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

None bundled yet. Any bundled font must be OFL-1.1 or a permissive alternative, with its
full license text included alongside the font file.
