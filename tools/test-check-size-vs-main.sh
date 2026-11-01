#!/usr/bin/env bash
# Adversarial self-test for tools/check-size-vs-main.sh and tools/size-compare.mjs (DECISIONS rule 18,
# PR-B).
#
# EVERY RULE HAS A PROBE ON EVERY RUN: a plant that breaks exactly that rule must be refused NAMING
# the reason, and its near-miss twin (the same plant with the rule satisfied, or legal but unusual)
# must pass. Then each rule is broken in a COPY of the engine, beside the original so `./size-lines.mjs`
# resolves, and the copy must stop refusing that plant (or, for the count, stop passing its twin): that
# is what shows the plant exercises the rule rather than tripping another. Every mutation asserts it
# applied first. The numbers printed are measured, not claimed: the count of cases, the line counts
# read from the real size-budget.json, and the end-to-end run through the real script.
#
# The plants go through `compareSizeLines`, the pure function the script calls, so every input is
# planted -- both artifacts, the recording at main's commit, this tree's recording. The end-to-end
# half drives the real script on files on disk.
#
# Usage: tools/test-check-size-vs-main.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-size-vs-main.sh"
engine="$here/size-compare.mjs"
mutant="$here/size-compare.selftest-mutant.mjs"
workroot="$(mktemp -d)"
probe="$workroot/probe.mjs"

