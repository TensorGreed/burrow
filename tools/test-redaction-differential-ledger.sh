#!/usr/bin/env bash
# Adversarial self-test for the browser differential's LEDGER (#137), and for the checker that
# reconciles it from outside the spec.
#
# `apps/web/e2e/redaction-differential.spec.ts` records what the web was observed to reply for
# every case it ran, and its last test -- and, after the run, tools/check-redaction-differential-
# ledger.sh -- judges each observation against `tests/redaction/outcomes.tsv` by case name. This
# plants defects in a COPY of the spec, beside the original so it resolves its paths as the
# original does, and runs it in its own output directory, so the real run's ledger is untouched:
#
#   - a silent DROP in each place a case can be lost: the test body, the plan loop, the parser;
#   - a divergence each document test PARKS with a runtime `test.skip`;
#   - the same, with the LEDGER TEST parked too (`test.fail()`) -- Playwright then passes, and
#     only the checker outside the spec can refuse it;
#   - a verdict BOUND TO THE WRONG CASE, so every document test passes over a divergence.
#
# Each must be refused by the ledger test alone where the ledger test is live, and by the checker
# in every case, naming the reason, with the compared count the run should report. The near-miss
# declares a skip instead of making one silently, and must pass both, named with its reason.
#
# Then the checker's DEFAULT invocation -- what CI runs: Playwright's own project list, the real
# spec's ledger names -- over ledgers copied from the near-miss run: a browser missing, an extra
# browser's ledger, all three present, and a copy of the config with a project the first version's
# regex could not see. No Playwright run for these; only the checker's listing.
#
# Chromium only: the ledger is the same code in every project, and what is being tested is the
# ledger, not the browsers. Each run is the whole spec, because the ledger reconciles the whole
# golden file.
#
# Usage: tools/test-redaction-differential-ledger.sh
#   The first run builds the harness site (global setup), the rest reuse it: BURROW_SKIP_BUILD=1.
#   It builds even in CI, where the step before has just built the same harness site, and that is
#   deliberate: `dist/` carries no build stamp (#202), so reusing one would make this suite's
#   verdict depend on which step ran before it.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
e2e="$repo/apps/web/e2e"
spec="$e2e/redaction-differential.spec.ts"
helper="$e2e/redaction-differential.ts"
mutant_spec="$e2e/redaction-differential.mutant.spec.ts"
mutant_helper="$e2e/redaction-differential.mutant.ts"
mutant_config="$repo/apps/web/playwright.mutant.config.ts"
workroot=""

cleanup() {
  trap - EXIT INT TERM HUP
  rm -f "$mutant_spec" "$mutant_helper" "$mutant_config"
  [ -n "${workroot:-}" ] && rm -rf "$workroot"
  return 0
}
trap cleanup EXIT INT TERM HUP

workroot="$(mktemp -d)"
# `|| true`: `grep -c` exits 1 on a count of zero, and under `set -e` that would end the script
# before the guard below could say why.
total="$(grep -c $'^CASE\t' "$repo/tests/redaction/outcomes.tsv" || true)"
documents="$(grep -c $'^INPUT\t' "$repo/tests/redaction/outcomes.tsv" || true)"
# The first case, spelled as `caseName` spells it, and how many cases carry each label.
first_case="$(awk -F'\t' '$1=="CASE"{print $2" page "$3" covering "$4", "$5; exit}' \
  "$repo/tests/redaction/outcomes.tsv")"
first_document="${first_case%% page *}"
first_document_cases="$(awk -F'\t' -v d="$first_document" '$1=="CASE" && $2==d' \
  "$repo/tests/redaction/outcomes.tsv" | wc -l)"
band_cases="$(awk -F'\t' '$1=="CASE" && $5=="band"' "$repo/tests/redaction/outcomes.tsv" | wc -l)"
echo "golden file: $total cases over $documents documents; first case: $first_case"
for n in "$total" "$documents" "$first_document_cases" "$band_cases"; do
  if [ "$n" -lt 1 ]; then
    echo "  REFUSED: a count this suite plants against is zero; the golden file changed shape" >&2
    exit 1
  fi
