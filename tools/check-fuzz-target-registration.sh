#!/usr/bin/env bash
# Every fuzz target declared in fuzz/Cargo.toml is run by ci.yml's smoke job AND is in
# fuzz-nightly.yml's matrix.
#
#     tools/check-fuzz-target-registration.sh
#
# WHY THIS IS A SCRIPT AND NOT A `run:` BLOCK
#
# It was a `run:` block in ci.yml, and that is precisely the shape `CLAUDE.md` records as the
# fifth row of its ci-local table: `tools/ci-local.py` maps a step to a token it can recognise,
# an inline block naming no script maps to nothing, and the parity table reported full coverage
# over a gate with no local counterpart. So the local runner passed clean over the exact miss
# this guard was written for -- measured on #128, where two new targets reached ci.yml and never
# reached the nightly matrix, and `ci-local.py --check` said every command was covered.
#
# `CLAUDE.md`: "a gate belongs in a script: `tools/check-*.sh` is a name both CI and this runner
# can invoke, which is what gives the parity table something to track."
#
# WHY IT READS Cargo.toml RATHER THAN `cargo +nightly fuzz list`
#
# `cargo fuzz list` reads the same `[[bin]]` stanzas, and needs a nightly toolchain and
# cargo-fuzz installed. Reading the manifest lets this run in a job that fetches nothing and on
# a machine with no nightly -- and a target that is in the manifest and not in the binary is a
# different failure, which `cargo +nightly fuzz build` already catches.
#
# WHAT IT REPORTS
#
# The three lists by name, not a verdict. A guard that prints OK over an empty declared list has
# measured nothing, so an empty list is itself a failure.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="${FUZZ_MANIFEST:-$root/fuzz/Cargo.toml}"
ci="${CI_WORKFLOW:-$root/.github/workflows/ci.yml}"
nightly="${NIGHTLY_WORKFLOW:-$root/.github/workflows/fuzz-nightly.yml}"

for file in "$manifest" "$ci" "$nightly"; do
  if [ ! -f "$file" ]; then
    echo "check-fuzz-target-registration: no such file: $file" >&2
    exit 1
  fi
done

# `|| true` on each: an empty result is a FINDING this script reports by name, and `set -e`
# would otherwise kill it on grep's exit 1 before the report -- an empty manifest exiting 1 with
# no message reads as a broken script rather than as the miss it is.
declared=$(grep -oE '^name = "[a-z_]+"' "$manifest" | sed 's/name = "//; s/"//' | sort -u || true)
run=$(grep -ohE 'for target in [a-z_ ]+' "$ci" | sed 's/for target in //' | tr ' ' '\n' \
      | grep -v '^$' | sort -u || true)
matrix=$(sed -n '/^        target:/,/^    steps:/p' "$nightly" \
         | grep -oE '^ *- [a-z_]+$' | tr -d ' -' | sort -u || true)

echo "declared in $(basename "$(dirname "$manifest")")/Cargo.toml: $(echo "$declared" | tr '\n' ' ')"
echo "run by ci.yml:                                  $(echo "$run" | tr '\n' ' ')"
echo "in the nightly matrix:                          $(echo "$matrix" | tr '\n' ' ')"

failed=0
if [ -z "$declared" ]; then
  echo "::error::no fuzz targets found in $manifest -- this guard examined nothing" >&2
  failed=1
fi

missing=$(comm -23 <(echo "$declared") <(echo "$run"))
if [ -n "$missing" ]; then
  echo "::error::fuzz targets declared but never run by ci.yml: $(echo "$missing" | tr '\n' ' ')" >&2
  failed=1
fi

absent=$(comm -23 <(echo "$declared") <(echo "$matrix"))
if [ -n "$absent" ]; then
  # Worse than a missing step: ci.yml's smoke run is UNSEEDED while #62 is open, and the nightly
  # matrix is what runs a target seeded. A target absent here has never met the definition of
  # done's "runs clean for 60 s against a seeded corpus".
  echo "::error::fuzz targets missing from the nightly matrix, so they never run seeded: $(echo "$absent" | tr '\n' ' ')" >&2
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "OK -- all $(echo "$declared" | wc -l | tr -d ' ') declared target(s) run in ci.yml and nightly."
