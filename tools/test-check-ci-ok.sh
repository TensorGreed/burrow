#!/usr/bin/env bash
# Self-test for tools/check-ci-ok.py: per-rule probes for both modes, and an asserted mutation of
# the real ci.yml in a copy beside it.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
check="$here/check-ci-ok.py"
real="$repo/.github/workflows/ci.yml"
copy="$here/.ci-ok-probe.yml"
work="$(mktemp -d)"
trap 'rm -rf "$work"; rm -f "$copy"' EXIT
pass=0
fail=0
expected=16
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# A probe workflow: two jobs, and a gate needing $1 under `if: $2`, whose step runs $3.
probe() {
  cat > "$work/w.yml" <<YML
jobs:
  build: {runs-on: x, steps: [{run: "true"}]}
  test: {runs-on: x, steps: [{run: "true"}]}
  ci-ok:
    runs-on: x
    if: $2
    needs: [$1]
    steps: [{run: "${3:-python3 tools/check-ci-ok.py results}"}]
YML
}

# 1. A COMPLETE GATE PASSES, and reports what it examined.
probe "build, test" "always()"
status=0; out="$(python3 "$check" needs "$work/w.yml" 2>&1)" || status=$?
if [ "$status" -eq 0 ] && grep -q "2 of 2 job(s)" <<<"$out"; then ok "a gate needing every job passes, counting them"
else bad "a complete gate did not pass with its count (status $status): $out"; fi

# 2. A JOB MISSING FROM needs IS REFUSED, by name.
probe "build" "always()"
status=0; out="$(python3 "$check" needs "$work/w.yml" 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "does not need test" <<<"$out"; then ok "a job missing from needs is refused, named"
else bad "a missing job was not refused by name (status $status): $out"; fi

# 3. A GATE WITHOUT always() IS REFUSED: a failed need would skip it.
probe "build, test" "success()"
status=0; out="$(python3 "$check" needs "$work/w.yml" 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "if: always()" <<<"$out"; then ok "a gate not under always() is refused"
else bad "a gate without always() was not refused (status $status): $out"; fi

# 3a. A GATE WITH NO STEP RUNNING THE RESULTS CHECK IS REFUSED: it would pass on anything.
probe "build, test" "always()" "true"
status=0; out="$(python3 "$check" needs "$work/w.yml" 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "no step running" <<<"$out"; then ok "a gate with no results step is refused"
else bad "a gate with no results step was not refused (status $status): $out"; fi

# 3b. A GATE NEEDING A JOB THE WORKFLOW LACKS IS REFUSED, by name.
probe "build, test, nope" "always()"
status=0; out="$(python3 "$check" needs "$work/w.yml" 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "does not have: nope" <<<"$out"; then ok "a gate needing an absent job is refused, named"
else bad "a gate needing an absent job was not refused by name (status $status): $out"; fi

# 4-8. THE RESULTS MODE: every non-success value fails, named; all success passes; none fails.
for result in failure cancelled skipped; do
  status=0; out="$(NEEDS="{\"build\":{\"result\":\"success\"},\"test\":{\"result\":\"$result\"}}" python3 "$check" results 2>&1)" || status=$?
  if [ "$status" -eq 1 ] && grep -q "test ($result)" <<<"$out"; then ok "a $result job fails the gate, named"
  else bad "a $result job did not fail the gate by name (status $status): $out"; fi
done
status=0; out="$(NEEDS='{"build":{"result":"success"},"test":{"result":"success"}}' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 0 ] && grep -q "all 2 job(s) succeeded" <<<"$out"; then ok "every job succeeding passes the gate"
else bad "all-success did not pass (status $status): $out"; fi
status=0; out="$(NEEDS='{}' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "names no jobs" <<<"$out"; then ok "a gate that saw no jobs fails"
else bad "an empty needs did not fail (status $status): $out"; fi

# 8a-8d. MALFORMED $NEEDS FAILS, each with its own reason rather than a traceback.
status=0; out="$(NEEDS='' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "NEEDS is empty" <<<"$out"; then ok "an empty \$NEEDS fails, named"
else bad "an empty \$NEEDS did not fail by name (status $status): $out"; fi
status=0; out="$(NEEDS='{"build":' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "is not JSON" <<<"$out"; then ok "a garbled \$NEEDS fails, named"
else bad "a garbled \$NEEDS did not fail by name (status $status): $out"; fi
status=0; out="$(NEEDS='["build"]' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "not a JSON object of jobs (it is a list)" <<<"$out"; then ok "a non-object \$NEEDS fails, named"
else bad "a non-object \$NEEDS did not fail by name (status $status): $out"; fi
status=0; out="$(NEEDS='{"build":{"result":"success"},"test":"success"}' python3 "$check" results 2>&1)" || status=$?
if [ "$status" -eq 1 ] && grep -q "test (no result: not a job record)" <<<"$out"; then ok "a job entry that is not a record fails, named"
else bad "a non-record job entry did not fail by name (status $status): $out"; fi

# 9. THE REAL ci.yml PASSES.
status=0; out="$(python3 "$check" needs "$real" 2>&1)" || status=$?
if [ "$status" -eq 0 ]; then ok "the real ci.yml passes: $(tail -n 1 <<<"$out" | cut -c1-90)"
else bad "the real ci.yml does not pass (status $status): $out"; fi

# 10. AN ASSERTED MUTATION OF THE REAL ci.yml: a copy beside it with one job dropped from the
# gate's needs must go red, naming that job.
python3 - "$real" "$copy" <<'PY'
import pathlib, re, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
match = re.search(r"(\n  ci-ok:\n(?:    .*\n)*?    needs: \[)([^\]]*)\]", text)
assert match, "MUTATION DID NOT APPLY: ci-ok's needs line has moved"
jobs = [j.strip() for j in match.group(2).split(",")]
assert "audit" in jobs, "MUTATION DID NOT APPLY: audit is not in ci-ok's needs"
kept = ", ".join(j for j in jobs if j != "audit")
dst.write_text(text[:match.start(2)] + kept + text[match.end(2):])
PY
if cmp -s "$real" "$copy"; then
  bad "the needs mutation did not apply, so case 10 measured nothing"
else
  status=0; out="$(python3 "$check" needs "$copy" 2>&1)" || status=$?
  if [ "$status" -eq 1 ] && grep -q "does not need audit" <<<"$out"; then ok "the real ci.yml with audit dropped from needs goes red, naming audit"
  else bad "dropping audit from the real ci.yml's needs did not go red by name (status $status): $out"; fi
fi

echo
if [ "$fail" -ne 0 ] || [ "$pass" -ne "$expected" ]; then
  echo "FAILED — $pass of $expected case(s) passed, $fail failed" >&2
  exit 1
fi
echo "OK — $pass case(s) of $expected behaved as required"
