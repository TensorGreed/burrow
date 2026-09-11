#!/usr/bin/env bash
# Test that check-no-network-deps.sh actually fails when a banned crate is planted.
#
# A checker that silently passes is worse than no checker: it converts "nobody checked"
# into "something checked and it was fine". This repository has shipped three such tools
# already (a format hook that found no formatter, a fetch script that verified nothing, and
# an "engine tests ran" guard that asserted a count it would always meet), so this one gets
# a test before it is trusted.
#
# Every case builds a throwaway workspace under a temporary directory and points the
# checker at it with BURROW_MANIFESTS. This repository is never modified, and nothing is
# fetched: the stand-in for a banned crate is a local path dependency that merely *has the
# name* `ureq`, which is all the checker matches on.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-no-network-deps.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0

check() { # description expected_exit manifest
  local desc="$1" want="$2" manifest="$3" got
  set +e
  BURROW_MANIFESTS="$manifest" "$checker" >"$tmp/out" 2>"$tmp/err"
  got=$?
  set -e
  if [ "$got" = "$want" ]; then
    echo "  ok    $desc (exit $got)"
    pass=$((pass + 1))
  else
    echo "  FAIL  $desc: expected exit $want, got $got"
    sed 's/^/          /' "$tmp/out" "$tmp/err" | head -20
    fail=$((fail + 1))
  fi
}

# A crate that is merely NAMED like a banned one. The checker matches on the name in
# `cargo tree` output, which is exactly what it would see for the real thing.
mk_fake_banned() { # dir name
  mkdir -p "$1/src"
  cat >"$1/Cargo.toml" <<EOF
[package]
name = "$2"
version = "0.0.0"
edition = "2021"
license = "MIT"
EOF
  echo "" >"$1/src/lib.rs"
}

mk_crate() { # dir name  -- writes a minimal library crate, caller appends dependencies
  mkdir -p "$1/src"
  cat >"$1/Cargo.toml" <<EOF
[package]
name = "$2"
version = "0.0.0"
edition = "2021"
license = "MIT"
EOF
  echo "" >"$1/src/lib.rs"
}

echo "check-no-network-deps.sh self-test"

# --- 1. A clean workspace passes. Without this, every case below could be passing for the
#        wrong reason -- a checker that always fails would look perfect.
mk_crate "$tmp/clean" clean
check "a clean workspace passes" 0 "$tmp/clean/Cargo.toml"

# --- 2. The case that matters: a banned crate as a DEV-dependency. This is the shape the
#        whole tool exists for, and the one the original (incorrect) finding claimed
#        cargo-deny could not see.
mk_fake_banned "$tmp/devdep/ureq" ureq
mk_crate "$tmp/devdep/victim" victim
cat >>"$tmp/devdep/victim/Cargo.toml" <<'EOF'

[dev-dependencies]
ureq = { path = "../ureq" }
EOF
check "a banned crate as a dev-dependency is caught" 1 "$tmp/devdep/victim/Cargo.toml"

# --- 3. A banned crate reached TRANSITIVELY through a dev-dependency. A network client
#        almost never arrives directly; it arrives as somebody else's implementation detail.
mk_fake_banned "$tmp/transitive/ring" ring
mk_crate "$tmp/transitive/middle" middle
cat >>"$tmp/transitive/middle/Cargo.toml" <<'EOF'

[dependencies]
ring = { path = "../ring" }
EOF
mk_crate "$tmp/transitive/victim" victim
cat >>"$tmp/transitive/victim/Cargo.toml" <<'EOF'

[dev-dependencies]
middle = { path = "../middle" }
EOF
check "a banned crate transitively under a dev-dependency is caught" 1 "$tmp/transitive/victim/Cargo.toml"

# --- 4. A banned crate as a BUILD-dependency.
mk_fake_banned "$tmp/builddep/curl" curl
mk_crate "$tmp/builddep/victim" victim
cat >>"$tmp/builddep/victim/Cargo.toml" <<'EOF'

[build-dependencies]
curl = { path = "../curl" }
EOF
echo 'fn main() {}' >"$tmp/builddep/victim/build.rs"
check "a banned crate as a build-dependency is caught" 1 "$tmp/builddep/victim/Cargo.toml"

# --- 5. A banned crate behind a target cfg for a platform this host is not. `--target all`
#        is what covers this; without it the crate is invisible on the build machine and
#        appears only for whoever builds for that platform.
mk_fake_banned "$tmp/cfgdep/reqwest" reqwest
mk_crate "$tmp/cfgdep/victim" victim
cat >>"$tmp/cfgdep/victim/Cargo.toml" <<'EOF'

[target.'cfg(target_os = "windows")'.dependencies]
reqwest = { path = "../reqwest" }
EOF
check "a banned crate behind a foreign target cfg is caught" 1 "$tmp/cfgdep/victim/Cargo.toml"

# --- 6. The checker refuses to run rather than pass vacuously when it parses no ban list.
#        This is the failure mode that makes a green check meaningless.
cat >"$tmp/empty-deny.toml" <<'EOF'
[bans]
multiple-versions = "warn"
deny = []
EOF
set +e
BURROW_DENY_TOML="$tmp/empty-deny.toml" BURROW_MANIFESTS="$tmp/clean/Cargo.toml" \
  "$checker" >"$tmp/out" 2>"$tmp/err"
got=$?
set -e
if [ "$got" = "2" ]; then
  echo "  ok    an empty ban list is an error, not a pass (exit 2)"
  pass=$((pass + 1))
else
  echo "  FAIL  an empty ban list should exit 2, got $got"
  fail=$((fail + 1))
fi

# --- 7. A crate merely MENTIONED in a deny.toml comment is not treated as banned. The ban
#        list is parsed out of prose-heavy TOML, so this is a real way to get a false
#        positive that nobody could explain.
sed 's|^\[bans\]|[bans]\n# Historical note: we once considered vendoring clean, but did not.|' \
  "$(dirname "$here")/deny.toml" >"$tmp/commented-deny.toml"
check_comment() {
  set +e
  BURROW_DENY_TOML="$tmp/commented-deny.toml" BURROW_MANIFESTS="$tmp/clean/Cargo.toml" \
    "$checker" >"$tmp/out" 2>"$tmp/err"
  local got=$?
  set -e
  if [ "$got" = "0" ]; then
    echo "  ok    a crate named only in a comment is not treated as banned"
    pass=$((pass + 1))
  else
    echo "  FAIL  a commented crate name caused a false positive (exit $got)"
    sed 's/^/          /' "$tmp/err" | head -10
    fail=$((fail + 1))
  fi
}
check_comment

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ] || exit 1
