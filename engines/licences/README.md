# Committed licence texts for the bundled engine components

The verbatim licence text of every component in [`../licenses.toml`](../licenses.toml)
that is `linked = true` — confirmed present in a shipped artifact by symbol inspection.

## Why these are committed, when the originals are already on disk

Each component's `license_file` in the manifest points at the audited original, which lives
under `engines/vendor/`. **`engines/vendor/` is gitignored**, reproducible from
`engines/pins.toml` via `engines/fetch.sh`. That is the right home for an audit trail and the
wrong one for something a *build* depends on:

- The website credits page is generated from this manifest at build time, so it must build
  from a clean checkout with no vendor tree.
- CI's `web` job stages only the **wasm** vendor prefix, while most `license_file` paths name
  `vendor/native-aarch64/licenses-pdfium/`. Those files are simply not there when the site is
  built.
- [ADR 0008](../../docs/adr/0008-widened-licence-allowlist.md)'s FTL and IJG obligations bind
  *documentation accompanying the distribution*. Text that exists only in a directory nobody
  ships does not discharge them.

So the manifest carries two paths per component: `license_file`, the audited original (audit
provenance), and `license_text`, the committed copy here (what ships).

## Keeping them honest

`tools/check-engine-licences.py` fails CI if a `linked` component declares no `license_text`,
or if the file is missing. **When the vendor tree is present it also compares the two byte for
byte**, so a copy that drifts from the original it was taken from is a build failure rather
than a silent divergence — which is the whole failure mode committing a copy introduces.

Two files are exempt from the byte comparison, for stated reasons:

- `agg23` and `harfbuzz` reference [`docs/adr/licences/`](../../docs/adr/licences/) directly.
  Those two are committed **because they exist nowhere else** — AGG has no SPDX identifier and
  PDFium's package ships no HarfBuzz licence file at all — so there is no upstream copy to
  compare against. They are not duplicated here.
- `emscripten.txt`: its `license_file` is `emsdk:upstream/emscripten/LICENSE`, a pseudo-path
  into the pinned SDK install rather than into `engines/vendor/`, so there is no vendor path
  to resolve. The SDK is version-pinned in `engines/pins.toml` and verifies its own downloads.

## Filenames

Named after the component, not after the upstream file, because two components can ship a file
of the same name. Where one library appears twice — once bundled inside PDFium and once
vendored by us at a different version — the PDFium copy carries a `-pdfium` suffix:
`zlib.txt` / `zlib-pdfium.txt`, `libjpeg-turbo.md` / `libjpeg-turbo-pdfium.ijg`. They are
genuinely different texts at different versions; do not deduplicate them.

## Adding one

Do not add a file here on its own. Add the component to `engines/licenses.toml` with both
paths, copy the text from the vendor tree, and run `python3 tools/check-engine-licences.py`.
A licence not on [ADR 0008](../../docs/adr/0008-widened-licence-allowlist.md)'s allowlist needs
a new ADR, not an entry here.
