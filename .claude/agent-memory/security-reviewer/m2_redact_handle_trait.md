---
name: m2-redact-handle-trait
description: #191 step 1 (policy moved onto redact/graph.rs traits) — what the outcomes golden catches and misses, and where the GAT's borrow guarantee really lives
metadata:
  type: project
---

Reviewed 2026-09-25 at 9757bc8. The move had no leak: every drain maps 1:1, and the base wasm is byte-identical to main's (sha256, measured).

- The outcomes golden (`core/burrow-ops/tests/redaction_outcomes.rs`, 463 cases) caught 6 of 8 planted mutations: annots erase, /Differences set, /ToUnicode replace, page-key strip, /Widths set, same_document inverted. **Drain deletions survive the golden and the full engines+ops suites** (1107 tests): `PdfDocument::page`'s `drained()`, which now stands in for four former drain sites, and the witness `mapped_codes` drain. No fixture latches an error at those points.
- The GAT `type Object<'a> where Self: 'a` stops **generic** code from outliving the document (E0505, measured). It does **not** make an implementation borrow: `type Object<'a> = Owned` compiles. So #147(a) holds per impl, and the web half needs its own compile_fail test.
- In the golden, `make-redaction-fixtures.py:647` uses Python's system zlib. A CI fixture hash could differ from the aarch64 bless. That fails closed, by name.
- A symlinked `engines/vendor` in a worktree shows as untracked, because the gitignore rule has a trailing slash.

**How to apply:** on the web half of #191, look for the per-impl borrow test and any drain-witnessing fixture. Related: [[m2-web-handle-newtype]], [[m2-qpdf-redact-steps]].
