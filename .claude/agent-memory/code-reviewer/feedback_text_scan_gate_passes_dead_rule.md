---
name: text-scan-gate-passes-dead-rule
description: Review technique — burrow's "every refusal is both raised and tested" gates are source-text scans, so a variant raised from unreachable code and "tested" by a bare mention in an array literal passes all of them; plant one to show it
metadata:
  type: feedback
---

`geometry.rs`'s module gates — `every_refusal_is_both_raised_and_tested`,
`every_rule_name_is_distinct`, `refusals_are_the_only_way_to_refuse_in_this_module` — read
`include_str!("geometry.rs")` and grep. "Raised" means the substring `Refusal::X.refuse(`
appears in the production half; "tested" means `Refusal::X)` or `Refusal::X,` appears in the
test half. Neither is a reachability or behaviour claim, though the gate's own comment says
*"a variant that nothing raises is a rule that cannot fire; one that no test names is a rule
nobody checks"*.

**Why:** measured on be385aa (2026-09-24). A planted `Refusal::NeverReachable`, raised only
from `fn unreachable_raiser(never: bool) { if never && !never { … } }` and "tested" by
`let _mentioned = [Refusal::NeverReachable,];`, passed all three gates with the hardcoded
`total, 35` bumped to 36. **557 tests passed, 0 failed, `cargo clippy --all-targets -D warnings`
rc 0.** A variant raised from an `#[allow(dead_code)]` fn passes too; without the `allow`,
clippy would catch that one half.

**How to apply:** whenever a commit adds or removes a `Refusal`-style variant and the reviewer
is asked "are the gates still doing work". Plant the variant rather than reasoning about the
scan. The fix that would actually hold: under `#[cfg(test)]`, have `refuse()` record the variant
in a set, and assert at the end of the run that every variant in `ALL` was raised by some test —
a reachability check instead of a grep. Recommend it as a design change, not a patch.

Related: [[feedback-probe-reimplements-rule]], [[nearmiss-fixture-accepted-as-refusal]].
