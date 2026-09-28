---
name: m2-web-differential-137
description: #137 browser differential + ledger (0ea646b, 4806150) — comparison is leak-tight; the ledger counts examined not agreed, so test.fail()/runtime test.skip after record() is green with "463 compared, 0 skipped" (measured).
metadata:
  type: project
---

Reviewed 2026-09-28 at 4806150 (branch test/137-web-differential). No exploitable leak.

- **Comparison holds for leaks.** Every refusal branch checks `reply.ok` before kind/rule/limit, and OK compares both sha256s exactly (`outputSha256` null unless output is a Blob). So the only leak-shaped divergence (web redacts where native refuses, or redacts differently) cannot pass `divergence()`. Rule/kind/limit looseness only mis-compares refusal-vs-refusal. All 463 golden outcomes carry at most one `[rule]` (measured), so first-bracket `ruleOf` is fine today.
- **Measured bypass (chromium, exit 0):** a mutant spec that arms the left/top-swap worker plant on two documents, then `test.fail()` on one and `test.skip(divergences.length>0)` after `record()` on the other: 5 real divergences, Playwright "135 passed, 1 skipped", ledger "463 compared, 0 skipped". Cause: record-before-verdict by design. Fix: record after the verdict, or record the verdict and require agreement.
- **Stale ledger:** CLI runner clears outputDir; the test-server path (UI mode / VS Code) sets `preserveOutputDir: true` (playwright 1.63 runner/index.js ~6845), so a ledger-only rerun there reads a previous run's file. Local only.
- Artifact hand-over is digest-gated per document; upload sits between corpus generation and `cargo test`, nothing mutates in between.
- Self-test report.json is not deleted between runs; plant counts (356/458/345) differ today so a stale report cannot pass.
- Method: whole chromium spec is ~30 s; ports 4331/4332; the code reviewer's worktree was /tmp/burrow-review-code (its playwright processes are not mine).

**Round 2, 3a4bd30 (same day).** Ledger now records `{name, agreed}`; the parked-doc-test plant is refused (self-test 5/5 measured). Still green, measured chromium, with the region-swap live on producer-writer:
- Park BOTH the doc test and the ledger test with `test.fail()`: exit 0, summary even says "3 diverged". Nothing checks the ledger test itself ran/passed (skip is the same). Fix idea: assert from the JSON report, or globalTeardown.
- One-token misbinding `divergence(cases[0].outcome, replies[0])`: "463 compared, 0 diverged", only 107 truly compared, no parking needed. The verdict is still the doc test's `why`; fix idea: record the reply's compared fields and have reconcile recompute divergence against goldenCases' own outcome by name.
- `if (!agreed)` accepts any truthy value (e.g. "false"); prefer `agreed !== true`.

**Round 3, 8b8a951 (same day).** Ledger records `observed` and reconcile re-judges by name; plus an outside CI checker (tools/check-redaction-differential-ledger.mjs). Both r2 escapes are closed. Measured escape: the checker takes its project list from the regex `/\{ name: "([a-z]+)", use: /` over playwright.config.ts. Adding `{ name: "webkit-mobile", ... }` with a parked divergence gives Playwright exit 0 (449 passed, 107 skipped) and the checker "examined 3 ledgers of 3 projects ... OK", while the webkit-mobile ledger (107 diverged) is ignored beside them. Fix idea: require the set of ledger files to equal the configured set, or take projects from `playwright test --list --reporter=json`. `isObserved`'s doc says extra fields are refused; they are accepted (harmless). A full 3-browser run of the spec takes 2.3 min.

**Round 4, 053c98a (same day).** Clean. The checker now gets its projects from `playwright test --list --reporter=json` and requires the set of ledger files to equal them. `--list` does NOT empty test-results (measured with sentinel files). The self-test ran 11/11. Every failure mode I found fails closed: a project whose testIgnore excludes the spec (refused as "no ledger", a bit coarse), a per-project outputDir, polluted stdout or PLAYWRIGHT_JSON_OUTPUT_NAME set (JSON parse refusal), a pnpm spawn error.

Related: [[m2-redaction-binding-137]], [[m2-r8-r9-specs]].
