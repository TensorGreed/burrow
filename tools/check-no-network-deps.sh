#!/usr/bin/env bash
# Fail if any crate on deny.toml's network/TLS ban list is reachable from any workspace
# member, through any dependency kind, for any target we ship to.
#
# WHY THIS EXISTS, GIVEN cargo-deny ALREADY DOES THIS
#
# Non-negotiable #1 -- file content never leaves the device -- rests on there being no
# HTTP client in the graph. `cargo deny check bans` enforces that and does cover
# dev-dependencies (measured: planting `ureq` as a dev-dependency of burrow-engines makes
# it fail, and catches transitive `ring` with it). So this is not closing a hole.
#
# It exists because the guarantee is worth more than one tool's opinion of the graph, and
# because cargo-deny's graph is demonstrably *not* the lockfile: with an empty allowlist
# its licence check names 31 crates while `Cargo.lock` holds 48, and `serde_json` is in the
# lockfile but absent from that output. We did not chase down why. Something we cannot
# fully explain is doing the enforcing, so a second check that we can explain in fifteen
# lines -- `cargo tree`, every kind, every target, plain string match -- is cheap insurance.
#
# It reads the ban list out of deny.toml rather than repeating it, so the two cannot drift.
#
# Run:   tools/check-no-network-deps.sh
# Test:  tools/test-check-no-network-deps.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(dirname "$here")"
deny_toml="${BURROW_DENY_TOML:-$repo/deny.toml}"

# Manifests to walk. `fuzz/` is excluded from the root workspace and has its own lockfile,
# so it needs naming separately -- that is exactly how libfuzzer-sys entered unaudited
# (ADR 0012).
#
# BURROW_MANIFESTS (colon-separated) overrides the list. Only the self-test uses it, so it
# can point at a throwaway workspace instead of mutating this one.
if [ -n "${BURROW_MANIFESTS:-}" ]; then
  IFS=: read -r -a manifests <<<"$BURROW_MANIFESTS"
else
  manifests=("$repo/Cargo.toml" "$repo/fuzz/Cargo.toml")
fi

# The `{ crate = "name", reason = "..." }` entries of deny.toml's [bans] deny list.
# Comment lines are skipped so a crate named in prose is not mistaken for a ban.
mapfile -t banned < <(
  sed -n '/^\[bans\]/,/^\[[^b]/p' "$deny_toml" |
    grep -v '^[[:space:]]*#' |
    grep -oE '\{[[:space:]]*crate[[:space:]]*=[[:space:]]*"[^"]+"' |
    grep -oE '"[^"]+"' |
    tr -d '"'
)

if [ "${#banned[@]}" -eq 0 ]; then
  echo "::error::no banned crates parsed from $deny_toml -- the [bans] deny list moved or is empty" >&2
  echo "A check that silently has nothing to check is worse than no check." >&2
  exit 2
fi

echo "banned crates from $(basename "$deny_toml"): ${banned[*]}"

# PROBES. Every banned name must be caught by the matcher below, and a near-miss must not.
#
# The matcher is `grep -qxF`, which is about as simple as a rule gets -- but "simple" is what
# `check-no-generated-files.sh` was before 15 of its 16 patterns turned out to be inert, and
# what this script's own crate-graph count was before it turned out to gate nothing. The
# expensive part of a check is not writing it, it is proving it is not inert. So the rule is
# exercised against fixtures on every run, using the SAME matcher the real loop uses.
matches() { printf '%s\n' "$2" | grep -qxF "$1"; }

probe_problems=0
for crate in "${banned[@]}"; do
  # Positive: the exact name must be caught.
  if ! matches "$crate" "serde
$crate
libc"; then
    echo "::error::the matcher does not catch '$crate', which it is supposed to ban" >&2
    probe_problems=$((probe_problems + 1))
  fi
  # Negative: a name that merely CONTAINS it must not be. `reqwest-middleware` and
  # `hyper-rustls` are real crates; banning them by substring would be wrong, and matching
  # them by accident would make every graph look poisoned.
  if matches "$crate" "serde
