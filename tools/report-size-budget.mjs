#!/usr/bin/env node
// Report the first-load payload against its budget, as a markdown table.
//
// Writes to `$GITHUB_STEP_SUMMARY` when CI sets it, and to stdout always. **Not a PR
// comment**: `ci.yml` runs with `permissions: contents: read`, a comment would need
// `pull-requests: write`, and a token with write access to the repository is not something
// to add for a size table. A step summary renders on every PR including one from a fork,
// which a comment does not.
//
// It reports the change without needing a baseline build of `main`. The comparison is against
// `measured_brotli` in `apps/web/size-budget.json` -- the value each budget was actually set
// from -- so the table answers "how much has this grown since someone last looked at it",
// which is the question, and answers it offline and deterministically.
//
// Never fails the build. `apps/web/src/size-budget.test.ts` is the gate; this is the report,
// and a report that can fail a build turns into one people stop reading.

import { readFileSync } from "node:fs";
import { appendFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { byBudgetKey, firstLoad } from "./first-load.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const webApp = join(repo, "apps", "web");

const buildDir = process.argv[2] ?? join(webApp, "dist");
const budget = JSON.parse(readFileSync(join(webApp, "size-budget.json"), "utf8"));

const measurement = firstLoad(buildDir);
const groups = byBudgetKey(measurement);

const kib = (n) => `${(n / 1024).toFixed(1)} KiB`;
const delta = (now, then) => {
  const d = now - then;
  if (d === 0) return "—";
  const pct = then === 0 ? "" : ` (${d > 0 ? "+" : ""}${((d / then) * 100).toFixed(1)}%)`;
  return `${d > 0 ? "+" : "−"}${kib(Math.abs(d))}${pct}`;
};
const verdict = (now, limit) => (now > limit ? "❌ over" : "✅");

const rows = [];
for (const key of Object.keys(groups).sort()) {
  const line = budget.artifacts[key];
  const actual = groups[key];
  rows.push([
    `\`${key}\``,
    kib(actual.brotli),
    line ? delta(actual.brotli, line.measured_brotli) : "—",
    line ? kib(line.budget_brotli) : "**no budget**",
    line ? verdict(actual.brotli, line.budget_brotli) : "❌ unbudgeted",
  ]);
}

const table = [
  "| Artifact | brotli | vs recorded | budget | |",
  "|---|--:|--:|--:|:--|",
  ...rows.map((r) => `| ${r.join(" | ")} |`),
  `| **Total first load** | **${kib(measurement.total.brotli)}** | ` +
    `**${delta(measurement.total.brotli, budget.total.measured_brotli)}** | ` +
    `**${kib(budget.total.budget_brotli)}** | **${verdict(measurement.total.brotli, budget.total.budget_brotli)}** |`,
].join("\n");

const report = [
  "## First-load size budget",
  "",
  "Everything a user downloads before their first operation can run: the page shell, the",
  "worker bundle, all three wasm modules, and the CSP control file. Brotli, because that is",
  "what a host serves.",
  "",
  table,
  "",
  `Raw (uncompressed, what the browser compiles): **${kib(measurement.total.raw)}**.`,
  "",
  `_"vs recorded" compares against \`measured_brotli\` in \`apps/web/size-budget.json\`, taken`,
  `on ${budget.measured_on}. The gate is \`apps/web/src/size-budget.test.ts\`, not this report._`,
].join("\n");

console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, report + "\n");
}