cleanup() {
  trap - EXIT INT TERM HUP
  rm -f "$mutant"
  rm -rf "$workroot"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# --- the probe: a context built from the real budget's names, a case planted over it ----------------
cat >"$probe" <<PROBE
import { readFileSync } from "node:fs";
import { expectedLineKeys } from "$here/size-lines.mjs";
const [module, name] = process.argv.slice(2);
const { compareSizeLines } = await import(module);
const real = JSON.parse(readFileSync("$repo/apps/web/size-budget.json", "utf8"));
const hex = (c) => c.repeat(64);
const SHA = "0123456789abcdef0123456789abcdef01234567";
const clone = (x) => JSON.parse(JSON.stringify(x));
const keys = expectedLineKeys(real);
const baseKeys = Object.keys(real.artifacts);
const K = baseKeys[0];
const R = Object.keys(real.render.artifacts)[0];
const sums = (lines, ks) => ({
  raw: ks.reduce((n, k) => n + lines[k].raw, 0),
  brotli: ks.reduce((n, k) => n + lines[k].brotli, 0),
});
function fresh() {
  const lines = Object.fromEntries(
    keys.map((k, i) => [k, { raw: 1000 + i, brotli: 500 + i, sha256: hex("a"), sha256_normalised: hex("b") }]),
  );
  const build = {
    format: 1, arch: "x86_64", measured_route: "split-pdf/index.html", source: null, lines,
    total: sums(lines, baseKeys), render_total: sums(lines, keys),
  };
  const main = clone(build);
  main.source = { sha: SHA, run_id: "123", workflow: "CI", event: "push" };
  const meta = { run_id: "123", head_sha: SHA, event: "push" };
  const mainBudget = clone(real);
  for (const k of keys) {
    const rec = mainBudget.artifacts[k] ?? mainBudget.render.artifacts[k];
    Object.assign(rec, {
      measured_raw: lines[k].raw, measured_brotli: lines[k].brotli,
      measured_sha256: lines[k].sha256, measured_sha256_normalised: lines[k].sha256_normalised,
    });
  }
  mainBudget.artifacts.page.measured_route = "split-pdf/index.html";
  mainBudget.total.measured_raw = build.total.raw; mainBudget.total.measured_brotli = build.total.brotli;
  mainBudget.render.total.measured_raw = build.render_total.raw;
  mainBudget.render.total.measured_brotli = build.render_total.brotli;
  mainBudget.measured_arch = "x86_64"; // the witness runs unless a case says otherwise
  return { build, main, meta, tree: clone(mainBudget), mainBudget };
}
const recOf = (b, k) => b.artifacts[k] ?? b.render.artifacts[k];
// A real change to a line of the build, totals following (a base line is in both totals).
function grow(c, k, field, d) {
  c.build.lines[k][field] += d;
  if (field === "raw" || field === "brotli") {
    if (baseKeys.includes(k)) c.build.total[field] += d;
    c.build.render_total[field] += d;
  }
}
function shaChange(c, k) { c.build.lines[k].sha256 = hex("f"); }
const rerecord = (c, k) => {
  const l = c.build.lines[k], r = recOf(c.tree, k);
  Object.assign(r, { measured_raw: l.raw, measured_brotli: l.brotli, measured_sha256: l.sha256 });
};
const rerecordTotals = (c) => {
  c.tree.total.measured_raw = c.build.total.raw; c.tree.total.measured_brotli = c.build.total.brotli;
  c.tree.render.total.measured_raw = c.build.render_total.raw;
  c.tree.render.total.measured_brotli = c.build.render_total.brotli;
};
const cases = {
  baseline: () => {},
  "grow K, unrecorded": (c) => grow(c, K, "brotli", 1),
  "grow K, recorded": (c) => { grow(c, K, "brotli", 1); rerecord(c, K); rerecordTotals(c); },
  "grow K raw only, unrecorded": (c) => grow(c, K, "raw", 1),
  "grow K raw only, recorded": (c) => { grow(c, K, "raw", 1); rerecord(c, K); rerecordTotals(c); },
  "K sha only, unrecorded": (c) => shaChange(c, K),
  "K sha only, recorded": (c) => { shaChange(c, K); rerecord(c, K); },
  "grow R, unrecorded": (c) => grow(c, R, "brotli", 1),
  "grow R, recorded": (c) => { grow(c, R, "brotli", 1); rerecord(c, R); rerecordTotals(c); },
  "grow K +500, record +1": (c) => { grow(c, K, "brotli", 500); recOf(c.tree, K).measured_brotli += 1; rerecordTotals(c); },
  "grow K +500, record exactly": (c) => { grow(c, K, "brotli", 500); rerecord(c, K); rerecordTotals(c); },
  "grow K raw and brotli, record brotli only": (c) => { grow(c, K, "raw", 7); grow(c, K, "brotli", 7); recOf(c.tree, K).measured_brotli = c.build.lines[K].brotli; rerecordTotals(c); },
  "grow K +500, record -1 (below main)": (c) => { grow(c, K, "brotli", 500); recOf(c.tree, K).measured_brotli -= 1; rerecordTotals(c); },
  "K recorded, totals not": (c) => { grow(c, K, "brotli", 1); rerecord(c, K); },
  "R recorded, render total not": (c) => { grow(c, R, "brotli", 1); rerecord(c, R); c.tree.total = clone(c.mainBudget.total); },
  "build is aarch64": (c) => { c.build.arch = "aarch64"; },
  "artifact sha differs from the run": (c) => { c.main.source.sha = "f".repeat(40); },
  "artifact run id differs from the fetch": (c) => { c.main.source.run_id = "999"; },
  "artifact has no source": (c) => { c.main.source = null; },
  "run event is pull_request": (c) => { c.meta.event = "pull_request"; },
  "run event is schedule": (c) => { c.meta.event = "schedule"; },
  "artifact lacks a line": (c) => { delete c.main.lines[K]; },
  "build lacks a line": (c) => { delete c.build.lines[K]; },
  "witness: recording is 1 byte off its artifact": (c) => { recOf(c.mainBudget, K).measured_brotli += 1; recOf(c.tree, K).measured_brotli += 1; },
  "witness: only the normalised digest is off": (c) => { recOf(c.mainBudget, K).measured_sha256_normalised = hex("c"); },
  "witness: recording is another arch, numbers differ": (c) => { c.mainBudget.measured_arch = "aarch64"; recOf(c.mainBudget, K).measured_brotli += 7; recOf(c.tree, K).measured_brotli += 7; },
  "route moved, not re-recorded": (c) => { c.build.measured_route = "merge-pdf/index.html"; },
  "route moved, re-recorded": (c) => { c.build.measured_route = "merge-pdf/index.html"; c.tree.artifacts.page.measured_route = "merge-pdf/index.html"; },
  "a line is new in this tree": (c) => {
    const key = "engines/zzz-new.wasm";
    c.build.lines[key] = { raw: 50, brotli: 40, sha256: hex("d"), sha256_normalised: hex("d") };
    c.build.total.raw += 50; c.build.total.brotli += 40; c.build.render_total.raw += 50; c.build.render_total.brotli += 40;
    c.tree.artifacts[key] = { measured_raw: 50, measured_brotli: 40, measured_sha256: hex("d"), measured_sha256_normalised: hex("d") };
    rerecordTotals(c);
  },
  "a line is gone from this tree": (c) => {
    const l = c.build.lines[K];
    delete c.build.lines[K]; delete c.tree.artifacts[K];
    c.build.total.raw -= l.raw; c.build.total.brotli -= l.brotli; c.build.render_total.raw -= l.raw; c.build.render_total.brotli -= l.brotli;
    rerecordTotals(c);
  },
};
if (!(name in cases)) { console.log("NO SUCH CASE " + name); process.exit(3); }
const c = fresh();
cases[name](c);
const r = compareSizeLines(c);
const out = [...r.problems.map((p) => "PROBLEM " + p)];
out.push("COUNTS examined=" + r.counts.examined + " expected=" + r.counts.expected + " added=" + r.counts.added.length + " removed=" + r.counts.removed.length + " totals=" + r.counts.totals);
out.push("WITNESS " + r.witness);
for (const row of r.rows) out.push("ROW " + row.key + " | " + row.kind);
if (r.problems.length === 0 && r.counts.examined + r.counts.added.length !== r.counts.expected) out.push("COUNT-MISMATCH examined plus new is not expected");
console.log(out.join("\n"));
PROBE

# run <module> <case> -> the probe's output on stdout (never fails the script)
run() { node "$probe" "$1" "$2" 2>&1 || true; }
refused() { grep -q '^PROBLEM ' <<<"$1"; }

# `refuses <name> <case> <expect>`: refused, naming the reason.
refuses() {
  local name="$1" case="$2" expect="$3" out
  out="$(run "$engine" "$case")"
  if ! refused "$out"; then bad "$name: it did not refuse"; return; fi
  if ! grep -qF -- "$expect" <<<"$out"; then
    bad "$name: refused, but not for the stated reason (wanted: $expect)"; sed 's/^/        /' <<<"$out" | head -4; return
  fi
  ok "$name"
}
# `accepts <name> <case> [<expect text in the output>]`: the near-miss twin passes.
accepts() {
  local name="$1" case="$2" expect="${3:-}" out
  out="$(run "$engine" "$case")"
  if refused "$out" || grep -q 'COUNT-MISMATCH\|NO SUCH CASE\|^Error\|TypeError' <<<"$out"; then
    bad "$name: the near-miss was refused"; sed 's/^/        /' <<<"$out" | head -4; return
  fi
  if [ -n "$expect" ] && ! grep -qF -- "$expect" <<<"$out"; then
    bad "$name: passed, but the output lacks '$expect'"; sed 's/^/        /' <<<"$out" | head -6; return
  fi
  ok "$name"
}

expected_n="$(node --input-type=module -e '
import { readFileSync } from "node:fs";
import { expectedLineKeys } from "'"$here"'/size-lines.mjs";
console.log(expectedLineKeys(JSON.parse(readFileSync("'"$repo"'/apps/web/size-budget.json", "utf8"))).length);')"
if [ "${expected_n:-0}" -lt 1 ]; then echo "FAILED: could not read the expected line count" >&2; exit 1; fi
# The two lines the probes plant on, named as the probe names them (a base line and a render line), so a
# refusal is held to the LINE'S own message and not to the total's, which carries the same field names.
K="$(node -e 'console.log(Object.keys(JSON.parse(require("fs").readFileSync(process.argv[1])).artifacts)[0])' "$repo/apps/web/size-budget.json")"
R="$(node -e 'console.log(Object.keys(JSON.parse(require("fs").readFileSync(process.argv[1])).render.artifacts)[0])' "$repo/apps/web/size-budget.json")"
case "$K" in engines/*) ;; *) echo "FAILED: the base probe line is $K" >&2; exit 1 ;; esac
case "$R" in engines/*) ;; *) echo "FAILED: the render probe line is $R" >&2; exit 1 ;; esac

echo "per-rule probes (each: the plant is refused naming its reason, the twin passes) -- $expected_n base lines expected"

accepts "the synthetic baseline passes, so a refusal below means something" baseline "COUNTS examined=$expected_n expected=$expected_n added=0 removed=0 totals=2"
accepts "...and its witness RAN (recording and artifact on one arch)" baseline "WITNESS ran: $expected_n of $expected_n lines"

# R1: a line that moved and was not re-recorded is refused; recorded, it passes (each field alone).
refuses "R1 a base line +1 brotli, not re-recorded" "grow K, unrecorded" "$K moved against main's artifact (brotli: brotli"
accepts "R1 twin: the same line, re-recorded" "grow K, recorded" "| moved, re-recorded"
refuses "R1 a base line +1 raw only, not re-recorded" "grow K raw only, unrecorded" "$K moved against main's artifact (raw: brotli"
accepts "R1 twin: raw only, re-recorded" "grow K raw only, recorded"
refuses "R1 a base line whose bytes changed at the same size (sha256 only)" "K sha only, unrecorded" "$K moved against main's artifact (sha256: brotli"
accepts "R1 twin: sha256 only, re-recorded" "K sha only, recorded"
refuses "R1 a render line +1 brotli, not re-recorded" "grow R, unrecorded" "$R moved against main's artifact (brotli: brotli"
accepts "R1 twin: the render line, re-recorded" "grow R, recorded"
# R1b: a re-record is the build's own number, not any edit (the review's SF-1).
refuses "R1b a base line +500 B, recorded as +1 (an edit, not the build)" "grow K +500, record +1" "does not record this build's value for it (brotli: recorded"
accepts "R1b twin: +500 B, recorded exactly" "grow K +500, record exactly" "| moved, re-recorded"
refuses "R1b a recording below main's, still not the build" "grow K +500, record -1 (below main)" "does not record this build's value for it (brotli: recorded"
refuses "R1b two fields moved, only one recorded as built" "grow K raw and brotli, record brotli only" "does not record this build's value for them (raw: recorded"
# R2: totals are held the same way.
refuses "R2 the base total moved, not re-recorded" "K recorded, totals not" "total moved against main's artifact"
refuses "R2 the render total moved, not re-recorded" "R recorded, render total not" "render_total moved against main's artifact"
accepts "R2 twin: totals re-recorded with their line" "grow K, recorded"
# R3: nothing is compared across architectures.
refuses "R3 an aarch64 build against an x86_64 artifact" "build is aarch64" "not comparable across architectures"
# R4/R5: the artifact is what it says it is.
refuses "R4 the artifact's sha is not the run's" "artifact sha differs from the run" "says it is from"
refuses "R4 the artifact's run is not the one fetched" "artifact run id differs from the fetch" "says run"
refuses "R4 an artifact with no source" "artifact has no source" "has no source"
refuses "R5 a pull_request run is not a baseline" "run event is pull_request" "only push and schedule count"
accepts "R5 twin: a schedule run is" "run event is schedule"
# R6: names. The artifact holds exactly main's recording's lines; the build exactly this tree's.
refuses "R6 an artifact that lacks a line" "artifact lacks a line" "main's artifact: lines lacks"
refuses "R6 a build that lacks a line" "build lacks a line" "this build: lines lacks"
# R7: the witness.
refuses "R7 main's recording is 1 byte off its own artifact" "witness: recording is 1 byte off its artifact" "is not its own x86_64 artifact"
refuses "R7 only the normalised digest is off" "witness: only the normalised digest is off" "sha256_normalised"
accepts "R7 twin: a recording from another arch is SKIPPED, by name" "witness: recording is another arch, numbers differ" "WITNESS SKIPPED: main's recording was measured on aarch64"
# R8: the route.
refuses "R8 the heaviest route moved and page.measured_route was not re-recorded" "route moved, not re-recorded" "heaviest route moved"
accepts "R8 twin: re-recorded" "route moved, re-recorded"
# R9: lines new to or gone from this tree are accepted, and counted by name.
accepts "R9 a line new in this tree is counted as new" "a line is new in this tree" "added=1"
accepts "R9 a line gone from this tree is counted as removed" "a line is gone from this tree" "removed=1"

echo
echo "mutants (a copy of the engine with one rule broken must stop refusing that rule's plant)"
mutate_engine() { # <old> <new>: write the mutant, asserting it applied
  node -e '
const fs = require("fs");
const [src, dst, old, nw] = process.argv.slice(1);
const s = fs.readFileSync(src, "utf8");
if (!s.includes(old)) { console.error("MUTATION DID NOT APPLY: " + old); process.exit(3); }
const t = s.replace(old, nw);
if (t === s) { console.error("MUTATION CHANGED NOTHING"); process.exit(3); }
fs.writeFileSync(dst, t);' "$engine" "$mutant" "$1" "$2"
}
mutant_gate() { # <name> <old> <new> <plant case> <expect text the mutant must stop saying>
  local name="$1" old="$2" new="$3" case="$4" expect="$5" out
  mutate_engine "$old" "$new" || { bad "$name: the mutation did not apply"; return; }
  out="$(run "$mutant" "$case")"
  if grep -qF -- "$expect" <<<"$out"; then bad "$name: the mutant STILL says '$expect', so the rule is not what refuses it"; return; fi
  ok "$name: mutant no longer says '$expect'"
}
mutant_twin() { # <name> <old> <new> <twin case> <text the broken run must now show>
  local name="$1" old="$2" new="$3" case="$4" expect="$5" out
  mutate_engine "$old" "$new" || { bad "$name: the mutation did not apply"; return; }
  out="$(run "$mutant" "$case")"
  if ! grep -qF -- "$expect" <<<"$out"; then bad "$name: the mutant's twin run does not show '$expect'"; sed 's/^/        /' <<<"$out" | head -4; return; fi
  ok "$name: mutant now shows '$expect'"
}
mutant_gate "R1 the unrecorded-move refusal" 'if (!recorded) {' 'if (false) {' "grow K, unrecorded" "$K moved against main's artifact"
mutant_gate "R1 recordedAsBuilt always true" 'return field !== undefined && treeRec[field] === built[f];' 'return true;' "grow K, unrecorded" "moved against main's artifact"
mutant_gate "R1b reverted to 'differs from main', not 'equals the build'" 'const recorded = recordedAsBuilt(treeRec, built, moved, fields);' 'const recorded = treeRec !== undefined && moved.some((f) => treeRec[fields.find(([n]) => n === f)[1]] !== mainLine[f]);' "grow K +500, record +1" "does not record this build's value"
mutant_gate "R1b every moved field must equal: every -> some" 'return moved.every((f) => {' 'return moved.some((f) => {' "grow K raw and brotli, record brotli only" "does not record this build's value"
mutant_gate "R1 raw is not compared" 'const LINE_FIELDS = [
  ["raw", "measured_raw"],
' 'const LINE_FIELDS = [
' "grow K raw only, unrecorded" "$K moved against main's artifact (raw"
mutant_gate "R1 brotli is not compared" '  ["brotli", "measured_brotli"],
  ["sha256", "measured_sha256"],' '  ["sha256", "measured_sha256"],' "grow K, unrecorded" "$K moved against main's artifact (brotli"
mutant_gate "R1 sha256 is not compared" '  ["sha256", "measured_sha256"],
];' '];' "K sha only, unrecorded" "$K moved against main's artifact (sha256"
mutant_gate "R2 the totals are not judged" 'judge(name, build[name], main[name], treeRec, TOTAL_FIELDS);' 'void treeRec;' "K recorded, totals not" "total moved against"
mutant_gate "R3 the architecture rule" 'if (build.arch !== main.arch) {' 'if (false) {' "build is aarch64" "not comparable across architectures"
mutant_gate "R4 the artifact sha rule" 'if (src.sha !== meta.head_sha) {' 'if (false) {' "artifact sha differs from the run" "says it is from"
mutant_gate "R4 the artifact run rule" 'if (String(src.run_id) !== String(meta.run_id)) {' 'if (false) {' "artifact run id differs from the fetch" "says run"
mutant_gate "R4 the no-source rule" 'if (src === null || typeof src !== "object") {' 'if (false) {' "artifact has no source" "has no source"
mutant_gate "R5 the event rule" 'if (!EVENTS.includes(meta.event)) {' 'if (false) {' "run event is pull_request" "only push and schedule count"
mutant_gate "R6 the names rule" 'const { problems: found } = sizeLinesProblems(doc, keys, Object.keys(budget.artifacts));' 'const found = [];' "artifact lacks a line" "lines lacks"
mutant_gate "R7 the witness" 'if (mainBudget.measured_arch === main.arch) {' 'if (false) {' "witness: recording is 1 byte off its artifact" "is not its own"
mutant_gate "R8 the route rule" '    routeMoved &&
' '    false &&
' "route moved, not re-recorded" "heaviest route moved"
mutant_twin "R9 the count gate: examined forced to expected" 'examined: lineRows.filter((r) => r.kind !== "new" && r.kind !== "removed").length,' 'examined: treeKeys.length,' "a line is new in this tree" "COUNT-MISMATCH"

echo
echo "end to end, through the real script on files on disk (tree = the real size-budget.json)"
e2e="$workroot/e2e"
mkdir -p "$e2e/main"
node --input-type=module -e '
import { readFileSync, writeFileSync } from "node:fs";
import { expectedLineKeys } from "'"$here"'/size-lines.mjs";
const real = JSON.parse(readFileSync("'"$repo"'/apps/web/size-budget.json", "utf8"));
const keys = expectedLineKeys(real), baseKeys = Object.keys(real.artifacts);
const rec = (k) => real.artifacts[k] ?? real.render.artifacts[k];
const lines = Object.fromEntries(keys.map((k) => [k, { raw: rec(k).measured_raw, brotli: rec(k).measured_brotli, sha256: rec(k).measured_sha256, sha256_normalised: rec(k).measured_sha256_normalised }]));
const sum = (ks, f) => ks.reduce((n, k) => n + lines[k][f], 0);
const SHA = "0123456789abcdef0123456789abcdef01234567";
const doc = { format: 1, arch: "x86_64", measured_route: real.artifacts.page.measured_route, source: null, lines,
  total: { raw: sum(baseKeys, "raw"), brotli: sum(baseKeys, "brotli") }, render_total: { raw: sum(keys, "raw"), brotli: sum(keys, "brotli") } };
const w = (p, o) => writeFileSync(p, JSON.stringify(o, null, 2));
w("'"$e2e"'/build.json", doc);
w("'"$e2e"'/main/size-lines.json", { ...doc, source: { sha: SHA, run_id: "123", workflow: "CI", event: "push" } });
w("'"$e2e"'/main/size-budget.json", real);
w("'"$e2e"'/main/meta.json", { run_id: "123", head_sha: SHA, event: "push", workflow: "ci.yml", artifact_id: "9", created_at: "2026-11-01T00:00:00Z", expires_at: "2026-12-01T00:00:00Z" });
const grown = JSON.parse(JSON.stringify(doc));
grown.lines.page.brotli += 1; grown.total.brotli += 1; grown.render_total.brotli += 1;
w("'"$e2e"'/grown.json", grown);'
status=0
out="$("$checker" --build "$e2e/build.json" --main "$e2e/main" 2>&1)" || status=$?
if [ "$status" -eq 0 ]; then ok "an unchanged build passes"; else bad "an unchanged build was refused"; sed 's/^/        /' <<<"$out" | head -6; fi
if grep -qF "examined $expected_n of $expected_n base line(s) and 2 of 2 totals at 0% against main; artifact from run 123, sha 0123456789abcdef0123456789abcdef01234567, push." <<<"$out"; then ok "it reported 'examined $expected_n of $expected_n' with the artifact's run, sha and event"; else bad "its count line is not what was expected"; sed 's/^/        /' <<<"$out" | head -4; fi
status=0
out="$("$checker" --build "$e2e/grown.json" --main "$e2e/main" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "page moved against main's artifact" <<<"$out"; then ok "page +1 byte, not re-recorded, is refused naming the line"; else bad "page +1 was not refused by name"; sed 's/^/        /' <<<"$out" | head -4; fi
rm "$e2e/main/meta.json"
status=0
out="$("$checker" --build "$e2e/build.json" --main "$e2e/main" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "cannot read the fetch's meta.json" <<<"$out"; then ok "a fetch with no meta.json is refused, naming it"; else bad "a missing meta.json was not refused by name"; fi
status=0
out="$("$checker" --build "$e2e/build.json" 2>&1)" || status=$?
if [ "$status" -eq 2 ]; then ok "a call with no --main is a usage error"; else bad "a missing --main was not exit 2"; fi

echo
if [ "$fail" -ne 0 ]; then echo "FAILED — $fail case(s) failed, $pass passed" >&2; exit 1; fi
echo "OK — $pass case(s) all behaved as required"
