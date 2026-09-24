---
name: feedback-rewriter-reach-vs-detector-reach
description: Review technique — when a refusal is replaced by a rewrite, feed the detector's WIDEST accepting shape to the rewriter; a recursive detector paired with a top-level-only rewriter emits the secret with no refusal left to catch it
metadata:
  type: feedback
---

A commit that turns "refuse when X is present" into "rewrite X out" splits one predicate into
two: the **detector** that decides a span is dangerous and the **rewriter** that neutralises it.
Check they have the same reach, by constructing the shape the detector accepts most broadly and
running it through the rewriter.

**Why:** measured on d37556c (#165, `/ActualText` rewriter). `carried_by`/`holds_text_key`
recurse into nested dictionaries and arrays — its own comment says "read at any depth" — while
`without_carried_keys` strips only **top-level** keys. So
`/Span << /A << /ActualText (secret) >> /MCID 0 >> BDC` produced one edit whose replacement was
byte-identical to the input, `check_marked_content` returned `Ok` (the refusal had just been
deleted), and the secret was emitted. Before the commit the same document refused. The whole
probe set was green: every fixture put the key at the top level.

**How to apply:** find the detector predicate and the rewriter, and diff the *shapes* they
handle, not the code. Nesting, aliasing, case, and "value that looks like a key" are the four
that differ. Write the probe as a unit test calling the rewriter and asserting the canary is
absent from the **returned bytes** — the repo's own `strips()` helper already asserts that, so a
one-fixture addition kills it. Prefer recommending a verify-after-rewrite (re-run the detector on
the rewritten output and refuse if it still fires, ADR 0022's shape) over patching the instance.

Related: [[feedback-inert-detector-path]], [[feedback-probe-reimplements-rule]],
[[feedback-mutate-the-wiring-not-the-policy]].
