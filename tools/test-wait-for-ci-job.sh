#!/usr/bin/env bash
# Self-test for tools/wait-for-ci-job.sh: a fake `gh` on PATH answers from a scripted list of
# responses, so every outcome of the wait is exercised without the network.
#
#   success                      exits 0
#   skipped, cancelled, failure  each exits 1, naming the conclusion (a skipped job made no files)
#   no job named X               tolerated for a bounded number of polls (it may not be scheduled
#                                yet), then exits 1; a job that appears within the bound succeeds
#   two jobs named X             exits 1
#   the API erroring             gives up after 5 errors IN A ROW, exits 1
#   errors interleaved with      4 errors, a success, 4 more errors, then completed: exits 0 (only
#     successful reads           consecutive errors count; main run 41103583240 failed on 5 spread out)
#   stuck in_progress            exits 1 at the deadline
#   queued, in_progress, success exits 0
#
# Mutants: copies of the script BESIDE it, one rule broken, asserted applied; this script re-run
# against each must FAIL naming the case that rule guards (skipped passing; no deadline; the
# missing-job tolerance unbounded; the error count never reset by a successful read).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="${BURROW_WAIT_SCRIPT:-$here/wait-for-ci-job.sh}"
work="$(mktemp -d)"
mutant="$here/.wait-for-ci-job.mutant.sh"
trap 'rm -rf "$work"; rm -f "$mutant"' EXIT
pass=0
fail=0
expected=14
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

mkdir -p "$work/bin"
# The fake: answers call N with line N of $FAKE_RESPONSES (the last line repeats), and counts calls.
#   ERR            exit 1 (an API error)
#   EMPTY          print nothing (the job is not in the list)
#   anything else  printed verbatim, `\n` as a newline (what `--jq` would have produced)
cat > "$work/bin/gh" <<'FAKE'
#!/usr/bin/env bash
n=$(( $(cat "$FAKE_DIR/calls" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$FAKE_DIR/calls"
total="$(wc -l < "$FAKE_DIR/responses")"
line="$(sed -n "$(( n > total ? total : n ))p" "$FAKE_DIR/responses")"
case "$line" in
  ERR) exit 1 ;;
  EMPTY) exit 0 ;;
  *) printf '%b\n' "$line" ;;
esac
FAKE
chmod +x "$work/bin/gh"

# run_case <label> <want status> <want text> <want calls or ""> <deadline> -- <response lines...>
run_case() {
  local label="$1" want="$2" text="$3" calls="$4" deadline="$5"
  shift 6
  printf '%s\n' "$@" > "$work/responses"
  rm -f "$work/calls"
  local status=0 out got
  out="$(env PATH="$work/bin:$PATH" FAKE_DIR="$work" GITHUB_REPOSITORY=o/r GITHUB_RUN_ID=1 \
    BURROW_WAIT_INTERVAL_S=0.1 BURROW_WAIT_MISSING_POLLS=3 \
    timeout 12 "$script" "native conformance record" "$deadline" 2>&1)" || status=$?
  got="$(cat "$work/calls" 2>/dev/null || echo 0)"
  if [ "$status" -ne "$want" ] || ! grep -qF -- "$text" <<<"$out"; then
    bad "$label (status $status, wanted $want and '$text'): $(tail -n 2 <<<"$out")"
  elif [ -n "$calls" ] && [ "$got" -ne "$calls" ]; then
    bad "$label (gh called $got times, wanted $calls)"
  else ok "$label"; fi
}

run_case "success exits 0" 0 "succeeded" 1 60 -- "completed success"
for conclusion in skipped cancelled failure; do
  run_case "$conclusion exits 1, naming the conclusion" 1 "concluded '$conclusion'" 1 60 -- "completed $conclusion"
