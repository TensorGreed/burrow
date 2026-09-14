---
name: feedback-review-expectations
description: How the burrow maintainer wants code reviews done — named design decisions challenged, claims verified by running things, not asserted
metadata:
  type: feedback
---

When asking for a review, the maintainer names specific design decisions and asks for them
to be *challenged* (e.g. "is a default-off feature the right call given 'tests are part of
the feature'?", "is hand-parsing TOML a false economy?", "is there a less blunt way?").
They want a verdict on each, not a restatement of the code. Answer each named point
explicitly, including the ones where the answer is "yes, this is right, and here is why the
reasoning holds".

**Why:** the review is being used as a design gate, not a lint pass. An unanswered
challenge point is the part of the review they will notice is missing.

**How to apply:**
- Verify claims by running them rather than reasoning about them. `cargo` lives at
  `~/.cargo/bin` and is not on the default sandbox `PATH`; export it. A throwaway
  `git worktree` in the scratchpad is the clean way to reproduce a CI job that depends on
  gitignored build outputs being absent.
- Say "I could not verify this" rather than asserting, and label judgement calls as
  optional. They react badly to padded findings.
- Work arrives **staged but uncommitted**, often with the feature branch sitting at the same
  commit as `main`. `git diff main...HEAD` is then empty and says nothing — use
  `git diff --cached` (plus `git diff` for anything another agent changed in flight).
- Expect other agents to be editing the branch *while* you review it. Re-read files
  before reporting, and say which findings were already fixed in flight.
- Test strength is usually the headline ask. Mutation-test rather than eyeball: copy the
  module and its test file into the scratchpad and run
  `npx vitest run --root <scratchdir>` from `apps/web` (resolves vitest from the app's
  `node_modules`, leaves the repo untouched). Report the exact mutations that survived.
  Native `cargo test -p burrow-engines --all-features` runs locally — the engines are
  already built in `engines/` on this machine.
- The leak/closure test harnesses shell out to a `qpdf` CLI that is **not installed on this
  machine**. The engine build leaves one at
  `engines/vendor/src/build-qpdf-plain-<arch>/qpdf/qpdf`; symlink it into a scratchpad `bin`
  and prepend that to `PATH`, or `split_no_leak.rs` and `subset_closure.rs` panic on
  `Command::new("qpdf")`.
- `#[ignore]`d tests are the repo's way of pinning a known-unmet rule. Run them with
  `-- --ignored` to check they still fail, and for the reason claimed.
- Rust mutation testing here: edit the file in place, run
  `cargo test --workspace --all-features`, restore from a scratchpad copy. Two traps.
  (a) **A surviving mutation is not automatically a gap** — check it is not semantically
  equivalent first. `qpdf_add_page_at`'s `before` flag flipped to `after` survives the whole
  suite, and a brute-force simulation over every permutation up to n=6 shows both variants
  produce the identical result, so the tests are not at fault.
  (b) **A mutation that breaks a proptest writes `tests/<name>.proptest-regressions`** into
  the repo. Delete it before finishing — this repo fails CI on tracked generated files.
