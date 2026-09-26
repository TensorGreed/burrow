---
name: differential-probe-bypasses-harness
description: A differential's "the second engine is really asked" probe that calls that engine on its own guards neither the harness nor delegation; plant delegation to the reference engine.
metadata:
  type: feedback
---

When a differential test (engine B held to engine A) ships a probe that says "a harness that called A twice could not pass", check what the probe calls. In #191's `web_differential_tests`, `the_web_engine_is_really_asked` called `web.redact_page` directly and checked that two regions gave two outputs. That holds for native too. So making `impl PageRedactor for WebQpdf` delegate to `qpdf::Qpdf` kept all 5 tests green (measured 2026-09-25), and swapping the harness's `web.` for `native.` is invisible to it.

**Why:** the probe proves B varies with its input. It does not prove B's own code path ran. A and B produce the same outputs, so no output-only probe can tell them apart.

**How to apply:** plant "B delegates to A" at the trait impl and run the suite. The fix to ask for is a call counter on B's own seam, such as a bridge method only B's path reaches, asserted non-zero or per-case inside the harness loop. Related: [[mutate-the-wiring-not-the-policy]], [[probe-reimplements-rule]].