done
run_case "no job named X exits 1 after the bounded polls" 1 "after 3 polls" 3 60 -- EMPTY
run_case "a job that appears within the bound is waited for (N2)" 0 "succeeded" 3 60 -- EMPTY EMPTY "completed success"
run_case "two jobs named X exits 1" 1 "2 jobs named" 1 60 -- "completed success\ncompleted success"
run_case "an API error that persists gives up after 5 in a row" 1 "failed 5 times in a row" 5 60 -- ERR
run_case "errors interleaved with successful reads do not accumulate" 0 "succeeded" 10 60 -- ERR ERR ERR ERR "in_progress " ERR ERR ERR ERR "completed success"
run_case "an API error that clears is survived" 0 "succeeded" 3 60 -- ERR ERR "completed success"
run_case "stuck in_progress exits 1 at the deadline" 1 "had not completed after 2 s" "" 2 -- "in_progress "
run_case "queued, in_progress, success exits 0" 0 "succeeded" 3 60 -- "queued " "in_progress " "completed success"
status=0
out="$(env -u GITHUB_RUN_ID PATH="$work/bin:$PATH" GITHUB_REPOSITORY=o/r "$script" x 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "GITHUB_RUN_ID is unset" <<<"$out"; then ok "outside Actions it refuses, named"; else bad "outside Actions it refuses, named (status $status)"; fi
status=0
out="$(env PATH="$work/bin:$PATH" GITHUB_REPOSITORY=o/r GITHUB_RUN_ID=1 "$script" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "usage:" <<<"$out"; then ok "no job name refuses, with usage"; else bad "no job name refuses, with usage (status $status)"; fi

if [ -z "${BURROW_WAIT_MUTANT:-}" ]; then
  mutants=0
  # plant <label> <old> <new> <the FAIL line the nested run must print>
  plant() {
    mutants=$((mutants + 1))
    rm -f "$mutant"
    if ! python3 - "$here/wait-for-ci-job.sh" "$mutant" "$2" "$3" <<'PY'
import pathlib, sys
src, dst, old, new = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3], sys.argv[4]
text = src.read_text()
assert text.count(old) == 1, f"MUTATION DID NOT APPLY: {old!r} occurs {text.count(old)} times"
dst.write_text(text.replace(old, new, 1))
PY
    then bad "mutant '$1': the mutation did not apply"; return; fi
    if cmp -s "$here/wait-for-ci-job.sh" "$mutant"; then bad "mutant '$1': identical copy"; return; fi
    chmod +x "$mutant"
    local status=0 out
    out="$(BURROW_WAIT_MUTANT=1 BURROW_WAIT_SCRIPT="$mutant" "$here/test-wait-for-ci-job.sh" 2>&1)" || status=$?
    if [ "$status" -ne 0 ] && grep -qF -- "FAIL $4" <<<"$out"; then ok "mutant '$1' is caught: FAIL $4"
    else bad "mutant '$1' survived (status $status; no 'FAIL $4')"; fi
  }
  plant "skipped passes" 'if [ "$conclusion" = "success" ]; then' 'if [ "$conclusion" = "success" ] || [ "$conclusion" = "skipped" ]; then' \
    "skipped exits 1, naming the conclusion"
  plant "no deadline" 'if [ $((SECONDS - start)) -ge "$deadline_s" ]; then' 'if false; then' \
    "stuck in_progress exits 1 at the deadline"
  plant "the missing-job tolerance is unbounded" 'if [ "$missing" -ge "$max_missing" ]; then' 'if false; then' \
    "no job named X exits 1 after the bounded polls"
  plant "errors never reset by a successful read" 'errors=0 # a successful read ends a run of errors: only CONSECUTIVE errors count' ':' \
    "errors interleaved with successful reads do not accumulate"
  if [ "$mutants" -eq 4 ]; then ok "4 of 4 wait-script mutants were planted"; else bad "planted $mutants of 4 mutants"; fi
  expected=$((expected + mutants + 1))
fi

echo
if [ "$fail" -ne 0 ] || [ "$pass" -ne "$expected" ]; then
  echo "FAILED — $pass of $expected case(s) passed, $fail failed" >&2
  exit 1
fi
echo "OK — $pass case(s) of $expected behaved as required"
