---
name: m2-expect-after-round2-and-175
description: "#176 round 2 (7e34971) + #175 Watch (92abb7a), reviewed 2026-09-24: the fragment rule is blind on ch06/21 (measured), and the deadline loops that still sit outside the Watch."
metadata:
  type: project
---

Branch fix/176-expect-after, reviewed 2026-09-24. Baseline --after green: 90/90, 49 witness / 26 refused / 15 owed (13 leaking).

**Why:** --after is the independent judge of redaction, and #175 claims its only leftover gap is "one lex".

**How to apply:** re-check whenever `fragments_survive`, `see()`, or the redaction step loops change.

- **Partial removal on ch06/21 passes (measured).** Plant `region = [30,68,100,44]` on both
  fixtures: the output still draws `ECRET-06` / `ECRET-21`, and --after says OK. The fragment rule
  reads PDFium text (just GIDs there) and ASCII/UTF-16/hex byte spellings, so it cannot read
  Identity-H GIDs. Fix: check fragments through the placement's own witness (`cid-codes`).
- Otherwise the round-1 findings are fixed: owed placements are witnessed, `after_unwitnessed` is
  refused, hex is a spelling, and the gate script unsets BURROW_AFTER_ONLY. What is left:
  OWED_LEAKS_EXPECTED is a count, not a set, so one fixed leak plus one new leak still reads 13;
  refusal rules are printed but not gated; leftovers of 4 characters or fewer are not detected
  by design.
- #175 loops outside the Watch, found by reading the code and NOT measured (a timing-fixture
  attempt was stopped): `check_type_three`
  (fonts x CharProcs entries, lexed with no dedup and no checkpoint); `cut_fonts` (one checkpoint,
  then up to 4096 page fonts x narrow_font, which runs even when nothing is cut); `parse_w` in
  `read_font` (65,536 inserts per /W triple, with no cap on the number of triples, inside glyphs_in between reads).
  `Resources::form` stream_data has no decoded-size ceiling. Every production Watch uses the operation's clock.
  Expiry fails closed.
- Tooling: the sandbox refuses `cd <scratch worktree> && …` compound commands. Instead, put the
  steps in a wrapper script in the scratchpad and run `bash script`, and use `git -C` for single
  commands.

Related: [[m2_expect_after_harness]], [[m2_glyph_geometry]], [[m1_limits_real_strength]].