done

pass=0
fail=0

# plant <file> <from> <to>: an exact replacement that must match once, read back after writing.
plant() {
  python3 - "$1" "$2" "$3" <<'EOF'
import sys
path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(path, encoding="utf-8").read()
if text.count(old) != 1:
    sys.exit(f"  MUTATION DID NOT APPLY: {old!r} occurs {text.count(old)} times in {path}")
open(path, "w", encoding="utf-8").write(text.replace(old, new))
if new not in open(path, encoding="utf-8").read():
    sys.exit(f"  MUTATION DID NOT APPLY: {path} does not read back with {new!r}")
EOF
}

# fresh [helper]: a new copy of the spec, importing a copy of the helper when asked for one.
fresh() {
  cp "$spec" "$mutant_spec"
  rm -f "$mutant_helper"
  if [ "${1:-}" = helper ]; then
    cp "$helper" "$mutant_helper"
    plant "$mutant_spec" 'from "./redaction-differential";' 'from "./redaction-differential.mutant";'
  fi
}

built=0
# run <name> <playwright: pass|ledger> <compared> <finding> <tests skipped> <checker: pass|refuse>
#   ledger: Playwright fails, in the ledger test alone, and the ledger names <finding>.
#   pass:   Playwright passes, with exactly <tests skipped> skipped.
#   The checker is then run over the same records and must refuse naming <finding>, or pass.
run() {
  local name="$1" expect="$2" compared="$3" finding="$4" skipped="$5" checker="$6"
  local report="$workroot/report.json" results="$workroot/results" status=0 checked=0
  # NEVER THE LAST RUN'S REPORT OR RECORDS: a Playwright that dies before writing must read as
  # broken, and a ledger left by the previous plant must not be judged as this one's.
  rm -rf "$report" "$results"
  local skip="${BURROW_SKIP_BUILD:-}"
  [ "$built" = 1 ] && skip=1
  # THE STATUS OF PLAYWRIGHT ITSELF, not of a pipe: nothing to its right. Spelled
  # `cd apps/web && pnpm exec playwright test` from the repository root because that is the
  # command `tools/ci-local.py`'s `paths_as` names for this script, and it checks the script
  # still runs it. `--output` is this suite's own directory: Playwright empties its output
  # directory when a run starts, and the real run's ledger is what the checker reads in CI.
  (
    export BURROW_SKIP_BUILD="$skip" PLAYWRIGHT_JSON_OUTPUT_NAME="$report"
    cd "$repo"
    cd apps/web && pnpm exec playwright test "e2e/$(basename "$mutant_spec")" --project chromium \
      --output "$results" --reporter=json >"$workroot/stdout" 2>"$workroot/stderr"
  ) || status=$?
  built=1
  # THE COPY'S HELPER when it has one, because that is where a planted declaration lives.
  local helper_args=()
  [ -f "$mutant_helper" ] && helper_args=(--helper "$mutant_helper")
  "$here/check-redaction-differential-ledger.sh" --results "$results" \
    --spec "$(basename "$mutant_spec")" --projects chromium "${helper_args[@]}" \
    >"$workroot/checker" 2>&1 || checked=$?
  if python3 - "$report" "$status" "$expect" "$compared" "$total" "$finding" "$skipped" \
    "$checker" "$checked" "$workroot/checker" <<'EOF'; then
import json, sys
(path, status, expect, compared, total, finding, skipped, checker, checked,
 checker_log) = sys.argv[1:]
LEDGER = "the ledger: every case in the golden file was compared here and agreed, or is declared skipped"
try:
    report = json.load(open(path, encoding="utf-8"))
except (OSError, ValueError) as error:
    print(f"      no report from this run: {error}")
    sys.exit(1)
# BY PLAYWRIGHT'S VERDICT PER TEST, not per attempt: a `test.fail()` that failed is "expected".
failed, skips, passed, ledger = [], [], 0, None
def walk(suite):
    global passed, ledger
    for spec in suite.get("specs", []):
        for test in spec["tests"]:
            if spec["title"] == LEDGER and test["results"]:
                ledger = test["results"][-1]
            if test["status"] == "expected":
                passed += 1
            elif test["status"] == "skipped":
                skips.append(spec["title"])
            else:
                failed.append(spec["title"])
    for child in suite.get("suites", []):
        walk(child)
for suite in report["suites"]:
    walk(suite)
problems = []
want = f"{total} cases in the golden file: {compared} compared ("
if ledger is None:
    problems.append("the ledger test did not run")
else:
    out = "".join(s.get("text", "") for s in ledger.get("stdout", []))
    errors = "".join(e.get("message", "") for e in ledger.get("errors", []))
    if want not in out:
        problems.append(f"the ledger did not report {want!r}; it said {out.strip()!r}")
    if expect == "ledger" and finding not in out + errors:
        problems.append(f"the ledger did not name {finding!r}")
if expect == "pass":
    if status != "0" or failed:
        problems.append(f"expected Playwright to pass; exit {status}, failed: {failed}")
else:
    if status == "0":
        problems.append("expected a refusal; playwright exited 0")
    if failed != [LEDGER]:
        problems.append(f"expected the ledger test alone to fail; failed: {failed}")
if len(skips) != int(skipped):
    problems.append(f"expected {skipped} tests skipped; {len(skips)} were")
if passed == 0:
    problems.append("nothing passed at all: the run is broken, not refused")
said = open(checker_log, encoding="utf-8").read()
if want not in said:
    problems.append(f"the checker did not report {want!r}; it said {said.strip()[-300:]!r}")
if checker == "refuse":
    if checked == "0":
        problems.append("expected the checker to refuse; it exited 0")
    if finding not in said:
        problems.append(f"the checker did not name {finding!r}")
else:
    if checked != "0":
        problems.append(f"expected the checker to pass; it exited {checked}")
    if finding not in said:
        problems.append(f"the checker did not report {finding!r}")
for p in problems:
    print(f"      {p}")
sys.exit(1 if problems else 0)
EOF
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name" >&2
    tail -5 "$workroot/stderr" >&2 || true
    fail=$((fail + 1))
  fi
}

