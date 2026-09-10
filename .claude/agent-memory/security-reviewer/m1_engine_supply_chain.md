---
name: m1-engine-supply-chain
description: What the engine-acquisition checksum/provenance guarantee actually covers, and the gaps found in M1 PR 1 review (2026-09-10)
metadata:
  type: project
---

The engine supply-chain guarantee is narrower than the docs in `engines/pins.toml` and
`core/burrow-engines/build.rs` imply, and the boundary matters for reviewing M1 PR 2+.

**Why:** M1 PR 1 (engine acquisition, reviewed 2026-09-10) is where the user placed the
load-bearing checksum and provenance controls. `libpdfium.so` is the *only* file covered by
a `pins.toml` hash at build time. `libqpdf.a`, `libz.a`, `libjpeg.a` are built locally and
have no pins at all — build.rs only checks they exist — and the extracted source trees under
`engines/vendor/src/` are never verified, only the tarballs are. On a CI `engines-cache`
hit, `build-native.sh` never runs, so those archives are linked straight from the GitHub
Actions cache with no integrity check.

Also decided in PR 1: PDFium links **dynamically** (no static archive is published), so
provenance depends on rpath. Cargo does not propagate `rustc-link-arg`, so downstream
binaries (`burrow-core`, `burrow-ffi`) get `DT_NEEDED libpdfium.so` from the propagated
`-lpdfium` with no rpath. The chosen fix is `$ORIGIN` in `.cargo/config.toml` plus a
verified copy placed beside the test binaries by build.rs, with the provenance test asserting
**sha256 of the mapped file rather than its path**. `$ORIGIN` sorts first in DT_RPATH, so the
copy under `target/` — not the vendor file — is what actually loads.

**How to apply:** When reviewing anything that links or loads an engine, check (1) whether
the artifact in question is actually hash-covered or merely present, (2) whether the binary
doing the loading has an rpath at all — `readelf -d` the built test binary, don't infer, and
(3) whether the provenance assertion runs in the binary that loads the library, since the
existing one only lives in `burrow-engines`' own test binary. `RUSTFLAGS` set anywhere
(ci.yml sets it globally) discards `.cargo/config.toml` rustflags entirely.

See [[user-role]].
