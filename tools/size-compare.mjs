#!/usr/bin/env node
// This build's size lines against main's latest CI artifact, at 0% (DECISIONS rule 18, owner
// (85)(3), (87), (90)(2), (92); PR-B).
//
// THE RULE. Size records are taken from CI's x86_64 build, the one that ships. CI compares every
// base line of the build it just made with the same line of main's latest `web-size-lines`
// artifact (`tools/fetch-main-size-lines.sh` fetches it), raw bytes, brotli bytes and sha256, at 0%:
// any movement against main is a re-record, and the PR names the approval. Held bundles (`redact.*`)
// are not here: they keep `drift_tolerance`. The per-line budgets and the headroom policy are
// `apps/web/src/size-budget.test.ts`'s and unchanged.
//
// WHAT "A RE-RECORD" MEANS HERE, said precisely because it is the part that could be hollow. A line
// that moved against main's artifact is accepted only if this tree's `size-budget.json` records it
// DIFFERENTLY from main's recording at the artifact's commit (raw, brotli or sha256). Whether the
// new recording is the right number is the size test's job, which holds the build to the recording
// (exact on `measured_arch`, loose elsewhere); this check is what stops growth that nobody recorded.
// The loose local check alone could let a +1 KiB through unrecorded; this closes it.
//
// THE WITNESS (A). When main's recording was measured on the artifact's architecture, every line
// of that recording must equal main's artifact at 0 bytes, or the record is not the build that
// ships. While the recording is another architecture's the witness cannot run, and says so by
// name; it is a SKIP that is printed, never a silent pass, and it starts to run the day
// `measured_arch` is x86_64 with no further change here.
//
// NEVER A PASS ON ABSENCE. The artifact must hold exactly the lines main's recording names, main's
// recording must be readable at the artifact's own commit, the artifact must say it came from that
// commit and run, and both sides must be the same architecture. Anything else REFUSES.
//
//   node tools/size-compare.mjs compare <build.json> <main-dir>
//
// `<main-dir>` is what `tools/fetch-main-size-lines.sh --out` writes: `size-lines.json`,
// `size-budget.json` (main's recording at the artifact's commit) and `meta.json`.

import { readFileSync, appendFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expectedLineKeys, sizeLinesProblems } from "./size-lines.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** The fields a line is held to, in the artifact (`a`) and in a recording (`r`). */
const LINE_FIELDS = [
  ["raw", "measured_raw"],
  ["brotli", "measured_brotli"],
  ["sha256", "measured_sha256"],
];
const TOTAL_FIELDS = [
  ["raw", "measured_raw"],
  ["brotli", "measured_brotli"],
];
const EVENTS = ["push", "schedule"];

const recLine = (budget, key) => budget.artifacts?.[key] ?? budget.render?.artifacts?.[key];

/** Does a recording entry differ from another in any field a line is held to? */
function recordedDiffers(a, b, fields) {
  if (a === undefined || b === undefined) return a !== b;
  return fields.some(([, field]) => a[field] !== b[field]);
}

/**
 * Compare. Pure, so the self-test plants every input.
 *
 * @param {object} p
 * @param {any} p.build this build's size-lines document
 * @param {any} p.main main's artifact
 * @param {any} p.meta what the fetch recorded: { run_id, head_sha, event }
 * @param {any} p.tree this tree's size-budget.json
 * @param {any} p.mainBudget main's size-budget.json at the artifact's commit
 * @returns {{problems: string[], rows: object[], counts: object, witness: string}}
 */