# A divergence in the first case of every document: its observation is not the worker's reply.
diverge_first() {
  plant "$mutant_spec" "      observed: observe(replies[i])," \
    '      observed: i === 0 ? { ...observe(replies[i]), ok: false, kind: "Planted" } : observe(replies[i]),'
}

echo "planted silent drops, each refused by the ledger alone and by the checker:"

fresh
plant "$mutant_spec" "const cases = document.cases;" "const cases = document.cases.slice(1);"
run "one case of every document dropped in the test body" ledger \
  "$((total - documents))" "not compared, and not declared skipped: $first_case" 0 refuse

fresh
plant "$mutant_spec" "for (const document of PLAN) {" "for (const document of PLAN.slice(1)) {"
run "the first document dropped from the plan" ledger \
  "$((total - first_document_cases))" "not compared, and not declared skipped: $first_case" 0 refuse

fresh helper
plant "$mutant_helper" "      document.cases.push({" '      if (label !== "band") document.cases.push({'
run "every band case dropped by the golden file's parser" ledger \
  "$((total - band_cases))" "not compared, and not declared skipped:" 0 refuse

echo "planted divergences that no document test reports:"

fresh
diverge_first
plant "$mutant_spec" "    expect(divergences).toEqual([]);" \
  '    test.skip(divergences.length > 0, "known web divergence");'
run "every document test parking its divergence" ledger \
  "$total" "compared, and diverged: $first_case: " "$documents" refuse

fresh
diverge_first
plant "$mutant_spec" "    expect(divergences).toEqual([]);" \
  '    test.skip(divergences.length > 0, "known web divergence");'
plant "$mutant_spec" "  let recorded: Recorded[] = [];" "  test.fail();
  let recorded: Recorded[] = [];"
run "the same, with the ledger test parked too: Playwright passes, the checker must not" pass \
  "$total" "compared, and diverged: $first_case: " "$documents" refuse

fresh
plant "$mutant_spec" "      observed: observe(replies[i])," \
  '      observed: i === 1 ? { ...observe(replies[i]), ok: false, kind: "Planted" } : observe(replies[i]),'
