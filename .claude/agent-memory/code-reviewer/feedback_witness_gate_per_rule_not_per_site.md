---
name: witness-gate-per-rule-not-per-site
description: Review technique — geometry.rs's #177 witness gate (track_caller recorder + one witness per Refusal) proves one raise site per rule from any pub fn; plant a pub fn nobody calls, neuter second sites, and drop the RAISED clear
metadata:
  type: feedback
---

`every_refusal_is_raised_by_production_code_when_its_witness_runs` (6bc04a9, #177) replaced the
text-scan gate in [[text-scan-gate-passes-dead-rule]]. It records `(rule, Location::line)` in
`refuse` under test and requires a production line. What it does NOT prove, all measured
2026-09-24 in a worktree, crate lib suite 417/417 green each time:

- **A rule raised only from a `pub fn` no production code calls passes.** `pdfsyntax::geometry`
  is a `pub mod`, so no dead-code lint fires. Witnesses call pub checkers directly
  (`check_type_three_procedure`, `check_form_sharing`, `carried_text_edits`, `writing_mode_of`),
  so the gate is module-level raisability; wiring lives in `tests/redaction_defences.rs`.
- **Per rule, not per site:** 38 rules, 64 raise sites, witnesses hit 38. Neutering three
  second sites (trailing `WMode` in a CMap program, font-metrics non-finite, remove-path `TJ`)
  left every lib test green.
- `any()` over records: a witness that runs a real raise, discards it, and returns a
  test-built refusal passes. Deleting `RAISED.clear()` in `prove` survives the probe test.

**Why:** the gate's doc says "a variant nothing can raise leaves its witness nothing to return".
The pub-fn plant falsifies that.

**How to apply:** when this gate or a copy of it is touched, re-run those three plants. Print
the recorder per rule (instrument the loop) to see which site each witness hits.
