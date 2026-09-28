---
name: hyphenated-job-name-match
description: test-ci-local's scenario() greps job names with \b, so "web" is satisfied by web-e2e and "wasm" by always-selected wasm-pack; plant a skip of the short name
metadata:
  type: feedback
---

`tools/test-ci-local.sh`'s `scenario()` checks must_run/must_skip with `grep "\b$job\b"`. A hyphen is a word boundary, so `web` matches `web-e2e`, and `wasm` matches `wasm-pack`. `wasm-pack` has underivable paths and runs on every change, which makes `wasm` in must_run always true. Measured on #214 (ba0879d): planting a forced skip of the `wasm` job left all 49 cases green.

**Why:** a scenario that names a job it cannot fail on is inert, and it reads as coverage.

**How to apply:** when a change adds or edits a `scenario` line, force-skip each short-named job in a copy and check that the scenario fails. Also look for `paths_as` jobs: `artifacts_read` and `reached_text` read `job["run"]` only, so a wrapper script loses any `cd`-dependent derivation (see [[mutate-the-wiring-not-the-policy]]).

Re-review at a03c4a1: `job_texts` fixed that, but no table job used `paths_as` together with Playwright, so deleting the merge left 50/50 green. A fix for a *simulated* future shape is unwitnessed unless the suite carries the simulation. The same fix overwrote the `"(the job's command)"` key, which dropped inline guards. `PLAYWRIGHT_RUN`'s `(?=\s|$)` misses `playwright test)` and `playwright test;`.

Round 3 (676c8d5): a synthetic `select_changed` table whose changed file no job claims gets "RUNS" from the ORPHAN rule, not from the rule under test. The wrapper scenario stayed green with `artifacts_read` reverted, while the real table skipped web-e2e. The synthetic table needs a job that claims the changed path, and the scenario should assert the "under no job's derived paths" line is absent.