plant "$mutant_spec" "      const why = judge(cases[i].outcome, v.observed);" \
  "      const why = judge(cases[0].outcome, compared[0].observed);"
run "every document test judging against its first case, over a divergence in its second" ledger \
  "$total" "compared, and diverged: " 0 refuse

echo "the near-miss, which must pass both:"

fresh helper
plant "$mutant_helper" "export const DECLARED_SKIPS: readonly Skip[] = [];" \
  "export const DECLARED_SKIPS: readonly Skip[] = [
  { document: \"$first_document\", case: \"$first_case\", reason: \"planted by the self-test\" },
];"
run "the first case declared skipped, with its reason" pass \
  "$((total - 1))" "1 skipped
  skipped 1: $first_case -- planted by the self-test" 0 pass

# probe <name> <pass|refuse> <results dir> <text the checker must print, one per line> [args...]
probe() {
  local name="$1" expect="$2" dir="$3" wants="$4" status=0 missing=""
  shift 4
  "$here/check-redaction-differential-ledger.sh" --results "$dir" --helper "$mutant_helper" "$@" \
    >"$workroot/probe" 2>&1 || status=$?
  while IFS= read -r want; do
    grep -qF -- "$want" "$workroot/probe" || missing="$missing
      the checker did not print: $want"
  done <<<"$wants"
  if { [ "$expect" = pass ] && [ "$status" -ne 0 ]; } || { [ "$expect" = refuse ] && [ "$status" -eq 0 ]; }; then
    missing="$missing
      expected the checker to $expect; it exited $status"
  fi
  if [ -z "$missing" ]; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name$missing" >&2
    tail -5 "$workroot/probe" >&2 || true
    fail=$((fail + 1))
  fi
}

echo "the checker with Playwright's project list and the real spec's ledger names:"

# THE NEAR-MISS'S LEDGER, under the real spec's name, as each browser's. It reconciles with the
# copy's declaration, which is why these pass --helper; everything else is the default.
source_ledger="$workroot/results/redaction-differential-ledger/$(basename "$mutant_spec").chromium.jsonl"
if [ ! -s "$source_ledger" ]; then
  echo "  REFUSED: the near-miss run left no ledger at $source_ledger to probe with" >&2
  exit 1
fi
ledgers() {
  local dir="$workroot/probe-$1" project
  shift
  mkdir -p "$dir/redaction-differential-ledger"
  for project in "$@"; do
    cp "$source_ledger" "$dir/redaction-differential-ledger/redaction-differential.spec.ts.$project.jsonl"
  done
  echo "$dir"
}

probe "every configured browser's ledger present, each reconciling" pass \
  "$(ledgers all chromium firefox webkit)" "examined 3 ledgers of 3 configured projects (chromium, firefox, webkit)
OK: every case in every browser"
probe "a configured browser with no ledger" refuse "$(ledgers one chromium)" \
  "examined 1 ledgers of 3 configured projects
firefox: no ledger at
webkit: no ledger at"
probe "a ledger from a browser the config does not run" refuse \
  "$(ledgers extra chromium firefox webkit webkit-mobile)" \
  "webkit-mobile: a ledger from a project this check does not examine"

# THE REVIEWERS' ESCAPE, re-planted: a project the first version's regex could not see --
# hyphenated, wrapped over several lines -- in a copy of the config beside the original.
cp "$repo/apps/web/playwright.config.ts" "$mutant_config"
plant "$mutant_config" '    { name: "webkit", use: { ...devices["Desktop Safari"] } },' \
  '    { name: "webkit", use: { ...devices["Desktop Safari"] } },
    {
      name: "webkit-mobile",
      use: { ...devices["iPhone 13"] },
    },'
probe "a configured browser the first version's regex could not see" refuse \
  "$workroot/probe-all" "examined 3 ledgers of 4 configured projects
webkit-mobile: no ledger at" --config "$mutant_config"

echo
echo "$pass passed, $fail failed, of 11"
[ "$fail" -eq 0 ] && [ "$pass" -eq 11 ]
