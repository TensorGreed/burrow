---
name: feedback-probe-reimplements-rule
description: Review technique — when a Rust source-scan test carries a "the rule is probed" closure, check the probe calls the same predicate the scan uses; a re-implemented copy passes while the real filter is inert
metadata:
  type: feedback
---

burrow's source-scan tests (`no_method_takes_both_a_handle_and_a_document` in
`qpdf/handle.rs`, `no_method_takes_a_handle_and_a_separate_document` in `web/handle.rs`)
filter `include_str!` of their own file and then "probe" the rule. **Check whether the probe
invokes the same predicate the scan ran, or a hand-copied closure beside it.** A copy means
the probe asserts about itself: break the real filter chain, leave an offending line in the
file, and the test still passes.

**Why:** measured 2026-09-21 on `web/handle.rs`. Changing the real filter's
`line.contains("QpdfPtr")` to `"QpdfPtrXX"` with a planted offending method present left the
test green — the `catches` closure below it still matched and still asserted, and the
`methods >= 15` floor still counted. Same family as [[feedback-inert-detector-path]].

**How to apply:** three mutations, each run, each on a copy or a `git worktree` at HEAD so
the working tree is not disturbed — (1) plant an offender in the shape the rule names,
(2) plant one whose signature rustfmt would wrap across lines, (3) plant one with an extra
modifier (`const fn`, `async fn`, `pub fn`). A line-oriented scan usually catches only (1).
Report each result as measured, not inferred. Related: [[feedback-review-expectations]].
