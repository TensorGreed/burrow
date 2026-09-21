#!/usr/bin/env bash
# The checker itself is broken, one way per run, and must refuse each time — NAMING THE REASON.
#
#     tools/test-check-proptest-regressions.sh
#
# `CLAUDE.md`: "The probe gate itself has a test: break a rule in a COPY of the checker and
# assert it refuses, naming the reason. Put the copy BESIDE the original — a copy in a temp
# directory resolves its own paths wrongly and exits non-zero for the wrong reason, which an
# exit-code-only assertion reports as a pass."
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
original="$root/tools/check-proptest-regressions.py"
copy="$root/tools/.check-proptest-regressions.undertest.py"
# CASE 4 PLANTS A FILE IN THE REAL INDEX, so the trap has to know about it before it exists.
# On an ordinary failure the case cleans up after itself; on SIGINT it did not, and a stale
# intent-to-add entry makes the NEXT run of the checker die in `read_text` with a
# `FileNotFoundError` rather than a diagnosis. Found by code review.
planted="$root/core/burrow-ops/tests/.probe.proptest-regressions"
cleanup() {
  rm -f "$copy" "$planted"
  git -C "$root" rm -q --cached "$planted" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

# The real thing must pass first. A suite that only asserts refusals cannot tell a working
# checker from one that refuses everything.
if ! "$original" >/dev/null 2>&1 && ! python3 "$original" >/dev/null 2>&1; then
  echo "FAILED — the checker refuses the repository as it stands:" >&2
  python3 "$original" >&2 || true
  exit 1
fi
echo "  ok   the repository as it stands passes"

break_it() {
  local name="$1" from="$2" to="$3" expected="$4"
  cp "$original" "$copy"
  python3 - "$copy" "$from" "$to" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
# ASSERT THE MUTATION APPLIED. A replace that matched nothing leaves a green run that reads as
# "this defence works" while nothing was broken at all.
assert s.count(sys.argv[2]) == 1, f"mutation matched {s.count(sys.argv[2])} times: {sys.argv[2][:50]}"
p.write_text(s.replace(sys.argv[2], sys.argv[3], 1))
PY
  local out
  out="$(python3 "$copy" 2>&1 || true)"
  if ! grep -q "$expected" <<<"$out"; then
    echo "FAILED — $name: the broken checker did not refuse for the right reason." >&2
    echo "  expected to see: $expected" >&2
    echo "$out" >&2
    exit 1
  fi
  echo "  ok   $name"
  rm -f "$copy"
}

# 1. The rule stops noticing a bare seed. Its own probe must catch that.
break_it "a rule that accepts a seed with no comment" \
  'if not above.startswith("#"):' \
  'if False:' \
  "THE RULE ITSELF FAILED"

# 2. The rule stops recognising proptest's boilerplate as non-explanation.
break_it "a rule that accepts the boilerplate as an explanation" \
  'elif is_boilerplate(above):' \
  'elif False:' \
  "THE RULE ITSELF FAILED"

# 3. The length floor is removed, so `# flaky` would pass.
break_it "a rule with no length floor" \
  'MIN_EXPLANATION = 25' \
  'MIN_EXPLANATION = 0' \
  "THE RULE ITSELF FAILED"

# 4. A REAL unexplained seed in the tree is refused. The three above break the checker; this
#    one leaves it intact and breaks the INPUT, which is the failure the tool is actually for.
# THE BYTES PROPTEST WRITES, all five header lines including the bare `#`, with the seed
# immediately after -- `write_header` uses `writeln!`, so there is no blank line between. An
# abridged header here is what let two `BOILERPLATE` entries be deleted with this test green.
cat > "$planted" <<'SEED'
# Seeds for failure cases proptest has generated in the past. It is
# automatically read and these particular cases re-run before any
# novel cases are generated.
#
# It is recommended to check this file in to source control so that
# everyone who runs the test benefits from these saved cases.
cc 0000000000000000000000000000000000000000000000000000000000000000 # shrinks to pages = 2
SEED
git -C "$root" add -N "$planted" >/dev/null 2>&1 || true
out="$(python3 "$original" 2>&1 || true)"
cleanup
if ! grep -q "record no defect" <<<"$out"; then
  echo "FAILED — an unexplained seed in the tree was not refused." >&2
  echo "$out" >&2
  exit 1
fi
echo "  ok   an unexplained seed in the tree is refused"

echo "OK — 1 passing case, 3 broken rules and 1 planted seed, each refused for its own reason."
