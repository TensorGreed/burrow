#!/usr/bin/env bash
# Every committed fixture survives every shipped operation, with the outcome pinned (#260).
#
# WHY. `check-no-published-reproducers.py` reads workflows; nothing read `tests/` for an input
# that crashes an operation. This checks the property instead: whatever a fixture is, no shipped
# operation may die on it, and each must still do what it did when the outcome was pinned.
#
# WHAT IT DOES. Every tracked `.pdf` and `.bin` under `tests/` goes through the nine shipped
# operations -- open, check, rotate, reorder, split, compress, merge, redact, render -- natively,
# each pair in A PROCESS OF ITS OWN under a timeout: in-process, a crash takes the runner with
# it and cannot be told from a refusal. The runner is
# `core/burrow-ops/examples/run-operations.rs --op`, exiting 0 on `Ok` and 1 on a typed refusal,
# BUILT HERE, under this gate's own target directory, and found from cargo's own build output --
# never whatever happens to be in `./target`.
#
# THE OUTCOME IS PINNED, IN DETAIL. The runner prints one line per pair -- `<op> ok <detail>` or
# `<op> refused <error>` -- naming the arguments it was given and the output re-opened and counted,
# and `tests/fixtures-survive.tsv` records that line for every (fixture, operation). The table must
# cover exactly the tracked fixtures times the nine operations: a missing row, an extra row, or a
# different line fails. An exit of 0 or 1 without its matching line is the runner broken, not a
# result. Pinning only ok/refused let a runner that never reached an engine pass, and let an
# operation be narrowed (one page rotated, no cut) without a failure -- both measured by review.
# Re-record by name only:
#   BURROW_BLESS_FIXTURES_SURVIVE=1 tools/check-fixtures-survive.sh     (refused when CI is set)
#
# FAILURE IS: a death by signal (named, and a SIGKILL before the timeout is labelled a likely
# out-of-memory kill rather than a timeout); a timeout; any other exit, or an exit without its
# line, which is the runner broken; and any line that differs from the pinned one. Core dumps are
# disabled where the system's dump handler honours `ulimit -c`.
#
# WHAT IT DOES NOT CLAIM. Native only, not the wasm build. Not memory-safety: a fault that does not
# kill the process natively passes -- the nightly fuzz runs the seeded fixtures under
# AddressSanitizer for that. One fixed argument per operation.
#
# ENVIRONMENT, for the self-test (`tools/test-check-fixtures-survive.sh`):
#   BURROW_FIXTURE_RUNNER   a runner to use instead of building the example
#   BURROW_FIXTURE_TIMEOUT  seconds per pair (default 120, above the operations' own 60 s deadline)
#   BURROW_FIXTURE_TABLE    the pinned table (default tests/fixtures-survive.tsv)

set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo" || exit 2
timeout_s="${BURROW_FIXTURE_TIMEOUT:-120}"
table="${BURROW_FIXTURE_TABLE:-$repo/tests/fixtures-survive.tsv}"
operations=(open check rotate reorder split compress merge redact render)
bless="${BURROW_BLESS_FIXTURES_SURVIVE:-0}"
ulimit -c 0

fail() {
  echo "::error::$*" >&2
  exit 1
}

if [ "$bless" = 1 ] && [ -n "${CI:-}" ]; then
  fail "the pinned table is re-recorded by a person, never in CI"
fi

if [ -n "${BURROW_FIXTURE_RUNNER:-}" ]; then
  runner="$BURROW_FIXTURE_RUNNER"
