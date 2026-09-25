#!/usr/bin/env bash
# Adversarial self-test for tools/fuzz-classify.sh and tools/fuzz-describe.sh -- the fuzz
# workflows' verdict and description steps.
#
# WHY IT EXISTS. For six nights (2026-09-20..25) the Describe step died silently on every
# unsymbolised report: a frame grep that matched nothing exited 1 and `bash -e` with `pipefail`
# ended the step before it wrote a frame. And the classifier's exit 1 meant both "new finding" and
# "the tool broke". Both were measured by extracting the steps from the workflow and running them
# by hand; this is that measurement, committed, so the next edit cannot quietly undo it.
#
# Every case runs the REAL script over a synthetic log -- no real crash input exists here or
# anywhere public -- with the classifier stubbed where the case is about the step, not the tool.
#
# Usage: tools/test-fuzz-report-steps.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"
classify="$here/fuzz-classify.sh"
describe="$here/fuzz-describe.sh"
work="$(mktemp -d)"
copy=""
cleanup() {
  rm -rf "$work"
  [ -n "$copy" ] && rm -f "$copy"
  return 0
}
trap cleanup EXIT

pass=0
fail=0
EXPECTED_CASES=14
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

# Run "$@" with a fresh summary; set $status, $out (stdout+stderr) and $summary.
run() {
  : > "$work/summary"
  set +e
  out="$(GITHUB_STEP_SUMMARY="$work/summary" "$@" 2>&1)"
  status=$?
  set -e
  summary="$(cat "$work/summary")"
}

# Require exit $1, and $2 (fixed string) in the summary or the output.
expect() {
  local name="$1" want="$2" needle="$3"
  if [ "$status" -ne "$want" ]; then
    bad "$name (exit $status, wanted $want)"
    printf '%s\n%s\n' "$out" "$summary" | sed 's/^/         /' | tail -6 >&2
  elif ! grep -qF -- "$needle" <<<"$out$summary"; then
    bad "$name: right exit, wrong reason (wanted: $needle)"
    printf '%s\n%s\n' "$out" "$summary" | sed 's/^/         /' | tail -6 >&2
  else
    ok "$name"
  fi
}

# --- the fixtures: synthetic reports in the shapes the nightly produces -----------------------
unsymbolised="$work/unsymbolised.log"
printf '#1 NEW cov: 1\n==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1 at pc 0x2\n    #0 0x55  (/w/fuzz/target/x/release/t+0x44218f) (BuildId: ab)\n    #1 0x56  (/w/fuzz/target/x/release/t+0x44543f) (BuildId: ab)\nSUMMARY: AddressSanitizer: heap-use-after-free\n' > "$unsymbolised"
symbolised="$work/symbolised.log"
printf '==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n    #0 0x55 in Some::Function(Thing)\n    #1 0x56 in Some::Function(Thing)\n' > "$symbolised"
nocrash="$work/nocrash.log"
printf 'error[E0432]: unresolved import\nerror: could not compile\n' > "$nocrash"
mkdir -p "$work/no-artifacts"

echo "fuzz-describe.sh:"

# 1. THE SIX-NIGHT DEFECT: an unsymbolised report, and no binary to resolve it against. It must
#    complete and SAY it could not symbolise, not die before writing a frame.
run "$describe" t "$unsymbolised" "$work/no-such-binary" "$work/no-artifacts"
expect "an unsymbolised report completes and says the binary is missing" 0 "is NOT on disk"

# 2. A symbolised report names its hottest frame.
run "$describe" t "$symbolised" "$work/no-such-binary" "$work/no-artifacts"
expect "a symbolised report names its hottest frame" 0 "hottest frame: \`Some::Function\`"

# 3. A failure with no sanitiser report is not described as a crash.
run "$describe" t "$nocrash" "$work/no-such-binary" "$work/no-artifacts"
expect "a log with no report is not described as a crash" 0 "not a crash"

# 4. THE STEP ITSELF FAILING names itself as the tool. A summary that cannot be written is a
#    failure inside the step that has nothing to do with the crash.
set +e
out="$(GITHUB_STEP_SUMMARY="$work/no-such-dir/summary" "$describe" t "$unsymbolised" \
  "$work/no-such-binary" "$work/no-artifacts" 2>&1)"
status=$?
set -e
summary=""
if [ "$status" -ne 0 ] && grep -qF "DESCRIBE STEP FAILURE (tooling, not a crash)" <<<"$out"; then
  ok "a failure inside the step is titled as the tool's, not the crash's"
else
  bad "a failure inside the step is titled as the tool's (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

