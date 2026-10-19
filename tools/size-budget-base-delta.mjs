// The per-line brotli delta of every BASE line against main (owner, 2026-10-18).
//
// A base line is anything a visitor loads (DECISIONS rule 18's 2026-10-18 amendment): a
// top-level `artifacts` entry of `apps/web/size-budget.json`, or a `render.artifacts` one. The question a reviewer asks of a PR about those lines is "which one moved, by
// how many bytes, against main" -- and the existing table answers a different one ("against this
// tree's own recording", in KiB to one decimal, which rounds a 40-byte growth to "+0.0 KiB").
//
// Pure, so the size test can feed it planted inputs on every run. The REPORT that prints it is
// `tools/report-size-budget.mjs`; it refuses when the count examined is not the count expected.
//
// WHAT IT COMPARES, said plainly. Main's figure is main's RECORDING (`measured_brotli` in main's
// copy of the file), not a build of main: there is no baseline build in the job. So the live
// column carries any difference between the machine that recorded main and the one running this
// -- measured on 2026-10-18 (main's CI run 39184360337), CI's x86_64 build of an unchanged tree
// sat +0.1% on `burrow_wasm_bg.wasm` and on `burrow-worker.js`, tens of bytes each, against an
// aarch64 recording. The "recorded" column (this tree's recording minus main's) has no such
// noise: it is exactly what the PR re-records.

/**
 * @param {Record<string, {measured_brotli: number}>} mainLines main's `artifacts`
 * @param {Record<string, {measured_brotli: number}>} treeLines this tree's `artifacts`
 * @param {Record<string, {brotli: number}>} live this build's groups, by budget key
 * @returns {{rows: Array<{key: string, main: number|null, recorded: number|null, live: number|null}>,
 *            examined: string[], expected: string[], missing: string[]}}
 */
export function baseLineDeltas(mainLines, treeLines, live) {
  // EXPECTED is this tree's base lines: every one of them must be compared with main. A line
  // new in this tree has no main figure, so it is reported and NOT counted as examined -- which
  // makes the count say so, rather than a fresh line passing as compared.
  const expected = Object.keys(treeLines).sort();
  const keys = [...new Set([...expected, ...Object.keys(mainLines), ...Object.keys(live)])].sort();
  const rows = keys.map((key) => ({
    key,
    main: mainLines[key]?.measured_brotli ?? null,
    recorded: treeLines[key]?.measured_brotli ?? null,
    live: live[key]?.brotli ?? null,
  }));
  const examined = expected.filter(
    (key) => mainLines[key] !== undefined && live[key] !== undefined,
  );
  const missing = expected.filter((key) => !examined.includes(key));
  return { rows, examined, expected, missing };
}

const signed = (n) => (n === 0 ? "0" : `${n > 0 ? "+" : "−"}${Math.abs(n).toLocaleString("en")}`);

/** The rows as a markdown table, in bytes: at 0% drift a byte is the unit that matters. */
export function baseLineDeltaTable({ rows }) {
  const cell = (n) => (n === null ? "—" : n.toLocaleString("en"));
  const diff = (a, b) => (a === null || b === null ? "—" : signed(a - b));
  return [
    "| Base line | main recorded | this tree recorded | recorded vs main | this build | build vs main |",
    "|---|--:|--:|--:|--:|--:|",
    ...rows.map(
      (r) =>
        `| \`${r.key}\` | ${cell(r.main)} | ${cell(r.recorded)} | ${diff(r.recorded, r.main)} | ` +
        `${cell(r.live)} | ${diff(r.live, r.main)} |`,
    ),
  ].join("\n");
}
