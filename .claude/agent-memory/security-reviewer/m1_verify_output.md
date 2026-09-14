---
name: m1-verify-output
description: ADR 0022's read-back verification (branch m1-verify-output, 2026-09-14) — the WebQpdf::fresh logger leak, the two page sweeps that sit outside the deadline, max_input_bytes applied to the output, and how to build adversarial PDFs qpdf will actually open.
metadata:
  type: project
---

Measured 2026-09-14 on branch `m1-verify-output` (uncommitted), `core/burrow-ops/src/verify.rs`
plus the `OutputReader` impls.

**`WebQpdf::fresh()` calls `Self::new(Arc::clone(bridge))`, which makes a NEW
`installed: Arc<OnceLock>`.** `qpdf.rs:256` (`install()`) then runs again on the witness's first
open and calls `logger_create()` a second time — the exact per-operation leak the `installed`
field's own rustdoc (qpdf.rs:39-51) was added to stop, since `qpdflogger_cleanup` is declared on
neither path. `bindings/burrow-wasm/src/lib.rs:91` deliberately *clones* a thread-local `QPDF`
for the same reason. Fix is `self.clone()` — `WebQpdf` derives `Clone` and the fresh `qpdf_data`
comes from `open`, not from the engine value. The fake's `retained()` detector
(`web/fake.rs:723`, asserted in `web/tests.rs:788` and `:1243`) would catch it instantly, but
**no web test drives an operation through `burrow_ops`**, so it is structurally blind to it.

**Two per-page sweeps were added with no `Deadline::checkpoint` in either** — the promise sweep
in `ops::rotate`/`ops::reorder` and `OutputReader::rotations` inside `verify::output`, which has
no deadline at all. Measured, 10,000 pages (the default `max_pages`) at page-tree depth 60,
1.1 MB: edit+write **11.9 ms**, promise sweep **143.9 ms**, verification **173.7 ms**. The work
the cooperative limit actually covers is ~4% of the total. ADR 0022's own cost table says
"1.7x", measured only on 10- and 137-page fixtures where the write dominates.

**`open_output` re-applies the operation's `Limits` to the OUTPUT.** Reproduced: two 16,204-byte
inputs with `max_input_bytes = 32,408` pass `check_total_input_bytes` exactly, qpdf emits 32,995
bytes, and the merge dies as
`OutputRejected("merge: burrow produced a document it cannot read back (LimitExceeded …)")`.
Rotate's output is also bigger than its input (1,168,116 vs 1,168,105 on a merged fixture).

**Verification adds exactly one full copy of the output to peak memory** (`bytes.to_vec()` in
both `open_output`s). Measured on a 200 MB output: RSS 197 MB → 387 MB. It runs while the input
document is still alive (`source` lives to the end of the op fn), so peak is input + output +
copy. qpdf is lazy, so the read-back time is driven by pages x tree depth, not by stream bytes
(135 ms for a 200 MB output; 173 ms for a 1.1 MB one with 10,000 deep pages).

**`core/burrow-engines/tests/properties.rs` is PDFium-only** (`tests/support/mod.rs` opens via
`Pdfium`), and `#![cfg(all(feature = "native-engines", …, target_os = "linux"))]`. `verify.rs`
cites it as the guarantee that the qpdf errors it embeds via `{error:?}` are fixed constants;
it covers neither qpdf nor the web bridge. Its `check_outcome` also waves `LimitExceeded`
through without looking at `requested`, which `{error:?}` does print.

**Building adversarial PDFs qpdf will open, on this repo's settings:** every page needs
`/Resources << >> /Contents []` or qpdf trips `DOC_MAX_WARNINGS`
(`codes/qpdf.rs::policy::settings`) somewhere between 100 and 1,000 pages and `open` returns
`Malformed("qpdf: the document is damaged")` — which looks like a broken generator and is not.
A 60-deep `/Pages` chain over N leaves opens fine (`MAX_PAGE_TREE_DEPTH` is 64). To run anything
linked against the native engines on this machine:
`LD_LIBRARY_PATH=engines/vendor/native-aarch64/lib` (the host is aarch64; the x64 tarball is
also in the vendor tree and is not the one that gets built). A probe crate under the scratchpad
with `path` deps into `core/` builds and links fine.

**`/Rotate 45` now denies the whole operation.** Reproduced: the engine seam reorders such a
document happily, `burrow_ops::reorder` returns `Malformed("/Rotate is not a multiple of 90")`
because the promise sweep walks every page. Indirect `/Rotate 6 0 R` resolves correctly and is
unaffected.

See [[m1-limits-real-strength]] for what `max_duration_ms`/`max_memory_bytes` were already worth,
[[m1-web-reorder-bridge]] for the handle discipline the new `rotations()` copies correctly.