else
  export CARGO_TARGET_DIR="$repo/target/fixtures-survive"
  build_log="$(mktemp)"
  trap 'rm -f "$build_log"' EXIT
  runner=$(cargo build -p burrow-ops --features native-engines --release --example run-operations \
    --message-format=json 2>"$build_log" |
    python3 -c 'import json,sys
for line in sys.stdin:
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("target", {}).get("name") == "run-operations" and m.get("executable"):
        print(m["executable"])')
  if [ -z "$runner" ]; then
    # The JSON stream carries the compiler's own messages, which the filter above discards; a
    # second, cached build without it prints them readably (fourth code review).
    cargo build -p burrow-ops --features native-engines --release --example run-operations 2>&1 |
      tail -40 >&2
    fail "the runner did not build: cargo build -p burrow-ops --features native-engines --release --example run-operations"
  fi
  export LD_LIBRARY_PATH="$repo/engines/vendor/native-$(uname -m)/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
[ -x "$runner" ] || fail "no runner at $runner"

mapfile -t fixtures < <(git ls-files 'tests/*.pdf' 'tests/*.bin')
tracked=${#fixtures[@]}
[ "$tracked" -gt 0 ] || fail "git ls-files found no fixtures under tests/, so this examined nothing"

declare -A pinned=()
if [ "$bless" != 1 ]; then
  [ -f "$table" ] || fail "no pinned table at $table"
  while IFS=$'\t' read -r fixture op line; do
    case "$fixture" in '' | '#'*) continue ;; esac
    [ -z "${pinned["$fixture"$'\t'"$op"]+x}" ] || fail "the pinned table lists $fixture $op twice"
    pinned["$fixture"$'\t'"$op"]="$line"
  done <"$table"
  expected_rows=$((tracked * ${#operations[@]}))
  [ "${#pinned[@]}" -eq "$expected_rows" ] ||
    fail "the pinned table has ${#pinned[@]} row(s) and the tracked fixtures need exactly $expected_rows ($tracked x ${#operations[@]})"
fi

started=$(date +%s)
ok=0
refused=0
failures=()
recorded=()
for fixture in "${fixtures[@]}"; do
  for op in "${operations[@]}"; do
    key="$fixture"$'\t'"$op"
    if [ "$bless" != 1 ] && [ -z "${pinned[$key]+x}" ]; then
      failures+=("$fixture: $op has no row in the pinned table")
      continue
    fi
    began=$(date +%s)
    # The RUNNER's status, read through the pipe: under the inherited `pipefail`, the command
    # substitution's status is the pipeline's, and `head -1` exits 0 -- so a non-zero status here
    # is the runner's own, or `timeout`'s on its behalf. One exception, not reachable today: a
    # runner writing a second line after `head` has exited would die of SIGPIPE (141) and be
    # reported as a crash -- `run-operations --op` prints exactly one line.
    said=$(timeout --kill-after=5 "$timeout_s" "$runner" --op "$op" "$fixture" 2>/dev/null | head -1)
    status=$?
    took=$(($(date +%s) - began))
    outcome=""
    case "$status" in
      0 | 1)
        want=ok
        [ "$status" -eq 1 ] && want=refused
        if [ "${said%% *}" = "$op" ] && [[ "${said#"$op" }" == "$want "* ]]; then
          outcome=$want
        else
          failures+=("$fixture: $op exited $status without its '$op $want ...' line, which is the runner failing rather than a result")
        fi
        ;;
      124) failures+=("$fixture: $op TIMED OUT after ${timeout_s}s") ;;
      137)
        if [ "$took" -ge "$timeout_s" ]; then
          failures+=("$fixture: $op TIMED OUT after ${timeout_s}s, and had to be killed")
        else
          failures+=("$fixture: $op KILLED by signal 9 after ${took}s, before the timeout -- likely out of memory")
        fi
        ;;
      *)
        if [ "$status" -gt 128 ]; then
          failures+=("$fixture: $op CRASHED, killed by signal $((status - 128))")
        else
          failures+=("$fixture: $op exited $status, which is the runner failing rather than the fixture surviving")
        fi
        ;;
    esac
    [ -n "$outcome" ] || continue
    [ "$outcome" = ok ] && ok=$((ok + 1)) || refused=$((refused + 1))
    line="${said#"$op" }"
    recorded+=("$key"$'\t'"$line")
    if [ "$bless" != 1 ] && [ "${pinned[$key]}" != "$line" ]; then
      failures+=("$fixture: $op was pinned '${pinned[$key]}' and is now '$line'")
    fi
  done
done
elapsed=$(($(date +%s) - started))

if [ "$bless" = 1 ]; then
  [ "${#failures[@]}" -eq 0 ] || {
    printf '::error::%s\n' "${failures[@]}" >&2
    fail "not recording a table over a run that failed"
  }
  {
    echo "# The outcome of every shipped operation on every committed fixture, pinned (#260)."
    echo "# Read by tools/check-fixtures-survive.sh; re-record with BURROW_BLESS_FIXTURES_SURVIVE=1."
    printf '%s\n' "${recorded[@]}"
  } >"$table"
  echo "recorded ${#recorded[@]} outcome(s) for $tracked fixture(s) into $table"
  exit 0
fi

echo "check-fixtures-survive: $tracked tracked fixture(s), ${#operations[@]} operations each" \
  "(${#recorded[@]} of $((tracked * ${#operations[@]})) runs with an outcome: $ok ok, $refused refused;" \
  "${#failures[@]} failed) in ${elapsed}s"
if [ "${#failures[@]}" -gt 0 ]; then
  printf '::error::%s\n' "${failures[@]}" >&2
  exit 1
fi
[ "${#recorded[@]}" -eq "$((tracked * ${#operations[@]}))" ] ||
  fail "only ${#recorded[@]} of $((tracked * ${#operations[@]})) runs produced an outcome"
echo "OK -- every committed fixture survives every operation, as pinned"
