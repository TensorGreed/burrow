#!/usr/bin/env node
// The SIZE LINES of a production build, as a small JSON file (owner (92)(Q2), 2026-10-30; DECISIONS
// rule 18's x86-records change, PR-A).
//
// Rule 18 will compare every base line against main's latest CI artifact at 0%. That needs main's
// figures as measured by CI's x86 build, not as recorded from another machine, and a few hundred
// bytes of JSON is the cheapest thing that carries them: `web-bundles-*` holds only the wasm and JS
// files, so it has no `page` (html, css, fonts), no `engines/control.txt`, and no way to name the
// heaviest route. THIS PR ONLY PRODUCES AND CHECKS THE FILE. Nothing reads it yet; the fetch, the
// 0% compare and the re-record are PR-B.
//
// THE FORMAT (`format: 1`), one object:
//   arch            "x86_64" | "aarch64" | ...  the machine that measured it
//   measured_route  the heaviest landing route the figures were weighed on
//   source          { sha, run_id, workflow, event } from the CI environment, or null off CI
//   lines           one entry per BASE line, by its `size-budget.json` key: the top-level
//                   `artifacts` and `render.artifacts`, which is what a visitor loads:
//                   { raw, brotli, sha256, sha256_normalised }
//   total           { raw, brotli } -- the base payload's first load
//   render_total    { raw, brotli } -- first load of a page that also renders
// Held bundles (`redact.*`) are not here: they are not a base line and are measured from the
// harness build, which is not what this file describes.
//
// MEASURED THE SAME WAY AS THE GATE: `heaviestFirstLoad`, `byBudgetKey`, `digestsByBudgetKey` and
// `renderFirstLoad`, the calls `apps/web/src/size-budget.test.ts` and `report-size-budget.mjs`
// make, so the file cannot describe a different measurement from the one the gate judges.
//
//   node tools/size-lines.mjs write <dist> <out.json>   measure and write
//   node tools/size-lines.mjs check <file.json>          hold a file to size-budget.json's names
//
// `check` is `tools/check-size-lines.sh`'s engine. It reports what it examined and refuses unless
// the file holds EVERY base line the budget names, by name, and nothing else.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  byBudgetKey,
  digestsByBudgetKey,
  engineClosures,
  heaviestFirstLoad,
  renderFirstLoad,
} from "./first-load.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const budgetPath = join(repo, "apps", "web", "size-budget.json");

const ARCH = { x64: "x86_64", arm64: "aarch64" };

/** The base lines `size-budget.json` names, sorted: `artifacts` plus `render.artifacts`. */
export function expectedLineKeys(budget) {
  return [...Object.keys(budget.artifacts), ...Object.keys(budget.render.artifacts)].sort();
}

/**
 * Measure a production `dist/`.
 *
 * @param {string} dist
 * @param {NodeJS.ProcessEnv} env
 */
export function measureSizeLines(dist, env = process.env) {
  const heaviest = heaviestFirstLoad(dist);
  const groups = byBudgetKey(heaviest.measurement);
  const renderMeasurement = renderFirstLoad(dist, heaviest.page);
  const renderOnly = new Set(engineClosures(dist).render);
  const renderGroups = byBudgetKey({
    entries: renderMeasurement.entries.filter((entry) => renderOnly.has(entry.path)),
  });
  const lines = {};
  for (const [set, digests] of [
    [groups, digestsByBudgetKey(dist, groups)],
    [renderGroups, digestsByBudgetKey(dist, renderGroups)],
  ]) {
    for (const [key, group] of Object.entries(set)) {
      lines[key] = {
        raw: group.raw,
        brotli: group.brotli,
        sha256: digests[key].raw,
        sha256_normalised: digests[key].normalised,
      };
    }
  }
  return {
    format: 1,
    arch: ARCH[process.arch] ?? process.arch,
    measured_route: heaviest.page,
    source: env.GITHUB_SHA
      ? {
          sha: env.GITHUB_SHA,
          run_id: env.GITHUB_RUN_ID ?? null,
          workflow: env.GITHUB_WORKFLOW ?? null,
          event: env.GITHUB_EVENT_NAME ?? null,
        }
      : null,
    lines: Object.fromEntries(Object.entries(lines).sort(([a], [b]) => (a < b ? -1 : 1))),
    total: { raw: heaviest.measurement.total.raw, brotli: heaviest.measurement.total.brotli },
    render_total: {
      raw: renderMeasurement.total.raw,
      brotli: renderMeasurement.total.brotli,
    },
  };
}

const isCount = (n) => Number.isSafeInteger(n) && n > 0;
const isHex64 = (s) => typeof s === "string" && /^[0-9a-f]{64}$/.test(s);

