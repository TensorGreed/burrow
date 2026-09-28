---
name: exemption-list-postable-first
description: An exemption keyed on "the list the harness posted first" is forgeable when the harness posts nothing in some mode; also, a per-entry witness proves a rule exists, not how tight it is
metadata:
  type: feedback
---

On d00aa49 (#137 R8) stub reports were accepted only for exits "the armed list" named, and the
armed list was trusted if it was message 0. With no stubs armed the prologue posts no list, so a
bundle whose FIRST statement posts `{__burrowSideChannelArmed: [...]}` owns index 0; 512 forged
reports then decoded the input's first 64 bytes with r8Violations = [] in all three browsers.
(The stubbed R9 run caught the same worker, so the suite held; R8's own claim did not.)

Same commit: 25 per-FIELD_RULES hand cases each feed one refused value ("x y" / 1.5), so every
rule's EXISTENCE was witnessed but its tightening was not: `-0`, u64 canonical/max, the "0" pins,
the report's digit and 64-font caps could each be reverted with the suite green.

**Why:** "witnessed on its own" was read as "every clause witnessed"; and "first message" is only
the harness's if the harness always sends one.
**How to apply:** for a positional trust ("first", "once"), plant the forgery at the very top of
the bundle in the mode where the harness posts nothing. For per-entry witnesses, revert each
tightening to the previous rule, not to `() => true`. Do not run probes while a sweep rebuilds
dist on the same port. See [[rule-table-ablate-per-entry]], [[harness-own-prefix-exemption]].
