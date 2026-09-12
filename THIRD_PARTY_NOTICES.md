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

None bundled yet. Any bundled font must be OFL-1.1 or a permissive alternative, with its
full license text included alongside the font file.
