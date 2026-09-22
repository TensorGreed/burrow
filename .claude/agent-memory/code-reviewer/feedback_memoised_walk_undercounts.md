---
name: memoised-walk-undercounts
description: When a graph walk memoises "already descended", probe a child nested under a twice-referenced parent; its count comes back 1 and reads as unshared.
metadata:
  type: feedback
---

A reference-counting walk that keeps a `descended`/`visited` set to avoid re-walking a subtree
counts **references to the node**, not **times the node is reached**. Plant a fixture where form
A is referenced twice and A's own resources reference B once: A comes back 2 and B comes back
**1**, though editing B changes both pages.

**Why:** measured on m2/131 (`core/burrow-engines/src/qpdf/sharing.rs`). The module header said
under-counting "reads as unshared — the direction that edits in place", and the walk did exactly
that transitively. Every committed fixture was one level deep, so nothing caught it; the cycle,
depth, inheritance and annotation fixtures all pass with the defect present.

**How to apply:** whenever a walk has both a per-reference `counts[x] += 1` and a
`if !descended.insert(x) { return }`, write the two-level fixture before believing the counts.
Ask the same of the consumer: if the rule only inspects the *innermost* container a glyph came
from (`glyph.source.form`), the enclosing chain's multiplicity never gets asked about either —
two independent defects that have to be closed together. Related: [[review-expectations]],
[[inert-detector-path]].
