---
name: ci-rustdoc-gate
description: #174 check-rustdoc.sh and ci-local `paths_as` — what the doc gate actually binds, measured 2026-09-24
metadata:
  type: project
---

Measured on the #174 branch (de66574), 2026-09-24:

- Stale doc pages do NOT fool the page-existence check: rustdoc wipes `target/doc/<crate>/` when
  it re-documents a crate, so dropping `--all-features` from the build reads 0 of 5 and exits 1
  even over planted stale pages. Do not re-raise "stale target/doc" unless the build is skipped.
- `RUSTDOCFLAGS="-D warnings"` inside the script is the only thing that fails a broken intra-doc
  link; the self-test runs every case under `BURROW_DOC_DIR` (build skipped), so deleting the flag
  survives checker + self-test. Rustdoc's broken-link lint is warn-by-default (exit 0 measured).
- `paths_as` verification is a raw substring test over the wrapper script, comments included:
  replacing the build with `: # used to run: cargo doc ...` keeps the narrowing and the checker
  passes on whatever tree is in target/doc.
- Edits to the wrapper scripts themselves run `doc` only through the orphan fallback, not by design.

**How to apply:** when a later change touches this gate, check the flag and the build line are
tested by something that actually builds, not only by BURROW_DOC_DIR cases. Related: [[ci-local-preflight]].
