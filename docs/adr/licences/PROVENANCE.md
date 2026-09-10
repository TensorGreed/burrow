# Provenance of the licence texts in this directory

These files exist because the artifact that ships the code **does not carry its own
licence text**. Where a component's notice travels with it, we do not duplicate it here.

Each entry records where the text came from and how to re-fetch it, so a reviewer can
verify it rather than trust it.

## `LicenseRef-AGG-2.3.txt`

Anti-Grain Geometry 2.3, bundled in PDFium as `third_party/agg23`.

| | |
|---|---|
| Source | `licenses/agg23.txt` inside `pdfium-wasm.tgz` / `pdfium-linux-*.tgz` at release `chromium/8044` |
| Why here | No SPDX identifier exists for this grant, so ADR 0008 names it `LicenseRef-AGG-2.3` and commits the text it refers to |
| Version note | **2.3 specifically.** AGG 2.4 and later were relicensed to the GPL, so the version pin is load-bearing |

## `harfbuzz-14.3.1-COPYING.txt`

HarfBuzz, bundled in PDFium as `third_party/harfbuzz`.

| | |
|---|---|
| Version | **14.3.1** (`meson.build` `version:` at the pinned commit) |
| Commit | `886fc1e645388080b72f6d9b06347533a0018045` |
| Pinned by | `harfbuzz_revision` in PDFium's `DEPS` on branch `chromium/8044` |
| Fetched from | `https://chromium.googlesource.com/external/github.com/harfbuzz/harfbuzz/+/886fc1e645388080b72f6d9b06347533a0018045/COPYING?format=TEXT` |
| sha256 | `ba8f810f2455c2f08e2d56bb49b72f37fcf68f1f4fade38977cfd7372050ad64` |
| SPDX | `MIT-Modern-Variant` — verified byte-for-byte against SPDX's canonical `licenseText` from the grant clause onward |
| **Why here** | **pdfium-binaries ships no harfbuzz licence file at all.** Upstream's `steps/08-licenses.sh` derives its licence set from `build.ninja` and misses it, so the notice required by the licence does not travel with the artifact. See ADR 0010 |

Re-fetch and verify:

```bash
curl -sSL 'https://chromium.googlesource.com/external/github.com/harfbuzz/harfbuzz/+/886fc1e645388080b72f6d9b06347533a0018045/COPYING?format=TEXT' \
  | base64 -d | sha256sum
# ba8f810f2455c2f08e2d56bb49b72f37fcf68f1f4fade38977cfd7372050ad64
```

## `harfbuzz-14.3.1-src-ms-use-COPYING.txt`

`src/ms-use/` inside HarfBuzz — the Microsoft Universal Shaping Engine tables. HarfBuzz's
top-level `COPYING` points at per-subdirectory `COPYING` files, and this is the only one
outside `test/`.

| | |
|---|---|
| Fetched from | `.../886fc1e6.../src/ms-use/COPYING?format=TEXT` (same commit) |
| sha256 | `c2cfccb812fe482101a8f04597dfc5a9991a6b2748266c47ac91b6a5aae15383` |
| SPDX | `MIT` (Microsoft Corporation) — already on the allowlist |
| Linked? | **No.** Zero `use_machine` symbols in `libpdfium.so`; recorded because it is distributed inside the HarfBuzz tree, not because it is linked |
