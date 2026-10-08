#!/usr/bin/env bash
# `cargo check --workspace --all-features` for each mobile target CI checks (#293).
#
# A GATE IN A SCRIPT, so CI and tools/ci-local.py name the same thing. It was a `run:` line in
# ci.yml, and the parity extractor had no `cargo check` token, so the gate was INVISIBLE to it:
# PR #293 ran `tools/ci-local.py --changed` clean -- every job PASS -- and went red on both mobile
# targets, where a fuzz-only item gated on `fuzzing` alone was dead code under `-D warnings`.
#
# Usage: tools/check-mobile-targets.sh [target ...]   (default: both CI targets)
# Each target must be installed (`rustup target add <target>`); a missing one is refused by name
# rather than skipped, since a check that ran on fewer targets than CI is not CI's check.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
targets=("$@")
if [ "${#targets[@]}" -eq 0 ]; then
  targets=(aarch64-apple-ios aarch64-linux-android)
fi

installed="$(rustup target list --installed)"
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
echo "check-mobile-targets: $checked of ${#targets[@]} target(s) checked"