/**
 * Every reason a file is not a complete size-lines record. Pure, so the self-test feeds it plants.
 *
 * @param {any} doc parsed JSON
 * @param {string[]} expected the base-line names the budget records
 * @param {string[]} baseKeys the subset that is the base payload (`artifacts`, not `render.artifacts`)
 * @returns {{problems: string[], examined: string[]}}
 */
export function sizeLinesProblems(doc, expected, baseKeys = []) {
  const problems = [];
  if (doc === null || typeof doc !== "object" || Array.isArray(doc)) {
    return { problems: ["not a JSON object"], examined: [] };
  }
  if (doc.format !== 1) problems.push(`format is ${JSON.stringify(doc.format)}, expected 1`);
  if (typeof doc.arch !== "string" || doc.arch === "") problems.push("arch is missing");
  if (typeof doc.measured_route !== "string" || doc.measured_route === "") {
    problems.push("measured_route is missing");
  }
  if (!("source" in doc))
    problems.push("source is missing (null is allowed off CI, absent is not)");
  const lines = doc.lines && typeof doc.lines === "object" ? doc.lines : null;
  if (lines === null) {
    problems.push("lines is missing");
    return { problems, examined: [] };
  }
  const have = Object.keys(lines).sort();
  const absent = expected.filter((key) => !have.includes(key));
  const extra = have.filter((key) => !expected.includes(key));
  if (absent.length > 0) problems.push(`lines lacks ${absent.join(", ")}`);
  if (extra.length > 0) problems.push(`lines has unexpected ${extra.join(", ")}`);
  const examined = [];
  for (const key of expected.filter((k) => have.includes(k))) {
    const line = lines[key];
    const bad = [];
    if (!isCount(line?.raw)) bad.push("raw");
    if (!isCount(line?.brotli)) bad.push("brotli");
    if (!isHex64(line?.sha256)) bad.push("sha256");
    if (!isHex64(line?.sha256_normalised)) bad.push("sha256_normalised");
    if (bad.length > 0) problems.push(`line ${key} has a missing or malformed ${bad.join(", ")}`);
    else examined.push(key);
  }
  for (const name of ["total", "render_total"]) {
    if (!isCount(doc[name]?.raw) || !isCount(doc[name]?.brotli)) {
      problems.push(`${name} has a missing or malformed raw/brotli`);
    }
  }
  // THE TOTALS ARE THE SUMS, measured 2026-10-31 to the byte: the base total is the sum of the
  // top-level lines, and the render total is the sum of all of them. A file whose lines and totals
  // disagree was assembled, not measured. Only judged when every line is well-formed.
  if (examined.length === expected.length) {
    for (const [name, keys] of [
      ["total", baseKeys],
      ["render_total", expected],
    ]) {
      for (const field of ["raw", "brotli"]) {
        const sum = keys.reduce((n, key) => n + lines[key][field], 0);
        if (isCount(doc[name]?.[field]) && doc[name][field] !== sum) {
          problems.push(`${name}.${field} is ${doc[name][field]} but its lines sum to ${sum}`);
        }
      }
    }
  }
  return { problems, examined };
}

function main(argv) {
  const [mode, ...rest] = argv;
  if (mode === "write" && rest.length === 2) {
    const doc = measureSizeLines(resolve(rest[0]));
    mkdirSync(dirname(resolve(rest[1])), { recursive: true });
    writeFileSync(resolve(rest[1]), JSON.stringify(doc, null, 2) + "\n");
    console.error(
      `size-lines: wrote ${Object.keys(doc.lines).length} line(s) + 2 totals to ${rest[1]} ` +
        `(arch ${doc.arch}, route ${doc.measured_route}).`,
    );
    return 0;
  }
  if (mode === "check" && rest.length === 1) {
    const budget = JSON.parse(readFileSync(budgetPath, "utf8"));
    const expected = expectedLineKeys(budget);
    let doc;
    try {
      doc = JSON.parse(readFileSync(resolve(rest[0]), "utf8"));
    } catch (error) {
      console.error(`size-lines: REFUSED -- cannot read ${rest[0]} as JSON: ${error.message}`);
      return 1;
    }
    const { problems, examined } = sizeLinesProblems(doc, expected, Object.keys(budget.artifacts));
    console.error(
      `size-lines: examined ${examined.length} of ${expected.length} expected base line(s)` +
        `${examined.length > 0 ? `: ${examined.join(", ")}` : ""}; totals ` +
        `${isCount(doc?.total?.brotli) && isCount(doc?.render_total?.brotli) ? "2 of 2" : "not all"} present.`,
    );
    if (problems.length > 0) {
      for (const p of problems) console.error(`size-lines: REFUSED -- ${p}`);
      return 1;
    }
    console.error("size-lines: OK");
    return 0;
  }
  console.error("usage: size-lines.mjs write <dist> <out.json> | size-lines.mjs check <file.json>");
  return 2;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