# 5. THE META-TEST: put the six-night defect back -- the frame grep unguarded -- in a COPY beside
#    the original, and require case 1 to stop completing. Asserting the mutation applied first.
copy="$here/.fuzz-describe.mutant.sh"
python3 - "$describe" "$copy" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text()
old = "{ grep -hoE '^[[:space:]]*#[0-9]+ 0x[0-9a-f]+ in [A-Za-z_][A-Za-z0-9_:]*' \"$report\" || true; }"
assert source.count(old) == 1, "the guarded frame grep moved; update this meta-test"
mutated = source.replace(old, "grep -hoE '^[[:space:]]*#[0-9]+ 0x[0-9a-f]+ in [A-Za-z_][A-Za-z0-9_:]*' \"$report\"", 1)
assert mutated != source
Path(sys.argv[2]).write_text(mutated)
PY
chmod +x "$copy"
run "$copy" t "$unsymbolised" "$work/no-such-binary" "$work/no-artifacts"
if [ "$status" -ne 0 ] && grep -qF "DESCRIBE STEP FAILURE" <<<"$out"; then
  ok "the old unguarded grep fails the step, and the trap names it as the tool"
else
  bad "the old unguarded grep fails the step, and the trap names it (exit $status)"
  sed 's/^/         /' <<<"$out$summary" >&2
fi
rm -f "$copy"
copy=""

# 5b. A REAL-SIZED LOG. Thousands of coverage lines ahead of the report killed this step by SIGPIPE
#     (`sort | head -1` under pipefail, exit 141, measured by review from ~1,000 offsets) -- and it
#     read them as candidate frames. 3,000 here, with a report frame that must be the one named.
big="$work/big.log"
{
  for n in $(seq 1 3000); do
    printf '\tNEW_FUNC[1/1]: 0x%x  (/w/fuzz/target/x/release/t+0x%x) (BuildId: ab)\n' "$n" "$((65536 + n))"
  done
  printf '==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n'
  printf '    #0 0x55  (/w/fuzz/target/x/release/t+0x44218f) (BuildId: ab)\n'
  printf '    #1 0x56  (/w/fuzz/target/x/release/t+0x44218f) (BuildId: ab)\n'
} > "$big"
run "$describe" t "$big" "$work/no-such-binary" "$work/no-artifacts"
expect "a log with 3,000 coverage lines completes and names the REPORT's offset" 0 "offset \`0x44218f\`"

# 5c. A PANIC MESSAGE CARRYING THE BANNER TEXT. The CMap target's assertion formats its whole input
#     into the message printed before libFuzzer's banner; unanchored, this step published it as the
#     verdict (security review, 2026-09-25).
forged="$work/forged.log"
{
  printf "thread '<unnamed>' (1) panicked at fuzz_targets/x.rs:1:2:\n"
  printf '"prefix ERROR: libFuzzer: SECRETINPUTBYTES more input"\n'
  printf '==42== ERROR: libFuzzer: deadly signal\n'
  printf '    #0 0x1 in Some::Function(Thing)\n'
} > "$forged"
run "$describe" t "$forged" "$work/no-such-binary" "$work/no-artifacts"
if [ "$status" -eq 0 ] && grep -qF "libFuzzer: deadly signal" <<<"$summary" \
  && ! grep -qF "SECRETINPUTBYTES" <<<"$out$summary"; then
  ok "a panic message carrying the banner text is neither the verdict nor published"
else
  bad "a panic message carrying the banner text is neither the verdict nor published (exit $status)"
  printf '%s\n' "$summary" | sed 's/^/         /' >&2
fi

echo "fuzz-classify.sh:"

# A stub classifier that prints a line and exits with the code it is given.
stub="$work/stub-classifier.sh"
printf '#!/usr/bin/env bash\necho "stub classifier output"\nexit "${STUB_EXIT:-0}"\n' > "$stub"
chmod +x "$stub"

# 6. No crash is a pass, and says so.
run env BURROW_CLASSIFIER="$stub" "$classify" t 0 "$unsymbolised" "$work/no-such-binary"
expect "a clean fuzz run passes" 0 "no crash"

# 7. A failed fuzz step with no report is an INFRASTRUCTURE failure, under its own heading.
run env BURROW_CLASSIFIER="$stub" "$classify" t 1 "$nocrash" "$work/no-such-binary"
expect "a failure with no report is INFRASTRUCTURE FAILURE, exit 4, never the new-finding 1" 4 "INFRASTRUCTURE FAILURE"

# 8-11. Each classifier exit code under its own heading.
for pair in "0|KNOWN -- owned by a filed issue" "1|NEW FINDING" \
  "3|CLASSIFIER FAILURE (exit 3)" "7|CLASSIFIER FAILURE (exit 7)"; do
  code="${pair%%|*}"
  needle="${pair#*|}"
  run env STUB_EXIT="$code" BURROW_CLASSIFIER="$stub" "$classify" t 1 "$unsymbolised" \
    "$work/no-such-binary"
  expect "classifier exit $code is reported as '$needle'" "$code" "$needle"
done

# 12. And a tooling failure never carries the NEW FINDING title, which is what made six nights of
#     red unreadable.
run env STUB_EXIT=3 BURROW_CLASSIFIER="$stub" "$classify" t 1 "$unsymbolised" "$work/no-such-binary"
if ! grep -qF "NEW FINDING" <<<"$out$summary"; then
  ok "a classifier failure never reads as a new finding"
else
  bad "a classifier failure never reads as a new finding"
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
echo "OK -- $pass case(s) all behaved as required"
