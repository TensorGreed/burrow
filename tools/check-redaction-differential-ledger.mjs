// The browser differential's ledger, reconciled from OUTSIDE the spec (#137).
//
// `apps/web/e2e/redaction-differential.spec.ts` records what the web was observed to reply for
// every case it ran, per browser, and its last test reconciles that record with
// `tests/redaction/outcomes.tsv`. A test inside the spec can be parked -- `test.fail()`, a runtime
// `test.skip` -- and a parked ledger test over a divergence reads as green. This reads the same
// records after the run and makes the same judgement with the same `reconcile`, whatever any test
// was marked. What it cannot be is parked: it is a CI step.
//
// THE BROWSERS ARE PLAYWRIGHT'S OWN LIST, from `playwright test --list --reporter=json`, and the
// ledgers on disk must be EXACTLY that set: a browser with no ledger is refused by name, and so is
// a ledger from a browser the list does not name. The first version read the list from the config
// with a regex, and a review added a project it did not match -- hyphenated, or wrapped over
// several lines -- whose ledger held 107 parked divergences; this printed OK over the other three.
//
// Usage: tools/check-redaction-differential-ledger.sh [--results <dir>] [--spec <basename>]
//          [--projects a,b] [--helper <path>] [--config <path>]
//   The defaults are the real run's: apps/web/test-results, the spec, every project the config
//   runs, the spec's own helper module (whose `DECLARED_SKIPS` are the declarations) and the
//   config itself. The options exist for tools/test-redaction-differential-ledger.sh, which points
//   it at a planted copy's run, and CI passes none. `--projects` narrows to named, configured
//   projects, and the report says so; the results directory must then hold ledgers from those
//   projects only, because any other is refused as one this check does not examine.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  if (at === -1) return undefined;
  const value = args[at + 1];
  if (value === undefined) {
    console.error(`  ${name} needs a value`);
    process.exit(2);
  }
  return value;
};
for (const [i, a] of args.entries()) {
  const known = ["--results", "--spec", "--projects", "--helper", "--config"];
  if (a.startsWith("--") && !known.includes(a)) {
    console.error(`  unknown option ${a}`);
    process.exit(2);
  }
  if (!a.startsWith("--") && !args[i - 1]?.startsWith("--")) {
    console.error(`  unexpected argument ${a}`);
    process.exit(2);
  }
}

const helper = option("--helper");
const { DECLARED_SKIPS, goldenCases, reconcile, repo } = await import(
  helper === undefined
    ? "../apps/web/e2e/redaction-differential.ts"
    : pathToFileURL(resolve(helper)).href
);

const results = resolve(option("--results") ?? join(repo, "apps/web/test-results"));
const spec = option("--spec") ?? "redaction-differential.spec.ts";
const refuse = (why) => {
  console.error(`  REFUSED: ${why}`);
  process.exit(1);
};

// THE PROJECTS PLAYWRIGHT RUNS, as Playwright resolves them. Listing loads the config and the
// specs; it runs no test and no global setup.
const configPath = resolve(option("--config") ?? join(repo, "apps/web/playwright.config.ts"));
const listed = spawnSync(
  "pnpm",
  ["exec", "playwright", "test", "--list", "--reporter=json", "--config", configPath],
  { cwd: join(repo, "apps/web"), encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
);
if (listed.error) refuse(`could not run playwright to list its projects: ${listed.error.message}`);
if (listed.status !== 0) refuse(`playwright could not list its projects: ${listed.stderr.trim()}`);
let configured;
try {
  configured = JSON.parse(listed.stdout).config.projects.map((p) => p.name);
} catch (error) {
  refuse(`playwright's project list is not the JSON this reads: ${error}`);
}
if (configured.length === 0) refuse(`${configPath} configures no project`);
const narrowed = option("--projects")?.split(",");
for (const p of narrowed ?? []) {
  if (!configured.includes(p)) refuse(`--projects names ${p}, which the config does not run`);
}
const projects = narrowed ?? configured;

const expected = goldenCases();
const failures = [];
let examined = 0;

// A LEDGER FROM A BROWSER NOT BEING CHECKED is a run this would otherwise not read.
const directory = join(results, "redaction-differential-ledger");
const prefix = `${spec}.`;
const onDisk = existsSync(directory)
  ? readdirSync(directory)
      .filter((f) => f.startsWith(prefix) && f.endsWith(".jsonl"))
      .map((f) => f.slice(prefix.length, -".jsonl".length))
  : [];
for (const p of onDisk.filter((p) => !projects.includes(p))) {
  failures.push(`${p}: a ledger from a project this check does not examine`);
}
for (const project of projects) {
  const path = join(results, "redaction-differential-ledger", `${spec}.${project}.jsonl`);
  if (!existsSync(path)) {
    failures.push(`${project}: no ledger at ${path}; the differential did not run there`);
    continue;
  }
  examined += 1;
  const recorded = [];
  for (const [n, line] of readFileSync(path, "utf8").split("\n").entries()) {
    if (line === "") continue;
    let record;
    try {
      record = JSON.parse(line);
    } catch {
      failures.push(`${project}: line ${n + 1} of its ledger is not JSON`);
      continue;
    }
    // A RECORD, before `reconcile` destructures it: `null` or a bare value is refused by name
    // rather than as a stack trace.
    if (typeof record !== "object" || record === null || typeof record.name !== "string") {
      failures.push(`${project}: line ${n + 1} of its ledger is not a named record`);
      continue;
    }
    recorded.push(record);
  }
  const { problems, summary } = reconcile(expected, recorded, DECLARED_SKIPS);
  console.log(`${project}: ${summary}`);
  // THE FIRST TWENTY, then the count: a run that dropped everything names every case otherwise.
  for (const p of problems.slice(0, 20)) failures.push(`${project}: ${p}`);
  if (problems.length > 20) failures.push(`${project}: ... and ${problems.length - 20} more`);
}

const scope =
  narrowed === undefined
    ? `${projects.length} configured projects`
    : `${projects.length} projects, NARROWED from ${configured.length} configured`;
console.log(
  `examined ${examined} ledgers of ${scope} (${projects.join(", ")}), ` +
    `against ${expected.size} cases in the golden file`,
);
if (failures.length > 0) {
  for (const f of failures) console.error(`  ${f}`);
  console.error("REFUSED: the differential's ledger does not reconcile with the golden file");
  process.exit(1);
}
console.log("OK: every case in every browser was compared and agreed, or is declared skipped");
