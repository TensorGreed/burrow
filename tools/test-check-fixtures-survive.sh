#!/usr/bin/env bash
# Self-test for tools/check-fixtures-survive.sh (#260).
#
# The gate must tell a crash, a hang or a changed outcome from survival. Committing a crashing
# input to prove that would be the very thing the gate forbids, so outcomes are planted as stub
# RUNNERS instead. The faithful stub answers exactly as the pinned table says and must pass; every
# other stub departs from it on one pair -- or everywhere -- and must fail, naming why.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-fixtures-survive.sh"
table="$repo/tests/fixtures-survive.tsv"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$repo" || exit 2

passed=0
failed=0
tracked=$(git ls-files 'tests/*.pdf' 'tests/*.bin' | wc -l)
runs=$((tracked * 9))
first=$(git ls-files 'tests/*.pdf' | head -1)

# A stub runner, called as `--op <op> <file>`. Unless its body exits first, it answers as the
# pinned table does: the pinned line on stdout, then 0 for `ok ...` and 1 for `refused ...`.
stub() {
  cat >"$work/$1" <<STUB
#!/usr/bin/env bash
op=\$2; file=\$3
line=\$(awk -F'\t' -v f="\$file" -v o="\$op" '\$1 == f && \$2 == o { print \$3 }' "$table")
$2
echo "\$op \$line"
[ "\${line%% *}" = ok ] && exit 0
exit 1
STUB
  chmod +x "$work/$1"
  echo "$work/$1"
}
on_first() { echo "[ \"\$file\" = '$first' ] && [ \"\$op\" = '$1' ] && { $2; }"; }

# expect <pass|fail> <label> <text the output must contain> [VAR=value ...]
expect() {
  local want=$1 label=$2 needle=$3 out status
  shift 3
  out=$(env BURROW_FIXTURE_TIMEOUT=2 "$@" "$checker" 2>&1)
  status=$?
  if { [ "$want" = pass ] && [ "$status" -ne 0 ]; } || { [ "$want" = fail ] && [ "$status" -eq 0 ]; }; then
    echo "  FAIL $label: exit $status"
    echo "$out" | tail -4 | sed 's/^/       /'
    failed=$((failed + 1))
  elif ! grep -qF -- "$needle" <<<"$out"; then
    echo "  FAIL $label: exit $status as required, but the output does not say \"$needle\""
    echo "$out" | tail -4 | sed 's/^/       /'
    failed=$((failed + 1))
  else
    echo "  ok   $label"
    passed=$((passed + 1))
  fi
}

echo "check-fixtures-survive self-test ($tracked tracked fixture(s), $runs runs; planted on $first):"

faithful=$(stub faithful '')
expect pass "a runner that answers as the table pins passes, all $runs runs" \
  "$runs of $runs runs with an outcome" BURROW_FIXTURE_RUNNER="$faithful"

# THE OUTCOME IS PINNED: refusing everything, or succeeding everywhere, is a change.
expect fail "a runner that refuses everything fails -- it never reached an engine" \
  "and is now 'refused" BURROW_FIXTURE_RUNNER="$(stub refuse 'echo "$op refused planted"; exit 1')"
expect fail "a runner that succeeds everywhere fails too" \
  "and is now 'ok" BURROW_FIXTURE_RUNNER="$(stub succeed 'echo "$op ok planted"; exit 0')"
expect fail "an operation narrowed but still ok fails, because its line is pinned" \
  "$first: rotate was pinned" \
  BURROW_FIXTURE_RUNNER="$(stub narrowed "$(on_first rotate 'echo "rotate ok pages Some((1, 1)) by 0: output 1 page(s)"; exit 0')")"
expect fail "an exit of 0 with no line is the runner broken, not a success" \
  "$first: open exited 0 without its 'open ok ...' line" \
  BURROW_FIXTURE_RUNNER="$(stub silent "$(on_first open 'exit 0')")"

# THE PLANTED DEATHS, each on one pair, so the report must name the fixture and the operation.
expect fail "a segfault fails, naming the fixture, the operation and the signal" \
  "$first: split CRASHED, killed by signal 11" \
  BURROW_FIXTURE_RUNNER="$(stub segv "$(on_first split 'kill -SEGV $$')")"
expect fail "a crash after the line was printed still fails -- the line does not make it a result" \
  "$first: reorder CRASHED, killed by signal 11" \
  BURROW_FIXTURE_RUNNER="$(stub late "$(on_first reorder 'echo "$op $line"; kill -SEGV $$')")"
expect fail "an abort fails the same way" \
  "$first: merge CRASHED, killed by signal 6" \
  BURROW_FIXTURE_RUNNER="$(stub abrt "$(on_first merge 'kill -ABRT $$')")"
expect fail "a kill before the timeout is labelled a likely out-of-memory kill, not a timeout" \
  "$first: render KILLED by signal 9" \
  BURROW_FIXTURE_RUNNER="$(stub oom "$(on_first render 'kill -KILL $$')")"
expect fail "a hang past the timeout fails, naming it" \
  "$first: open TIMED OUT" \
  BURROW_FIXTURE_RUNNER="$(stub hang "$(on_first open 'sleep 30')")"
expect fail "a hang that ignores the polite signal is killed, and still called a timeout" \
  "$first: compress TIMED OUT after 2s, and had to be killed" \
  BURROW_FIXTURE_RUNNER="$(stub stubborn "$(on_first compress "trap '' TERM; sleep 30")")"
expect fail "a status no runner should return fails as the runner broken" \
  "$first: check exited 3" \
  BURROW_FIXTURE_RUNNER="$(stub odd "$(on_first check 'exit 3')")"
expect fail "a runner that is not there is refused, not passed over" \
  "no runner at" BURROW_FIXTURE_RUNNER="$work/does-not-exist"

# THE TABLE MUST COVER EXACTLY THE TRACKED SET.
grep -v -F "$first"$'\t'"redact"$'\t' "$table" >"$work/short.tsv"
expect fail "a table missing one row fails, counting the rows" \
  "row(s) and the tracked fixtures need exactly $runs" \
  BURROW_FIXTURE_RUNNER="$faithful" BURROW_FIXTURE_TABLE="$work/short.tsv"
{ cat "$table"; printf 'tests/not-tracked.pdf\topen\tok\n'; } >"$work/long.tsv"
expect fail "a table with an extra row fails" \
  "row(s) and the tracked fixtures need exactly $runs" \
  BURROW_FIXTURE_RUNNER="$faithful" BURROW_FIXTURE_TABLE="$work/long.tsv"
sed "s#^$first\t#tests/not-tracked.pdf\t#" "$table" >"$work/renamed.tsv"
expect fail "a table of the right size naming an untracked fixture fails, naming the pair it lacks" \
  "$first: open has no row in the pinned table" \
  BURROW_FIXTURE_RUNNER="$faithful" BURROW_FIXTURE_TABLE="$work/renamed.tsv"
{ cat "$table"; grep -F "$first"$'\t'"open"$'\t' "$table"; } >"$work/twice.tsv"
expect fail "a table listing one pair twice fails" \
  "lists $first open twice" \
  BURROW_FIXTURE_RUNNER="$faithful" BURROW_FIXTURE_TABLE="$work/twice.tsv"
expect fail "re-recording the table is refused in CI" \
  "never in CI" BURROW_FIXTURE_RUNNER="$faithful" BURROW_BLESS_FIXTURES_SURVIVE=1 CI=true

if [ "$failed" -gt 0 ]; then
  echo "FAILED -- $failed case(s) failed, $passed passed"
  exit 1
fi
echo "OK -- $passed adversarial case(s) all behaved as required"
