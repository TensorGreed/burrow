---
name: fuzz-seed-prefix-check
description: Review technique — decode seed-fuzz-corpus.py's fixed 0x05 prefix by hand through any new fuzz target, because an inert seed corpus reads as coverage
metadata:
  type: feedback
---

When a change adds a fuzz target with a parameter prefix, hand-decode the seeder's fixed
prefix through the target's own arithmetic before believing the seeded-60s claim.
`tools/seed-fuzz-corpus.py` writes `bytes([5] * prefix) + document` — every seed's parameter
bytes are `0x05`. `verify_prefixes()` only checks that the target slices `&data[N..]`; it
cannot see that `5` decodes to a request the target refuses.

Measured on #57's `render` target: `let wanted = usize::from(data[2] % 5)` made `5 % 5 == 0`,
which the target's own `if wanted == 0 { return; }` turned into an immediate return — all 20
seeds inert, the engine never reached on the first pass.

**Why:** this repo's whole seeded-corpus rule exists because an unseeded target measured the
parser's rejection path and nothing else (a planted defect survived 577,209 executions). An
inert seed reproduces that failure while the CI step still passes.

**How to apply:** for each new target, substitute 0x05 for every prefix byte and walk the
early-return conditions. Report it as blocking the definition-of-done fuzz item, and suggest
changing the target's decoding (e.g. `1 + data[2] % 4`) rather than the shared prefix.
Related: [[feedback-review-expectations]].
