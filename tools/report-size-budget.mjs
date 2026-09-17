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

import {
  byBudgetKey,
  engineClosures,
  heaviestFirstLoad,
  renderFirstLoad,
} from "./first-load.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const webApp = join(repo, "apps", "web");

const buildDir = process.argv[2] ?? join(webApp, "dist");
const budget = JSON.parse(readFileSync(join(webApp, "size-budget.json"), "utf8"));

// THE HEAVIEST ROUTE, which is what the GATE measures. This called `firstLoad(buildDir)`,
// which defaults to `index.html` -- the LIGHTEST route, because the home page carries no
// island. So the report on every pull request compared the home page's payload against the
// total budget and printed a delta against a recording taken on a tool page.
//
// Measured when /split-pdf landed: the report said the total was DOWN 19.6 KiB (-0.8%) in the
// same build where the heaviest route had grown by 2,704 bytes. A reassuring number about the
// wrong thing is worse than no number, and this one runs as a step summary on every PR --
// which is exactly where somebody reads it instead of the gate.
//
// `measured_route` is printed too, so the report says which page it weighed rather than
// leaving it to be assumed.
const { page: measuredRoute, measurement } = heaviestFirstLoad(buildDir);
const groups = byBudgetKey(measurement);

const kib = (n) => `${(n / 1024).toFixed(1)} KiB`;
const delta = (now, then) => {
  const d = now - then;
  if (d === 0) return "—";
  const pct = then === 0 ? "" : ` (${d > 0 ? "+" : ""}${((d / then) * 100).toFixed(1)}%)`;
  return `${d > 0 ? "+" : "−"}${kib(Math.abs(d))}${pct}`;
};
const verdict = (now, limit) => (now > limit ? "❌ over" : "✅");

/** One table body, for whichever payload's groups and budget lines it is handed. */
function rowsFor(measured, lines) {
  return Object.keys(measured)
    .sort()
    .map((key) => {
      const line = lines[key];
      const actual = measured[key];
      return [
        `\`${key}\``,
        kib(actual.brotli),
        line ? delta(actual.brotli, line.measured_brotli) : "—",
        line ? kib(line.budget_brotli) : "**no budget**",
        line ? verdict(actual.brotli, line.budget_brotli) : "❌ unbudgeted",
      ];
    });
}

const rows = rowsFor(groups, budget.artifacts);

// THE SECOND PAYLOAD (ADR 0026). A page that needs a picture of a page fetches a second worker
// bundle and PDFium with it, so one number stopped describing this build -- and reporting only
// the base one would hide four fifths of what such a page costs, which is the number spike 0004
// was about in the first place.
const renderMeasurement = renderFirstLoad(buildDir, measuredRoute);
const renderOnly = new Set(engineClosures(buildDir).render);
const renderGroups = byBudgetKey({
  entries: renderMeasurement.entries.filter((entry) => renderOnly.has(entry.path)),
});

const table = [
  "| Artifact | brotli | vs recorded | budget | |",
  "|---|--:|--:|--:|:--|",
  ...rows.map((r) => `| ${r.join(" | ")} |`),
  `| **Total first load** | **${kib(measurement.total.brotli)}** | ` +
    `**${delta(measurement.total.brotli, budget.total.measured_brotli)}** | ` +
    `**${kib(budget.total.budget_brotli)}** | **${verdict(measurement.total.brotli, budget.total.budget_brotli)}** |`,
].join("\n");

const renderTable = [
  "| Artifact | brotli | vs recorded | budget | |",
  "|---|--:|--:|--:|:--|",
  ...rowsFor(renderGroups, budget.render.artifacts).map((r) => `| ${r.join(" | ")} |`),
  `| **Total, with rendering** | **${kib(renderMeasurement.total.brotli)}** | ` +
    `**${delta(renderMeasurement.total.brotli, budget.render.total.measured_brotli)}** | ` +
    `**${kib(budget.render.total.budget_brotli)}** | ` +
    `**${verdict(renderMeasurement.total.brotli, budget.render.total.budget_brotli)}** |`,
].join("\n");

const report = [
  "## First-load size budget",
  "",
  "Everything a user downloads before their first operation can run: the page shell, the",
  "worker bundle, every wasm module, and the CSP control file. Brotli, because that is",
  "what a host serves.",
  "",
  `Weighed on the heaviest landing route in this build: \`${measuredRoute}\`.`,
  "",
  table,
  "",
  `Raw (uncompressed, what the browser compiles): **${kib(measurement.total.raw)}**.`,
  "",
  "### And again, for a page that shows page pictures",
  "",
  "PDFium is in a second worker bundle, fetched only when a tool needs to render a page",
  "(ADR 0026). Nobody who merges or splits a file downloads any of this; the rows below are",
  "what it adds on top of the total above, and the last line is what such a page pays",
  "altogether.",
  "",
  renderTable,
  "",
  `Raw, with rendering: **${kib(renderMeasurement.total.raw)}**.`,
  "",
  `_"vs recorded" compares against \`measured_brotli\` in \`apps/web/size-budget.json\`, taken`,
  `on ${budget.measured_on}. The gate is \`apps/web/src/size-budget.test.ts\`, not this report._`,
].join("\n");

console.log(report);
if (process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, report + "\n");
}
