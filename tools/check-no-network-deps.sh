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
