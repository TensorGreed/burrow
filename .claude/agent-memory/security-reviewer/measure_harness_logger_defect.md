---
name: measure-harness-logger-defect
description: The measure-* examples' silence() installs qpdf's DEFAULT logger, not a discarding one (dest 0 vs 3) and declares qpdflogger_set_* with 3 of its 4 parameters; measured 2026-09-15 against qpdflogger-c.h/.cc.
metadata:
  type: project
---

`core/burrow-engines/examples/measure-merge.rs` and `measure-compress.rs` each hand-declare the
qpdf C API rather than going through `qpdf/ffi.rs`, and both got the logger wrong in the same two
ways (compress copied merge):

- `qpdflogger_set_info/warn/error` take **four** parameters
  (`handle, enum qpdf_log_dest_e, qpdf_log_fn_t fn, void* udata` — `qpdflogger-c.h:70-77`); the
  examples declare three (`handle, c_int, *const c_char`).
- They pass `0` with a comment saying "0 is `qpdf_log_dest_discard`". `qpdf_log_dest_discard` is
  **3**; `0` is `qpdf_log_dest_default`. `set_log_dest` (`qpdflogger-c.cc:44`) maps default to
  `method(nullptr)`, and `QPDFLogger::setInfo(nullptr)` selects **stdout**, `setError(nullptr)`
  **stderr** (`QPDFLogger.cc:164-189`). So the "third suppression layer" in those harnesses is the
  default logger, and info would land on the harness's own report stream.

Production is correct: `codes/qpdf.rs` has `LOG_DEST_DISCARD = 3` and `qpdf/limits.rs` passes four
arguments. Only the examples are affected.

**A third harness was worse and was found by the same pass.** `measure-split.rs` installed **no
logger at all** — two suppression layers of three, so qpdf warnings carrying object numbers and
byte offsets reached stderr. All three examples are fixed (dest `3`, four arguments,
`measure-split` gaining the layer it never had); verified by running each and asserting stderr is
empty.

**Why:** `docs/spikes/0005`'s Finding 6 states its measurement was taken "with all three of
burrow's native suppression layers installed". Only two were live. The finding's *conclusion*
survives — `BaseHandle::warn` writes to `QPDFLogger::defaultLogger()`, which `qpdf_set_logger`
never touches — but the stated setup is not what ran.

**How to apply:** when reviewing anything that hand-declares qpdf entry points outside
`core/burrow-engines/src/qpdf/ffi.rs`, check it against the header by hand.
`tools/check-qpdf-trapped.py` reads only four declaration sites (native `ffi.rs`, the JS bridge,
`web/bridge.rs`, the wasm bridge) — `examples/` is outside every gate, so nothing else will catch
an arity or constant error there. Related: [[m1_qpdf_exception_boundary]].
