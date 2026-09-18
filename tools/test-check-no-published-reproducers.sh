#!/usr/bin/env bash
# Test that check-no-published-reproducers.py refuses a workflow publishing a fuzzing input,
# and that its own probe gate refuses a rule that has been made inert.
#
# The case that matters is the first: the exact step that was in `fuzz-nightly.yml` for four
# days, replanted. A gate written after an incident that cannot reproduce that incident is a
# gate nobody has tested against the thing it exists for.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
# Beside the original: the checker resolves REPO from its own __file__, so a copy in a temp
# directory scans the wrong tree and exits non-zero for a reason unrelated to the rule.
mutant="$here/check-no-published-reproducers.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=11
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

# The checker scans $REPO/.github/workflows, so a case is a temporary tree with its own
# workflows directory and a copy of the checker two levels up from it.
plant() {
  local name="$1" body="$2"
  rm -rf "$work/case"
  mkdir -p "$work/case/.github/workflows" "$work/case/tools"
  cp "$here/check-no-published-reproducers.py" "$work/case/tools/"
  printf '%s\n' "$body" > "$work/case/.github/workflows/planted.yml"
  python3 -c "
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
assert d and (d.get('jobs') or d.get('runs')), 'the planted file parses as neither workflow nor action'
" "$work/case/.github/workflows/planted.yml"
}

expect() {
  local name="$1" want="$2" needle="${3:-}"
  local out status=0
  out="$(python3 "$work/case/tools/check-no-published-reproducers.py" 2>&1)" || status=$?
  if [ "$status" -ne "$want" ]; then
    bad "$name (exit $status, wanted $want)"
    sed 's/^/         /' <<<"$out" >&2
  elif [ -n "$needle" ] && ! grep -qF "$needle" <<<"$out"; then
    bad "$name: refused, but not for the stated reason"
    echo "       expected: $needle" >&2
    sed 's/^/         /' <<<"$out" >&2
  else
    ok "$name"
  fi
}

echo "check-no-published-reproducers.py:"

# 1. THE INCIDENT, REPLANTED VERBATIM. This is the step that was in fuzz-nightly.yml.
plant "the original" 'name: n
on: {schedule: [{cron: "0 8 * * *"}]}
jobs:
  fuzz:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02
        with:
          name: fuzz-artifacts-rotate
          path: fuzz/artifacts/
          if-no-files-found: ignore'
expect "the step that published reproducers for four days is refused" 1 "fuzz/artifacts"

# 2. A crash directory somewhere else entirely.
plant "elsewhere" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: /tmp/crash-inputs}'
expect "an upload named for crashes, from any path, is refused" 1 "crash"

# 3. The corpus is inputs too.
plant "corpus" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: fuzz/corpus/rotate}'
expect "uploading the corpus is refused" 1 "fuzz/corpus"

# 4. THE NEAR-MISS. A gate that refuses legitimate uploads gets turned off.
plant "legitimate" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: apps/web/dist}'
expect "an ordinary artifact upload is accepted" 0

# 5. A WORKFLOW SET WITH NO UPLOADS AT ALL must refuse, because then the gate is examining
#    nothing -- this repository does upload artifacts, and finding none means the parser broke.
plant "no uploads" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi'
expect "a workflow set with no upload steps at all is refused, not passed" 1 "finding none"

# 6. THE BYPASSES A REVIEW DEMONSTRATED against the first version of this gate. Each one
#    published `fuzz/artifacts/` while containing none of the forbidden fragments.
plant "ancestor" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: fuzz/}'
expect "uploading the PARENT of the artifact directory is refused" 1 "ancestor"

plant "workspace" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: .}'
expect "uploading the whole workspace is refused" 1 "ancestor"

plant "the log" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/upload-artifact@v4
        with: {path: "${{ runner.temp }}/fuzz.log"}'
expect "uploading the raw fuzz log is refused" 1 "fuzz.log"

plant "composite" 'name: c
description: d
runs:
  using: composite
  steps:
    - uses: actions/upload-artifact@v4
      with: {path: fuzz/artifacts/}'
expect "a composite action that uploads inputs is refused" 1 "fuzz/artifacts"

plant "shouting" 'name: n
on: {push: {branches: [main]}}
jobs:
  x:
    runs-on: ubuntu-latest
    steps:
      - uses: ACTIONS/Upload-Artifact@v4
        with: {path: fuzz/artifacts/}'
expect "a differently-cased action reference is still matched" 1 "fuzz/artifacts"

# 7. THE META-TEST. Break a rule in a copy beside the original; the probe gate must refuse.
python3 - "$here/check-no-published-reproducers.py" "$mutant" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = 'INPUT_DIRS = ("fuzz/artifacts", "fuzz/corpus")'
assert old in source, "the rule list moved; update this meta-test rather than deleting it"
mutated = source.replace(old, 'INPUT_DIRS = ()', 1).replace(
    'FORBIDDEN = ("fuzz/artifacts", "fuzz/corpus", "crash", "fuzz.log", "fuzz-log")',
    'FORBIDDEN = ("nothing-matches-this",)', 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
set +e
out="$(python3 "$mutant" 2>&1)"
status=$?
set -e
if [ "$status" -ne 0 ] && grep -qF "the rule is inert" <<<"$out"; then
  ok "the probe gate refuses an inert rule, and says so"
else
  bad "the probe gate refuses an inert rule, and says so (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
