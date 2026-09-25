---
name: m2-web-redaction-impl
description: #191 web half (web/redact.rs over the JS bridge, af2812a) — the copy_in latch measured fail-closed over 8840 injected failures, why that sweep cannot isolate the latch, and the web open's untested ceilings
metadata:
  type: project
---

Reviewed 2026-09-25 at af2812a (branch feat/redaction-on-the-web, stacked on #198). No leak found.

- **copy_in failure sweep.** A NativeBridge wrapper failing the Nth `copy_in` (single-shot and sticky) over all 212 OK cases in outcomes.tsv: 8840/8840 runs Err, 0 Ok. **But the same sweep stays 8840/8840 Err with the latch deleted.** qpdf latches its own error when handle 0 (`nothing()`) reaches the C API, so the Rust latch is the second of two mechanisms. It stands alone only where no qpdf call follows the failed copy: `remove_key`, whose drain is right after it. `a_key_the_engine_heap_refuses...` is what catches the latch deletion.
- `write()` checks `session.take_error()` and never the latch. That is sound only because every step ends in a drain.
- Mutations that survive the whole engines lib suite (591 tests): the web open's `max_pages` check, the first `deadline.checkpoint`, and the drains in `page`, `page_content`, `stream_data` and `object`. The native half got drain tests after #198's review; the web half has no twin. NativeBridge `heap_bytes` is 0, so `check_measured_memory` cannot be observed.
- WebObject borrows its document (E0597 measured). No compile_fail test pins it. Cross-document `&Self` pairs compile and are refused at runtime (same_session) in the two writes.
- Base wasm (raw cargo release) grew +1,902 B with 0 of 8 needles. The only new strings are the import names, plus one mangled `RawVec<Pending<WebObject>>::grow_one` in the name section, which is a merged-function alias.

**How to apply:** when #137 wires a shipped redaction entry point, check again that `copy_in` and the heap limits are exercised on real qpdf.wasm, not only on NativeBridge. Related: [[m2-redact-handle-trait]], [[m2-web-handle-newtype]].
