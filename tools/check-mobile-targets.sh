#!/usr/bin/env bash
# `cargo check --workspace --all-features` for each mobile target CI checks (#293).
#
# A GATE IN A SCRIPT, so CI and tools/ci-local.py name the same thing. It was a `run:` line in
# ci.yml, and the parity extractor had no `cargo check` token, so the gate was INVISIBLE to it:
# PR #293 ran `tools/ci-local.py --changed` clean -- every job PASS -- and went red on both mobile
# targets, where a fuzz-only item gated on `fuzzing` alone was dead code under `-D warnings`.
#
# THE TARGETS ARE CI'S, READ FROM ci.yml's `mobile` matrix when none are given, so a target added
# there is checked here without anyone remembering this file (a hand copy was the first version,
# and the code review of #293 called it what it was: the same shape as the bug).
#
# `-D warnings` IS THE DEFAULT HERE TOO: CI sets it in the workflow's env, and a direct run without
# it passes the exact dead-code regression this exists for. An explicit RUSTFLAGS is respected.
#
# Usage: tools/check-mobile-targets.sh [--list] [target ...]
#   --list   print the targets that would be checked, one per line, and exit
# Each target must be installed (`rustup target add <target>`); a missing one is refused by name
# rather than skipped, since a check that ran on fewer targets than CI is not CI's check.
# Self-test: tools/test-check-mobile-targets.sh. CI_YML overrides the workflow read (self-test).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
ci_yml="${CI_YML:-$repo/.github/workflows/ci.yml}"
: "${RUSTFLAGS:=-D warnings}"
export RUSTFLAGS

list=0
if [ "${1:-}" = "--list" ]; then
  list=1
  shift
fi
targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  mapfile -t targets < <(python3 - "$ci_yml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
job = re.search(r"^  mobile:\n((?:    .*\n|\s*\n)+)", text, re.M)
if not job:
    sys.exit(0)
block = re.search(r"^ {8}target:\n((?: {10}- .*\n)+)", job.group(1), re.M)
if block:
    for line in block.group(1).splitlines():
        print(line.split("- ", 1)[1].strip())
PY
)
  if [ "${#targets[@]}" -eq 0 ]; then
    echo "FAILED -- no targets found in $ci_yml's mobile matrix; a check of nothing is not a check" >&2
    exit 1
  fi
fi
if [ "$list" -eq 1 ]; then
  printf '%s\n' "${targets[@]}"
  exit 0
fi

installed="$(cd "$repo" && rustup target list --installed)"
checked=0
for target in "${targets[@]}"; do
  if ! grep -qxF "$target" <<<"$installed"; then
    echo "FAILED -- target $target is not installed (rustup target add $target); refusing to report a check that did not run" >&2
    exit 1
  fi
  echo "check-mobile-targets: cargo check --workspace --all-features --target $target"
  (cd "$repo" && cargo check --workspace --all-features --target "$target")
  checked=$((checked + 1))
done
echo "check-mobile-targets: $checked of ${#targets[@]} target(s) checked (RUSTFLAGS=$RUSTFLAGS)"