${crate}-middleware
not-${crate}
${crate}_sys
libc"; then
    echo "::error::the matcher catches a near-miss of '$crate' -- it is not an exact match" >&2
    probe_problems=$((probe_problems + 1))
  fi
done

if [ "$probe_problems" -ne 0 ]; then
  echo "A ban list whose matcher does not behave as declared bans nothing, or bans" >&2
  echo "everything. Neither is a check." >&2
  exit 2
fi
echo "  ${#banned[@]} ban(s), each verified to match itself and reject a near-miss"

# `--target all` covers every platform, so a dependency gated behind cfg(windows) cannot
# hide. `-e normal,dev,build` covers every kind, which is the whole point.
found=0
for manifest in "${manifests[@]}"; do
  [ -f "$manifest" ] || {
    echo "::error::$manifest does not exist" >&2
    exit 2
  }

  tree="$(cargo tree --manifest-path "$manifest" --workspace --all-features \
    --target all -e normal,dev,build --prefix none --no-dedupe 2>/dev/null)" || {
    echo "::error::cargo tree failed for $manifest" >&2
    exit 2
  }

  # First whitespace-separated field of each line is the crate name.
  names="$(printf '%s\n' "$tree" | awk 'NF {print $1}' | sort -u)"
  count="$(printf '%s\n' "$names" | grep -c . || true)"
  echo "$(realpath --relative-to="$repo" "$manifest" 2>/dev/null || echo "$manifest"): $count distinct crates"

  # COVERAGE: the graph must contain the crates we KNOW are in it.
  #
  # A bare count is not a gate -- `cargo tree` returning three crates would have printed "3
  # distinct crates" and passed, and every ban would have been checked against a graph that
  # was not the graph. The expected members are knowable exactly, so they are asserted rather
  # than eyeballed.
  # DERIVED from the manifest, not hardcoded. A hardcoded list was wrong twice over: it
  # would rot when a member is added, and it made this script unusable against any manifest
  # but this repository's -- which broke its own self-test, whose fixtures are synthetic
  # workspaces. Reading the manifest keeps the expectation exact AND general.
  expected_members="$(python3 - "$manifest" <<'PYEOF'
import sys, tomllib
with open(sys.argv[1], "rb") as fh:
    data = tomllib.load(fh)
members = data.get("workspace", {}).get("members", [])
names = [m.rstrip("/").split("/")[-1] for m in members]
if not names:
    package = data.get("package", {}).get("name")
    if package:
        names = [package]
print(" ".join(names))
PYEOF
  )"
  if [ -z "$expected_members" ]; then
    echo "::error::could not derive any expected crate from $manifest" >&2
    echo "  Without an expectation the crate count below gates nothing." >&2
    exit 2
  fi
  missing_members=""
  for member in $expected_members; do
    printf '%s\n' "$names" | grep -qxF "$member" || missing_members="$missing_members $member"
  done
  if [ -n "$missing_members" ]; then
    echo "::error::the dependency graph for $manifest is missing workspace member(s):$missing_members" >&2
    echo "  cargo tree returned something that is not this workspace's graph, so every ban" >&2
    echo "  below would have been checked against the wrong thing." >&2
    exit 2
  fi
  echo "  contains all $(printf '%s' "$expected_members" | wc -w) expected workspace member(s)"

  for crate in "${banned[@]}"; do
    if printf '%s\n' "$names" | grep -qxF "$crate"; then
      echo "::error::banned crate '$crate' is reachable from $manifest" >&2
      printf '%s\n' "$tree" | grep -nF "$crate " | head -5 >&2
      found=1
    fi
  done
done

if [ "$found" != 0 ]; then
  echo >&2
  echo "A crate on the network/TLS ban list is in the dependency graph." >&2
  echo "See deny.toml's [bans] deny list and CLAUDE.md non-negotiable #1." >&2
  exit 1
fi

echo "no banned network or TLS crate is reachable from any workspace member"
