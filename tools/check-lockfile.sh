#!/usr/bin/env bash
# Fail if a Cargo workspace's committed lockfile does not already satisfy its manifests.
#
# WHY. No cargo invocation in CI, the deploy payload build or the tools/*.sh gates passed
# `--locked` (2026-10-15, checked by hand). So a manifest change that needed a new lock entry,
# or a lock entry that no longer matched, was RESOLVED -- possibly to a newer version than the
# one reviewed -- instead of failing. The owner's rule: lockfile drift fails the build, it never
# resolves.
#
# ONE GATE IS ENOUGH FOR A JOB. A lockfile that already satisfies its manifests is never
# rewritten by a later cargo command, so proving it here, before anything builds, proves no
# drift for every cargo command after it in the same checkout. The `--locked` flags elsewhere
# in ci.yml and the payload action are belt and braces.
#
# WHAT IT EXAMINES, AND SAYS SO. Every workspace with its own lockfile: the root and `fuzz/`,
# which is its own workspace. It reports "examined N of N lockfiles", and refuses naming the
# workspace and cargo's reason.
#
# Usage: check-lockfile.sh [workspace-dir ...]
# With no arguments, checks the root workspace and fuzz/. Directories may be given for testing.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

if [ "$#" -gt 0 ]; then
  workspaces=("$@")
else
  workspaces=("$repo" "$repo/fuzz")
fi

failed=0
examined=0
for dir in "${workspaces[@]}"; do
  examined=$((examined + 1))
  name="${dir#"$repo"/}"
  [ "$dir" = "$repo" ] && name="(root)"
  if [ ! -f "$dir/Cargo.lock" ]; then
    echo "check-lockfile: $name: no Cargo.lock -- a workspace with no committed lockfile resolves whatever is newest" >&2
    failed=$((failed + 1))
    continue
  fi
  # STATUS CAPTURED, NOT PIPED: the exit code is the verdict, and stderr is the reason.
  if err="$(cargo metadata --locked --format-version 1 --manifest-path "$dir/Cargo.toml" 2>&1 >/dev/null)"; then
    echo "  ok   $name: Cargo.lock satisfies its manifests"
  else
    reason="$(printf '%s\n' "$err" | grep -m1 -E 'lock file|--locked|error' || true)"
    echo "check-lockfile: $name: the lockfile would change -- ${reason:-cargo metadata --locked failed}" >&2
    failed=$((failed + 1))
  fi
done

total="${#workspaces[@]}"
if [ "$failed" -ne 0 ]; then
  echo "check-lockfile: REFUSED -- $failed of $total lockfile(s) would change; examined $examined of $total lockfiles" >&2
  exit 1
fi
echo "check-lockfile: examined $examined of $total lockfiles; none would change"
