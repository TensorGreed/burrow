---
name: m1-qpdf-exception-boundary
description: qpdf's C API does NOT catch exceptions in every function — qpdf_is_linearized aborts the process; measured in the M1 PR 3 review (2026-09-10)
metadata:
  type: project
---

The premise that "every function in `qpdf-c.h` catches C++ exceptions internally" is false
for qpdf 12.4.1, and the exception is one burrow calls on every successful check.

**Why:** measured in the M1 PR 3 (`feat/qpdf-structure-engine`) review, 2026-09-10.
In `engines/vendor/src/qpdf-12.4.1/libqpdf/qpdf-c.cc`, only the functions that *return*
`QPDF_ERROR_CODE` go through the `trap_errors` helper (line ~69). `qpdf_is_linearized`
(line 382) and `qpdf_is_encrypted` (line 389) call straight into C++ with no try/catch.
`isEncrypted()` is a field read and cannot throw; `isLinearized()` runs `Lin::linearized()`,
which tokenises the first 1024 bytes and calls `toI(QUtil::string_to_ll(...))` on whatever
object number it finds — `std::range_error` for any object number above `INT_MAX`.

A 356-byte PDF that is otherwise completely valid, with `9999999999 0 obj\nnull\nendobj`
placed after the header, makes `Qpdf::check` die with
`fatal runtime error: Rust cannot catch foreign exceptions, aborting` (SIGABRT).
No `catch_unwind` and no `burrow-ffi::guard` can intercept it.

Two related facts from the same source read:
- `qpdflogger-c.cc` contains **zero** try/catch. The qpdf-c.h exception contract does not
  extend to the logger C API, which is a separate header and translation unit.
- `qpdf_cleanup` writes `WARNING: application did not handle error: <text with byte
  offsets>` to **`QPDFLogger::defaultLogger()`** — not the per-document logger — whenever
  the error slot is still full at cleanup. Demonstrated with a C harness using burrow's
  exact suppression setup: the message still reaches process stderr. burrow's discarding
  logger does not cover it; only draining the error before drop does.

**Closed in M1 PR 4b (2026-09-11):** `qpdf_is_linearized` and `qpdf_is_encrypted` are declared
in neither `qpdf/ffi.rs` nor `engines/build-wasm.sh`'s export list, and the abort input is now a
committed fixture, `tests/conformance/fixtures/object-number-above-int-max.pdf`. Re-measured:
PDFium returns `Ok(1)` and qpdf returns `Malformed`, no abort, on both paths.

**How to apply:** when reviewing any new qpdf call, check `libqpdf/qpdf-c.cc` for a
`trap_errors` wrapper around that specific function rather than trusting the header's
blanket statement. Anything unwrapped needs `extern "C-unwind"` plus a catch, or a
`trap_errors`-wrapped alternative. See [[m1-prescan-key-scan-bypass]] and [[user-role]].

**Mechanised in M1 PR 4c (2026-09-12): `tools/check-qpdf-trapped.py`.** Generates
`engines/qpdf-trapped-functions.txt` (22 names) from `libqpdf/qpdf-c.cc` and checks every
declaration against it or against `engines/qpdf-untrapped-accepted.toml` (14 entries). I
re-derived the 22 with an independent comment- and string-aware scanner: **byte-identical, no
false positives**, and all 14 exemption arguments check out against the C++ source.

Three limits of the mechanisation, all verified:
- The trapped set is scanned from **one file**. `qpdf_global_set_uint32` actually lives in
  `libqpdf/global.cc`, so it can never be recognised as trapped no matter what upstream does.
  Conservative direction (missing → must be exempted), but the coverage claim is narrower than
  the docstring says.
- `declared_functions()` enumerates **four hard-coded file paths**. `core/burrow-engines/src/
  link_check.rs` declares `qpdf_get_qpdf_version` and is invisible to it (test-only, benign).
- `brace_body()` is brace-counting with **no string/comment awareness**. qpdf-c.cc 12.4.1 has
  no brace inside a literal, so today it is correct — but `*p << "{";` appears in JSON.cc and
  QPDF_json.cc, and qpdf-c.cc has JSON entry points. A single such literal on a version bump
  swallows the next function's body and can list an untrapped function as trapped.

The real backstop on the web is `engines/build-wasm.sh`'s `EXPORTED_FUNCTIONS` allowlist,
derived from `ffi.rs` plus `{malloc, free, qpdf_get_qpdf_version}` and asserted against the
built module's export table. No `cwrap`/`ccall` is exported, so there is no dynamic call
surface past that allowlist.
