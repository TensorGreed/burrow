---
name: nearmiss-fixture-accepted-as-refusal
description: Review technique — a corpus test whose Err arm accepts "refused by an allowed rule" accepts it for the near-miss fixtures too, so the twin that exists to stop over-refusal cannot fail; flip the rule wide and watch the near-miss go green
metadata:
  type: feedback
---

burrow's evasion-corpus tests pair each `evade-*` fixture with a `nearmiss-*` twin whose whole
purpose is that the rule **declines** to fire. The test loop is usually written as
`match redact(&pdf) { Err(e) => assert!(ALLOWED_RULES.iter().any(|r| e.contains(r))),
Ok(out) => assert_absent(&out, canary) }` — one arm for every fixture in the list. **That `Err`
arm accepts a refusal for the near-miss too**, and a refusal keeps the canary out of the output,
so over-refusal reads as the defence holding.

**Why:** measured on cc1d0b9 (#165). Reinstating the rejected wide rule — one line,
`Operand::Str { .. } => true` in `names_a_text_key` — flipped
`nearmiss-ordinary-string-in-a-property-list` from redacted to refused. The census moved
51 redacted/8 refused → 50/9 and **every integration suite stayed green**
(`redaction_corpus`, `redaction_defences`). Only the unit probe in `geometry.rs` caught it. The
commit had added that fixture saying "a rule with probes and no corpus witness is a rule nothing
runs" — and as a witness it asserted nothing. Hardening `CARRIER_REFUSALS` to name the allowed
rules closed the wrong half: it stops a fixture refusing for an *unrelated* reason, not a
near-miss refusing at all.

**How to apply:** on any commit adding an evasion/near-miss fixture pair, or tightening the
`Err` arm of a corpus sweep. Widen the rule the near-miss is a twin of and re-run the
integration suites, not just the unit probes. The fix is seven lines and verified to kill the
mutation while passing clean: `assert!(!name.starts_with("nearmiss-"), "…must be redacted, not
refused")` at the top of the `Err` arm. The durable fix is the manifest's `expect_after` field,
which `check-redaction-corpus.py` still prints as unchecked.

Related: [[calibration-list-vs-accepted-set]], [[feedback-rewriter-reach-vs-detector-reach]],
[[feedback-probe-reimplements-rule]].
