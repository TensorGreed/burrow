---
name: regex-parsed-config-list
description: An "expected count" a checker derives by regex over a config file (e.g. Playwright projects) undercounts silently; plant an entry the regex cannot read and compare with the tool's own list.
metadata:
  type: feedback
---

A checker that derives its expected set by regex over a source file (e.g. #137's ledger checker
reading `{ name: "([a-z]+)", use: ` from playwright.config.ts) misses entries with hyphens,
digits or a prettier line-wrap. Measured 2026-09-28: Playwright listed 5 projects, and the
checker expected 3 and would have printed OK.

**Why:** the rule in CLAUDE.md is to gate on the expected count. An expected count that is
itself a lossy parse is "4 of 15" again, with the 15 wrong this time.

**How to apply:** plant one entry of each unusual shape, then diff the checker's list against
the tool's own (`playwright test --list`). Also check whether the self-test ever runs the
checker's DEFAULT invocation. #137's self-test always passed `--projects`, so it never ran
CI's exact call. See [[calibration-list-vs-accepted-set]].