export function compareSizeLines({ build, main, meta, tree, mainBudget }) {
  const problems = [];
  const treeKeys = expectedLineKeys(tree);
  const mainKeys = expectedLineKeys(mainBudget);

  // Both documents are held to the names of the recording they belong to.
  for (const [label, doc, keys, budget] of [
    ["this build", build, treeKeys, tree],
    ["main's artifact", main, mainKeys, mainBudget],
  ]) {
    const { problems: found } = sizeLinesProblems(doc, keys, Object.keys(budget.artifacts));
    for (const p of found) problems.push(`${label}: ${p}`);
  }
  if (problems.length > 0) {
    return { problems, rows: [], counts: emptyCounts(treeKeys), witness: "not run" };
  }

  // THE ARTIFACT IS WHAT IT SAYS IT IS: from the commit and run the fetch found, on a main event.
  const src = main.source;
  if (src === null || typeof src !== "object") {
    problems.push("main's artifact has no source (it was not written by CI)");
  } else {
    if (src.sha !== meta.head_sha) {
      problems.push(`main's artifact says it is from ${src.sha}, but the run is ${meta.head_sha}`);
    }
    if (String(src.run_id) !== String(meta.run_id)) {
      problems.push(`main's artifact says run ${src.run_id}, but the fetch took run ${meta.run_id}`);
    }
  }
  if (!EVENTS.includes(meta.event)) {
    problems.push(`main's artifact is from a '${meta.event}' run; only ${EVENTS.join(" and ")} count`);
  }
  if (build.arch !== main.arch) {
    problems.push(
      `this build is ${build.arch} and main's artifact is ${main.arch}: sizes are not comparable ` +
        `across architectures, so nothing was compared`,
    );
  }
  if (problems.length > 0) {
    return { problems, rows: [], counts: emptyCounts(treeKeys), witness: "not run" };
  }

  // THE WITNESS (A).
  let witness;
  if (mainBudget.measured_arch === main.arch) {
    const off = [];
    for (const key of mainKeys) {
      const rec = recLine(mainBudget, key);
      const art = main.lines[key];
      const differs = [...LINE_FIELDS, ["sha256_normalised", "measured_sha256_normalised"]]
        .filter(([f, r]) => art[f] !== rec[r])
        .map(([f]) => f);
      if (differs.length > 0) off.push(`${key} (${differs.join(", ")})`);
    }
    for (const [name, rec] of [
      ["total", mainBudget.total],
      ["render_total", mainBudget.render?.total],
    ]) {
      const differs = TOTAL_FIELDS.filter(([f, r]) => main[name][f] !== rec?.[r]).map(([f]) => f);
      if (differs.length > 0) off.push(`${name} (${differs.join(", ")})`);
    }
    if (off.length > 0) {
      problems.push(
        `main's recording at ${meta.head_sha} is not its own ${main.arch} artifact: ` +
          `${off.join("; ")}. The record is not the build that ships; re-record from the artifact.`,
      );
    }
    witness = `ran: ${mainKeys.length} of ${mainKeys.length} lines + 2 of 2 totals, recording == artifact`;
  } else {
    witness =
      `SKIPPED: main's recording was measured on ${mainBudget.measured_arch} and its artifact on ` +
      `${main.arch}, so they are not expected to match yet`;
  }

  // THE ROUTE: the page line weighs the heaviest route, and a different route is a different line.
  const routeMoved = build.measured_route !== main.measured_route;
  if (
    routeMoved &&
    tree.artifacts?.page?.measured_route === mainBudget.artifacts?.page?.measured_route
  ) {
    problems.push(
      `the heaviest route moved from ${main.measured_route} to ${build.measured_route} against ` +
        `main, and size-budget.json's page.measured_route was not re-recorded`,
    );
  }

  const rows = [];
  const judge = (key, built, mainLine, treeRec, mainRec, fields) => {
    if (built === undefined && mainLine === undefined) return;
    if (mainLine === undefined) {
      rows.push({ key, kind: "new", built, main: null });
      return;
    }
    if (built === undefined) {
      rows.push({ key, kind: "removed", built: null, main: mainLine });
      return;
    }
    const moved = fields.filter(([f]) => built[f] !== mainLine[f]).map(([f]) => f);
    if (moved.length === 0) {
      rows.push({ key, kind: "same", built, main: mainLine });
      return;
    }
    const recorded = recordedDiffers(treeRec, mainRec, fields);
    rows.push({
      key,
      kind: recorded ? "moved, re-recorded" : "moved, NOT re-recorded",
      moved,
      built,
      main: mainLine,
    });
    if (!recorded) {
      problems.push(
        `${key} moved against main's artifact (${moved.join(", ")}: brotli ` +
          `${mainLine.brotli} -> ${built.brotli}) and this tree's size-budget.json does not ` +
          `record it differently from main's. Base lines are held at 0%: re-record it, and name ` +
          `the approval in the PR body (DECISIONS rule 18).`,
      );
    }
  };

  const allKeys = [...new Set([...treeKeys, ...mainKeys])].sort();
  for (const key of allKeys) {
    judge(
      key,
      build.lines[key],
      main.lines[key],
      recLine(tree, key),
      recLine(mainBudget, key),
      LINE_FIELDS,
    );
  }
  for (const [name, treeRec, mainRec] of [
    ["total", tree.total, mainBudget.total],
    ["render_total", tree.render?.total, mainBudget.render?.total],
  ]) {
    judge(name, build[name], main[name], treeRec, mainRec, TOTAL_FIELDS);
  }

  const lineRows = rows.filter((r) => r.key !== "total" && r.key !== "render_total");
  const counts = {
    expected: treeKeys.length,
    // Every line this tree names was compared with main's, or is NEW and says so.
    examined: lineRows.filter((r) => r.kind !== "new" && r.kind !== "removed").length,
    added: lineRows.filter((r) => r.kind === "new").map((r) => r.key),
    removed: lineRows.filter((r) => r.kind === "removed").map((r) => r.key),
    totals: rows.filter((r) => r.key === "total" || r.key === "render_total").length,
  };
  return { problems, rows, counts, witness };
}

