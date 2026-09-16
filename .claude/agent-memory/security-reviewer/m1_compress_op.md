---
name: m1-compress-op
description: Measured facts from reviewing burrow_ops::compress (M1 phase 2) — memory amplification vs rotate, the nightly instrumented-archive gap, and which mutations the unit tests survive.
metadata:
  type: project
---

`burrow_ops::compress` (branch `compress-engine-seam`, reviewed 2026-09-16). Measurements that
are expensive to re-derive.

**Why:** the op's own docs make quantitative claims (never worse, limits, fuzz reach); these are
the numbers behind whether they hold.

**How to apply:** reuse rather than re-measure when compress or its fuzz wiring changes again.

## Memory: compress is NOT worse than the existing ops

Generated bomb (1 page, 300k tiny referenced objects, 21.5 MB, xref correct, recovery off):

| op | peak RSS | time | result |
|---|--:|--:|---|
| `compress` | 325 MB | 1.0 s | Smaller 21,567,091 → 3,084,322 |
| `rotate` | 366 MB | 0.94 s | ok |

Same generator at 1.5 M objects (110 MB input): compress peaks at **1.6 GB** and then reports
`LimitExceeded max_memory_bytes at measured (requested 1,529,712,640, allowed 1,073,741,824)` —
detection after the fact, exactly as ADR 0007 says. ~14x amplification, linear in object count;
at the 512 MB `max_input_bytes` default that extrapolates to ~7 GB before anything fires.
Pre-existing class (see [[m1-limits-real-strength]]), not introduced by compress.

`objstm-bomb.pdf` through compress: 204 kB → 268 MB RSS, 0.55 s, output 520 bytes, accepted.

## The ADR 0022 read-back genuinely fires on compress

`tests/damaged/page-loss-on-write.pdf` → `Err output rejected: compress: 5 pages were expected
and 4 came out`. Live, fail-closed, on a committed fixture.

## Fuzz: the nightly run is seeded but NOT instrumented

`.github/workflows/fuzz-nightly.yml`'s `case` selecting the instrumented qpdf archive lists
`qpdf_check|merge|split|rotate|reorder` — **compress is absent**. Measured cost, 60 s seeded
from the 20 committed fixtures:

| archive | cov | ft | exec/s |
|---|--:|--:|--:|
| instrumented (`lib/fuzz`) | 6,294 | 24,463 | 806 |
| plain (what nightly runs) | 479 | 1,412 | 2,178 |

ASan *interceptors* still fire without it — the plain run reproduced the #62 heap-use-after-free
(`getKeys` ← `pushInheritedAttributesToPageInternal`, freed via `QPDFObject::move_to`) in under
60 s from a mutated `page-loss-on-write.pdf`. So the gap costs coverage feedback, not crash
detection. Confirms ADR 0025's own trace: it is on the **open** path, not in the writer.

## Mutations the unit suite survives (as of this review)

Verified in a full copy of the tree under the scratchpad, mutation asserted before running:

- Deleting **both** of compress's `deadline.checkpoint` calls: all 9 unit tests still pass.
  `the_operation_refuses_when_its_own_budget_is_spent` is satisfied by `verify::output`'s own
  first checkpoint. A probe driving the **NotSmaller** path (fake produces 250 against 100,
  stepping clock, budget 5 ms) does kill it — it returns `Ok(NotSmaller)` mutated and
  `LimitExceeded` unmutated.
- Moving the `rotations` promise sweep to **after** `engine.compress`: all tests still pass.
  ADR 0022's "computed before the operation runs" is unenforced for compress.
- Deleting the `verify::output` call: two tests fail, as the definition of done requires.
