---
name: m1-limits-real-strength
description: What burrow's Limits actually bound at the PDFium boundary, and the measured gaps found in the M1 PR 2 review (2026-09-10)
metadata:
  type: project
---

`max_memory_bytes` on native is a pure function of **input length** (`estimate.rs`:
`len + len/4 + 16 MB`). It therefore cannot see any quantity declared *inside* the file,
which is exactly the class its own docs claim it catches.

**Why:** measured in the M1 PR 2 review (2026-09-10). A 330,832-byte PDF whose
`/Type /XRef` stream declares `/Size 20000000` with `/W [1 8 8]` (a ~340 MB table that
zlib-compresses to ~330 kB, since the padding entries are zeros) makes PDFium allocate
**1,222 MB**. PDFium appears to sanity-reject `/Size` somewhere between 20M and 40M, so
~1.2 GB is near the peak single-file amplification.

**Partly fixed in the same PR, and it matters which part.** `open` used to return `Ok(1)`
under `Limits::DEFAULT`; it now returns `LimitExceeded` on `max_memory_bytes`, because
`pdfium::estimate::check_measured_memory` compares `/proc/self/statm` before and after the
load and refuses to hand back a document that already cost more than the caller allowed.
The regression test is `tests/limits.rs::a_declared_size_bomb_is_rejected_rather_than_returned`,
against the committed `tests/conformance/fixtures/xref-bomb.pdf`.

Note the noise margin is **absolute** (64 MiB), not proportional. A 2x margin was tried
first and let this bomb straight through, because 1.2 GB is only 1.2x over a 1 GiB ceiling.

**What is still true:** the allocation happens first — the check is after the fact. With
the address space capped at 300 MB the process still dies with a **silent SIGABRT** (no
message, core dumped) from inside PDFium's OOM handler, which no `catch_unwind` or
`burrow-ffi::guard` can intercept. The size-based pre-check remains blind to declared
quantities and always will be; the real fix is a structural pre-scan before the load,
recorded against M1 PR 3 (qpdf) in `ROADMAP` and ADR 0011.

Also measured, and **not** fixed: the single engine thread means `submit`'s
`reply_rx.recv()` has no timeout, so one slow file blocks unrelated operations — a 32 MB
xref-rebuild file made an unrelated 200-byte open with a 1 ms budget wait 716 ms and then
fail with `LimitExceeded`. Cross-thread denial of service, and the victim is told its own
budget expired. Recorded in ADR 0011's consequences, along with the missing registry-level
cap on concurrently open documents.

Reproduction lives outside the repo (scratchpad crate depending on
`core/burrow-engines` by path with `features = ["native-engines"]`, run with
`LD_LIBRARY_PATH=engines/vendor/native-$(uname -m)/lib`). The fuzz target cannot find
this: libFuzzer's default `-max_len` is 4096 and the corpus starts empty, so a 330 kB
input containing a valid deflate stream is unreachable.

**How to apply:** when reviewing any later engine op (render, redact, M1 PR 4's wasm
engine), treat `max_memory_bytes` on native as "you will be told afterwards", never as a
cap. Ask what the operation
reads from the file that multiplies cost, and whether the estimate is fed that value or
only the byte count. See [[m1-engine-supply-chain]] and [[user-role]].

**Addendum, M1 PR 4c review (2026-09-12): one real in-operation bound does exist, and the
4c docs erase it.** `core/burrow-engines/src/codes/qpdf.rs::policy::settings()` sets five
qpdf global decompression ceilings to 256 MiB each (`flate`/`dct`/`png`/`run_length`/`tiff`
`_max_memory`, all of which default to *unlimited*) plus `parser_max_nesting = 64` and
`doc_max_warnings = 256`. Applied on BOTH paths — native `qpdf/limits.rs::apply_global_limits`
behind a `OnceLock`, web `web/qpdf.rs::install()` behind a `OnceLock` — and enforced by qpdf
*during* the operation, i.e. they bound an allocation rather than detect it. They are
qpdf-only (nothing equivalent for PDFium) and are not derived from `max_memory_bytes`.

So the honest sentence is: *nothing derived from `max_memory_bytes` bounds anything; qpdf's
own filter ceilings do, at a fixed 256 MiB, on both platforms.* ADR 0007's 2026-09-12
amendment writes "anything that actually bounds an allocation | none | the 2 GiB module
maximum", which is an underclaim in both cells.