function emptyCounts(treeKeys) {
  return { expected: treeKeys.length, examined: 0, added: [], removed: [], totals: 0 };
}

const signed = (n) => (n === 0 ? "0" : `${n > 0 ? "+" : "-"}${Math.abs(n).toLocaleString("en")}`);

/** The comparison as a markdown table, in bytes. */
export function compareTable(rows) {
  const cell = (n) => (n === null || n === undefined ? "-" : n.toLocaleString("en"));
  return [
    "| Line | main (artifact) brotli | this build brotli | build vs main | verdict |",
    "|---|--:|--:|--:|:--|",
    ...rows.map(
      (r) =>
        `| \`${r.key}\` | ${cell(r.main?.brotli)} | ${cell(r.built?.brotli)} | ` +
        `${r.main && r.built ? signed(r.built.brotli - r.main.brotli) : "-"} | ${r.kind} |`,
    ),
  ].join("\n");
}

function readJson(path, what) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Error(`cannot read ${what} (${path}): ${error.message}`);
  }
}

function main(argv) {
  const [mode, buildPath, mainDir] = argv;
  if (mode !== "compare" || !buildPath || !mainDir || argv.length !== 3) {
    console.error("usage: size-compare.mjs compare <build.json> <main-dir>");
    return 2;
  }
  let inputs;
  try {
    inputs = {
      build: readJson(resolve(buildPath), "this build's size lines"),
      main: readJson(join(resolve(mainDir), "size-lines.json"), "main's artifact"),
      meta: readJson(join(resolve(mainDir), "meta.json"), "the fetch's meta.json"),
      mainBudget: readJson(join(resolve(mainDir), "size-budget.json"), "main's recording"),
      tree: readJson(join(repo, "apps", "web", "size-budget.json"), "this tree's size-budget.json"),
    };
  } catch (error) {
    console.error(`size-compare: REFUSED -- ${error.message}`);
    return 1;
  }
  const { problems, rows, counts, witness } = compareSizeLines(inputs);
  const origin = `artifact from run ${inputs.meta.run_id}, sha ${inputs.meta.head_sha}, ${inputs.meta.event}`;
  const summary =
    `examined ${counts.examined} of ${counts.expected} base line(s)` +
    `${counts.added.length > 0 ? `, ${counts.added.length} new (${counts.added.join(", ")})` : ""}` +
    `${counts.removed.length > 0 ? `, ${counts.removed.length} removed (${counts.removed.join(", ")})` : ""}` +
    ` and ${counts.totals} of 2 totals at 0% against main; ${origin}. Witness A ${witness}.`;
  console.error(`size-compare: ${summary}`);
  if (rows.length > 0) {
    const table = compareTable(rows);
    console.error(table);
    if (process.env.GITHUB_STEP_SUMMARY) {
      appendFileSync(
        process.env.GITHUB_STEP_SUMMARY,
        `### Base lines against main's latest CI artifact, at 0%\n\n${table}\n\n_${summary}_\n`,
      );
    }
  }
  if (problems.length > 0) {
    for (const p of problems) console.error(`size-compare: REFUSED -- ${p}`);
    return 1;
  }
  console.error("size-compare: OK");
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
