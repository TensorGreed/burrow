---
name: feedback-ci-diagnostic-step-lies
description: Review technique — a CI step that summarises a failure must be checked against real tool output: its paths, its regex character classes, and the failure modes it will announce as the wrong cause
metadata:
  type: feedback
---

When a change adds a workflow step that *describes* a failure (a step summary, a triage line, a
symbolised frame), do not read it — feed it a realistic log and check the paths it names exist.

Three things to check every time, all measured on `fuzz-nightly.yml`'s
"Describe the crash without publishing it" (2026-09-18):

1. **The path the step builds vs where the tool actually writes.** `binary="target/$(uname -m)-…/release/<target>"`
   resolved from the repo root, but `cargo-fuzz` writes to `fuzz/target/<triple>/release/`
   (the previous step had `working-directory: fuzz`; this one did not). The symbolisation —
   the entire purpose of the change — could never fire, and the `else` branch announced
   "no offset in the report" when the offset had been found and the *binary* was missing.
2. **Character classes against the real vocabulary.** `[a-z-]+` after `ERROR: AddressSanitizer: `
   matches `stack-overflow` and misses `SEGV` and `DEADLYSIGNAL`, which then fall through to a
   fallback claiming "timeout or OOM". Same family as the `[a-z- ]` invalid-range bug the
   maintainer had already fixed one line above.
3. **What `if: failure()` actually catches.** It fires when *any* earlier step failed —
   checkout, engine build, a compile error — so a step that opens with `### <target>: crash`
   asserts a crash for every infrastructure failure. Gate on `steps.<id>.outcome` plus a marker
   in the log, and give the non-crash case its own sentence.

**Why:** this repo's stated rule is "a check that names the wrong cause is worse than one that
names none, because the reader believes it". A diagnostic step is a check.

**How to apply:** reconstruct the tool's output in the scratchpad (`==1==ERROR: AddressSanitizer:
SEGV …`, a frame line `(/path/<target>+0x1a2b)`) and run the step's own pipeline over it.
Related: [[feedback-inert-detector-path]], [[feedback-review-expectations]].
