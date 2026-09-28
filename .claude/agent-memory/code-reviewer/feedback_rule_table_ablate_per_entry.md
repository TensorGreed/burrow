---
name: rule-table-ablate-per-entry
description: A table of per-field validators (FIELD_RULES) is N rules, not one per validator function; loosen each ENTRY and list survivors, and treat a "bytes" type as carrying metadata
metadata:
  type: feedback
---

When a check maps field names to validators (#137 R8's `FIELD_RULES`), a plant per shared
validator (`u64`, `integer`, `oneOf`) turns the validator's ablation red and leaves every other
entry that uses it unwitnessed. Measured on 2930dfb: loosening `innerKind`, `limit`, `requested`,
`engineHeapBytes`, `minConvergingMemoryBytes`, `retainedFonts`, `failedInput` or `id` to "any
string/number" left the R8/R9 specs green, while the ADR said only three stated belts survived.
The hand-log fixture's reply had 5 of ~19 real fields, so no hand log could reach the rest.

Also on that commit: a shape encoder that maps `instanceof Blob` to `bytes` admits a `File`
whose NAME is the whole input (3/3 browsers), and a harness-report exemption with no count
(`{__burrowSideChannel: name}` x1800) carried 600 input bytes past R8 alone.

**Why:** "every rule switched off in turn" is only as fine as the unit you switch off.
**How to apply:** ablate per table entry and per exemption branch; for any "bytes"/opaque type,
plant metadata (File name, Blob type). See [[harness-own-prefix-exemption]],
[[timing-witness-decays-under-load]], [[probe-fixture-vs-real-producer]].
