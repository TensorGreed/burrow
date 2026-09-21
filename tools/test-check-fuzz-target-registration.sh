#!/usr/bin/env bash
# The guard itself is broken, one way per run, and must refuse each time — NAMING THE REASON.
#
#     tools/test-check-fuzz-target-registration.sh
#
# `CLAUDE.md`: "The probe gate itself has a test: break a rule in a COPY of the checker and
# assert it refuses, naming the reason." Here the rules are about three files rather than about
# the script's own text, so what is planted is a broken *input* — which is the same thing one
# level out, and lets the real script be the one under test rather than a copy of it.
#
# Each case is a miss that actually happened:
#
#   1. a target in the manifest and in no `for target in` list       — M1 PR B, `reorder`
#   2. a target in the manifest and not in the nightly matrix        — `merge`, `split`,
#                                                                      `rotate`, `reorder`,
#                                                                      then #128's two
#   3. a manifest naming no targets at all                           — the "examined nothing"
#                                                                      shape, which the first
#                                                                      version would have
#                                                                      reported as OK
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
checker="$root/tools/check-fuzz-target-registration.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The real thing must pass first. A suite that only ever asserts refusals cannot tell a working
# guard from one that refuses everything.
if ! "$checker" >"$work/real.log" 2>&1; then
  echo "FAILED — the guard refuses the repository as it stands:" >&2
  cat "$work/real.log" >&2
  exit 1
fi
echo "  ok   the repository as it stands passes"

mkdir -p "$work/fuzz"
cp "$root/fuzz/Cargo.toml" "$work/fuzz/Cargo.toml"
cp "$root/.github/workflows/ci.yml" "$work/ci.yml"
cp "$root/.github/workflows/fuzz-nightly.yml" "$work/fuzz-nightly.yml"

# A name no real target has, so planting it cannot collide with one.
planted="never_registered_anywhere"

expect_refusal() {
  local case_name="$1" expected="$2"
  shift 2
  local status=0
  # ASSERT THE MUTATION APPLIED. A `sed` that matched nothing leaves a green run that reads as
  # "the guard works" while nothing was broken at all.
  if ! "$@" >"$work/out.log" 2>&1; then
    status=1
  fi
  if [ "$status" -eq 0 ]; then
    echo "FAILED — $case_name: the guard accepted it" >&2
    cat "$work/out.log" >&2
    exit 1
  fi
  if ! grep -q "$expected" "$work/out.log"; then
    echo "FAILED — $case_name: refused, but not for the reason it should have been." >&2
    echo "  expected to see: $expected" >&2
    cat "$work/out.log" >&2
    exit 1
  fi
  echo "  ok   $case_name"
}

# 1. Declared, run nowhere, and not in the matrix either.
printf '\n[[bin]]\nname = "%s"\npath = "fuzz_targets/%s.rs"\n' "$planted" "$planted" \
  >>"$work/fuzz/Cargo.toml"
grep -q "$planted" "$work/fuzz/Cargo.toml" || {
  echo "FAILED — the planted target never reached the manifest copy" >&2
  exit 1
}
expect_refusal "a target declared and run nowhere" "declared but never run" \
  env FUZZ_MANIFEST="$work/fuzz/Cargo.toml" CI_WORKFLOW="$work/ci.yml" \
      NIGHTLY_WORKFLOW="$work/fuzz-nightly.yml" "$checker"

# 2. Declared and run, but absent from the nightly matrix — so it never runs seeded.
before=$(md5sum <"$work/ci.yml")
sed -i "s/for target in document_open/for target in $planted document_open/" "$work/ci.yml"
[ "$(md5sum <"$work/ci.yml")" != "$before" ] || {
  echo "FAILED — the ci.yml mutation matched nothing" >&2
  exit 1
}
expect_refusal "a target absent from the nightly matrix" "never run seeded" \
  env FUZZ_MANIFEST="$work/fuzz/Cargo.toml" CI_WORKFLOW="$work/ci.yml" \
      NIGHTLY_WORKFLOW="$work/fuzz-nightly.yml" "$checker"

# 3. A manifest naming nothing. The guard must say it examined nothing rather than print OK.
: >"$work/fuzz/empty.toml"
expect_refusal "a manifest with no targets at all" "examined nothing" \
  env FUZZ_MANIFEST="$work/fuzz/empty.toml" CI_WORKFLOW="$work/ci.yml" \
      NIGHTLY_WORKFLOW="$work/fuzz-nightly.yml" "$checker"

echo "OK — 1 passing case and 3 planted misses, each refused for its own reason."
