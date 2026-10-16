#!/usr/bin/env bash
# Self-test for tools/check-lockfile.sh: it refuses a lockfile that would change, names the
# workspace and the reason, and passes a near-miss that would not.
#
# The planted workspace sits BESIDE the repository's own build output, under `target/` (which is
# gitignored), not in a system temp dir -- the CLAUDE.md rule for checker copies, kept here too.
# Offline: the plant is a PATH dependency, so the new lock entry it needs is one cargo can name
# without the network, and the refusal is about the lockfile and nothing else.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
gate="$here/check-lockfile.sh"
# `target/` MAY NOT EXIST: CI's deny job builds nothing before this runs, and mktemp does not
# create parents. Made here, so the self-test runs on a clean checkout (round 1: it did not).
mkdir -p "$repo/target"
scratch="$(mktemp -d "$repo/target/lockfile-selftest.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

passed=0
failed=0
ok() { echo "  ok   $1"; passed=$((passed + 1)); }
bad() { echo "  FAIL $1"; failed=$((failed + 1)); }

make_workspace() {
  local ws="$1"
  mkdir -p "$ws/a/src" "$ws/b/src"
  printf '[workspace]\nmembers = ["a", "b"]\nresolver = "2"\n' > "$ws/Cargo.toml"
  for c in a b; do
    printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' "$c" > "$ws/$c/Cargo.toml"
    : > "$ws/$c/src/lib.rs"
  done
  (cd "$ws" && cargo generate-lockfile --offline -q)
}

# 1. THE NEAR-MISS: a lockfile that satisfies its manifests passes, and is reported examined.
near="$scratch/near"
make_workspace "$near"
if out="$("$gate" "$near" 2>&1)" && printf '%s' "$out" | grep -q 'examined 1 of 1 lockfiles'; then
  ok "a lockfile that satisfies its manifests passes, examined 1 of 1"
else
  bad "the near-miss did not pass: $out"
fi

# 2. THE PLANT: a dependency that needs a new lock entry. Asserted applied before the gate runs.
drift="$scratch/drift"
make_workspace "$drift"
printf '\n[dependencies]\nb = { path = "../b" }\n' >> "$drift/a/Cargo.toml"
grep -q 'b = { path = "../b" }' "$drift/a/Cargo.toml" || { bad "the plant did not apply"; exit 1; }
before="$(sha256sum "$drift/Cargo.lock")"
if out="$("$gate" "$drift" 2>&1)"; then
  bad "a lockfile that would change passed"
elif printf '%s' "$out" | grep -q "drift: the lockfile would change" && printf '%s' "$out" | grep -q -- "--locked"; then
  ok "a lockfile that would change is refused, naming the workspace and cargo's reason"
else
  bad "refused for the wrong reason: $out"
fi
# AND THE GATE DID NOT FIX IT: the lockfile is untouched.
if [ "$before" = "$(sha256sum "$drift/Cargo.lock")" ]; then
  ok "the gate does not rewrite the lockfile it refuses"
else
  bad "the gate rewrote the lockfile"
fi

# 3. NO LOCKFILE AT ALL is refused by name, not resolved.
none="$scratch/none"
make_workspace "$none"
rm "$none/Cargo.lock"
if out="$("$gate" "$none" 2>&1)"; then
  bad "a workspace with no lockfile passed"
elif printf '%s' "$out" | grep -q "no Cargo.lock"; then
  ok "a workspace with no lockfile is refused by name"
else
  bad "no lockfile refused for the wrong reason: $out"
fi

# 4. THE GATE WITHOUT `--locked` -- a copy BESIDE the original, so it resolves its own paths as the
#    original does -- passes the same drift. That is the regression case 2 exists to catch, shown
#    to be one rather than assumed.
mutant="$here/.check-lockfile.mutant.sh"
trap 'rm -rf "$scratch" "$mutant"' EXIT
sed 's/cargo metadata --locked /cargo metadata /' "$gate" > "$mutant"
chmod +x "$mutant"
if grep -q -- 'cargo metadata --locked' "$mutant" || ! grep -q -- 'cargo metadata --format-version' "$mutant"; then
  bad "the mutation did not apply: the copy still passes --locked"
else
  drift2="$scratch/drift2"
  make_workspace "$drift2"
  printf '\n[dependencies]\nb = { path = "../b" }\n' >> "$drift2/a/Cargo.toml"
  if "$mutant" "$drift2" >/dev/null 2>&1; then
    ok "a gate without --locked passes the drift, so case 2 is what catches its loss"
  else
    bad "the mutant refused the drift: case 2 would not show the loss of --locked"
  fi
fi

# 5. THE REAL ONES, with no arguments: the set is DERIVED from git, so the expected count is too --
#    every committed lockfile less the exemptions, each exemption printed with its reason.
committed="$(git -C "$repo" ls-files '*Cargo.lock' | wc -l)"
exempt="$(grep -c '^  \["' "$gate")"
expected=$((committed - exempt))
if out="$("$gate" 2>&1)" && printf '%s' "$out" | grep -q "examined $expected of $expected lockfiles" &&
  printf '%s' "$out" | grep -q "^  skip spikes/redaction-survival: "; then
  ok "the repository's lockfiles pass, examined $expected of $expected ($committed committed, $exempt exempt by name)"
else
  bad "the repository's lockfiles (expected $expected of $committed): $out"
fi
[ "$expected" -ge 2 ] || bad "fewer than the root and fuzz/ lockfiles were expected: $expected"

# 6. AN EXEMPTION REMOVED -- in a copy beside the original -- examines the spike too, so the count
#    moves: the count comes from the derivation, not from a list someone keeps.
unexempt="$here/.check-lockfile.unexempt.sh"
trap 'rm -rf "$scratch" "$mutant" "$unexempt"' EXIT
sed '/^  \["spikes\/redaction-survival"\]=/d' "$gate" > "$unexempt"
chmod +x "$unexempt"
if grep -q 'spikes/redaction-survival"\]=' "$unexempt" || cmp -s "$gate" "$unexempt"; then
  bad "the exemption removal did not apply"
elif out="$("$unexempt" 2>&1)" && printf '%s' "$out" | grep -q "examined $committed of $committed lockfiles"; then
  ok "without the exemption every committed lockfile is examined, $committed of $committed"
else
  bad "the unexempted copy did not examine all $committed: $out"
fi

echo "$passed passed, $failed failed, of $((passed + failed))"
[ "$failed" -eq 0 ]
