---
name: depth-capped-dfs-padding
description: A DFS with a depth cap and a visited set can be starved by undrawn padding siblings; also time per-glyph resolution that re-walks a graph. Probe both with N padding forms.
metadata:
  type: feedback
---

#218 round 3 (2026-09-28): `chain_from` in `redact/resources.rs` recursed into the SAME scope for a
form with no own `/Resources`. That recursion is redundant, since any chain through such a form
resolves exactly as the chain without it, so it survives a mutation that deletes it. It is not
harmless, though. With 31 or more undrawn resourceless forms sorted ahead of the real chain, the
DFS spent the 32-deep cap on the padding. It marked the real form visited at the cap, and a valid
nested document was refused with `[font-scope-unresolved]` ("a form the page's resources do not
reach"), which was a false reason. The same resolver ran once per glyph with no memo. At 2000 forms
and 8000 glyphs (313 kB) it took 20.2 s against a 1 s budget. The parent took 6.3 s.

**Why:** a "cannot be reached" refusal read as defensive and unreachable, and nothing tested it.
The reachable trigger was padding, not a missing form. The cost multiplier only shows up at
N forms by G glyphs.

**How to apply:** when a change adds a graph search with a depth cap and a visited set, build
padding fixtures at N = cap-2, cap-1, cap and cap+8, run them at HEAD and at the parent, and
compare. Time any resolver called per glyph at a few hundred kB with a 1 s budget. Related:
[[residual-bound-by-uncounted-dimension]], [[memoised-walk-undercounts]].

Also from this round: when a "caught?" column in a circularity table says **yes**, ask whether
the read-back catches it or the operation prevents it. When both share one resolver, it is
prevention, and the row overclaims. And when a claim cites a sibling PR's plant, read that
PR's plants: #219 had none for narrowing.
