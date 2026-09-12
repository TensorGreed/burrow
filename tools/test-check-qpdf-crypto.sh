#!/usr/bin/env bash
# Adversarial self-test for tools/check-qpdf-crypto.sh.
#
# A checker with no negative test is a checker nobody has shown can fail. This repository
# already pairs its checkers this way -- tools/test-detect-engine-components.sh,
# tools/test-check-no-network-deps.sh -- and the first version of check-qpdf-crypto.sh shipped
# without one. Security review then found three ways it passed on input it should reject, and
# every one of them is a case below. That is the argument for this file, stated as a fact
# rather than a principle.
#
# Each case builds a synthetic archive or module, runs the real checker against it, and
# asserts BOTH the exit status and that the message names the right thing. Asserting the exit
# status alone would let a check fail for an unrelated reason and still look correct.
#
# Usage: tools/test-check-qpdf-crypto.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-qpdf-crypto.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

# Compile a C file into an object, or skip the whole suite if there is no compiler.
cc_obj() {
  local src="$1" obj="$2"
  cc -c "$src" -o "$obj" 2>/dev/null || return 1
}

check() {
  local name="$1" expect_status="$2" expect_text="$3"
  shift 3
  local out status
  set +e
  out="$("$checker" "$@" 2>&1)"
  status=$?
  set -e

  if [ "$status" -ne "$expect_status" ]; then
    echo "  FAIL $name: expected exit $expect_status, got $status"
    echo "$out" | sed 's/^/        /'
    fail=$((fail + 1))
    return
  fi
  if [ -n "$expect_text" ] && ! grep -qF "$expect_text" <<<"$out"; then
    echo "  FAIL $name: exit status was right but the message did not mention:"
    echo "        $expect_text"
    echo "$out" | sed 's/^/        /'
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

if ! command -v cc >/dev/null 2>&1 || ! command -v ar >/dev/null 2>&1; then
  echo "test-check-qpdf-crypto: cc and ar are required to build the synthetic archives" >&2
  exit 1
fi

echo "building fixtures in $work"

# A minimal stand-in for the native provider: a defined symbol whose name contains
# QPDFCrypto_native, in a member named like qpdf's.
cat >"$work/native.c" <<'EOF'
void _ZN17QPDFCrypto_native6MD5_initEv(void) {}
EOF
cc_obj "$work/native.c" "$work/QPDFCrypto_native.cc.o" || { echo "cc failed" >&2; exit 1; }
ar rcs "$work/clean.a" "$work/QPDFCrypto_native.cc.o" 2>/dev/null

check "a clean archive passes" 0 "ok" "$work/clean.a"

# --- Case 1: the forbidden provider, by name -------------------------------------------
cat >"$work/gnutls.c" <<'EOF'
void _ZN17QPDFCrypto_gnutls6MD5_initEv(void) {}
EOF
cc_obj "$work/gnutls.c" "$work/QPDFCrypto_gnutls.cc.o" || exit 1
cp "$work/clean.a" "$work/withgnutls.a"
ar rs "$work/withgnutls.a" "$work/QPDFCrypto_gnutls.cc.o" 2>/dev/null
check "a linked QPDFCrypto_gnutls fails" 1 "QPDFCrypto_gnutls is LINKED" "$work/withgnutls.a"

# The same finding through the SYMBOL rather than the member name. The fixture above is both
# named and defined for gnutls, so it cannot distinguish the two; this member is called
# something neutral and only its symbol gives it away.
cp "$work/gnutls.c" "$work/neutral.c"
cc_obj "$work/neutral.c" "$work/some_object.cc.o" || exit 1
cp "$work/clean.a" "$work/gnutls-symbol-only.a"
ar rs "$work/gnutls-symbol-only.a" "$work/some_object.cc.o" 2>/dev/null
check "a gnutls SYMBOL in a neutrally-named member fails" \
  1 "QPDFCrypto_gnutls is LINKED" "$work/gnutls-symbol-only.a"

# --- Case 2: a RENAMED wrapper, caught only by the symbol sweep -------------------------
#
# THE REGRESSION TEST FOR FINDING 1. The first version read symbols with `--defined-only`,
# and gnutls symbols are undefined references in a static archive -- so this case passed.
# The sweep now runs over the full table. If someone reintroduces `--defined-only`, this is
# the case that goes red.
cat >"$work/renamed.c" <<'EOF'
extern int gnutls_hash_init(void *, int);
void _ZN17QPDFCrypto_native6MD5_initEv(void) {}
int burrow_uses_tls(void) { return gnutls_hash_init(0, 0); }
EOF
cc_obj "$work/renamed.c" "$work/renamed.cc.o" || exit 1
ar rcs "$work/renamed.a" "$work/renamed.cc.o" 2>/dev/null
check "an undefined gnutls reference fails (finding 1)" 1 "TLS library symbols present" "$work/renamed.a"

# --- Case 3: no crypto provider at all --------------------------------------------------
cat >"$work/empty.c" <<'EOF'
void burrow_nothing(void) {}
EOF
cc_obj "$work/empty.c" "$work/empty.cc.o" || exit 1
ar rcs "$work/noprovider.a" "$work/empty.cc.o" 2>/dev/null
check "an archive with no provider fails" 1 "is not linked" "$work/noprovider.a"

# --- Case 4: a member header is not a symbol (finding 3) --------------------------------
#
# The member is NAMED QPDFCrypto_native.cc.o but defines nothing of the sort. `nm` prints the
# header line `QPDFCrypto_native.cc.o:`, which satisfied the first version's unanchored grep.
cp "$work/empty.cc.o" "$work/QPDFCrypto_native.cc.o.decoy"
cp "$work/empty.cc.o" "$work/decoydir_QPDFCrypto_native.cc.o"
(cd "$work" && cp empty.cc.o QPDFCrypto_native.cc.o && ar rcs headeronly.a QPDFCrypto_native.cc.o 2>/dev/null)
check "a member NAMED for the provider but defining nothing fails (finding 3)" \
  1 "is not linked" "$work/headeronly.a"

# --- Case 4b: a REFERENCE to the provider is not a definition ---------------------------
#
# Pins the presence regex rather than the caller. `present_re` requires a hex ADDRESS, and
# `nm` renders an undefined symbol with none -- so an archive that merely calls into the
# native provider without defining it must fail. Without this case, reading the presence
# check from the full symbol table instead of the defined-only one is an undetectable change.
cat >"$work/refonly.c" <<'EOF'
extern void _ZN17QPDFCrypto_native6MD5_initEv(void);
void burrow_calls_it(void) { _ZN17QPDFCrypto_native6MD5_initEv(); }
EOF
cc_obj "$work/refonly.c" "$work/refonly.cc.o" || exit 1
ar rcs "$work/refonly.a" "$work/refonly.cc.o" 2>/dev/null
check "an archive that only REFERENCES the provider fails" 1 "is not linked" "$work/refonly.a"

# --- Case 5: a partial read is not a pass (finding 2) -----------------------------------
#
# An archive with one readable member and one `nm` cannot parse. `nm` skips the bad member,
# complains only on stderr, and exits 0 -- so a partial table looked complete. The checker now
# compares header lines against `ar t` and refuses when they disagree.
cp "$work/clean.a" "$work/partial.a"
printf 'not an object file at all\n' >"$work/QPDFCrypto_gnutls.cc.o"
ar rs "$work/partial.a" "$work/QPDFCrypto_gnutls.cc.o" 2>/dev/null
check "an archive with an unreadable member fails (finding 2)" \
  1 "COMPLETE symbol table" "$work/partial.a"

# --- Case 6: the LINKED module, which is what a user downloads (finding 5) ---------------
#
# The first version scanned archives only. `qpdf.wasm` is the artifact that ships, and a
# library introduced at the emcc link step is invisible in `libqpdf.a`. A `.wasm` has no
# symbol table to read, so the checker scans strings -- C++ typeinfo names survive the link.
#
# Synthesised rather than copied from engines/vendor/, so this suite runs on a clean checkout
# where the vendor tree does not exist.
printf 'irrelevant padding\n_ZN17QPDFCrypto_native6MD5_initEv\nmore padding\n' >"$work/clean.wasm"
check "a clean linked module passes" 0 "ok" "$work/clean.wasm"

cp "$work/clean.wasm" "$work/dirty.wasm"
printf '_ZN17QPDFCrypto_gnutls6MD5_initEv\ngnutls_hash_init\n' >>"$work/dirty.wasm"
check "a linked module carrying gnutls fails (finding 5)" 1 "QPDFCrypto_gnutls is LINKED" \
  "$work/dirty.wasm"

printf 'no provider anywhere in here at all\n' >"$work/bare.wasm"
check "a linked module with no provider fails" 1 "is not linked" "$work/bare.wasm"

# A RENAMED wrapper in a MODULE -- the twin of case 2, and the one that was missing.
#
# In CI the `web` job examines the module and nothing else, so the TLS symbol sweep on this
# path is the only thing standing between a renamed gnutls wrapper and a shipped LGPL
# library. Case 6's `dirty.wasm` carries BOTH `QPDFCrypto_gnutls` and `gnutls_hash_init`, so
# it is satisfied by the provider-name check alone and says nothing about the sweep. This
# fixture deliberately carries NO `QPDFCrypto_gnutls`, so only the sweep can catch it.
#
# A mutation restricting the sweep to `kind = archive` survived the suite until this existed.
printf 'padding\n_ZN17QPDFCrypto_native6MD5_initEv\ngnutls_hash_init\nmore\n' >"$work/renamed.wasm"
check "a module referencing gnutls under another name fails (finding 1, module path)" \
  1 "TLS library symbols present" "$work/renamed.wasm"

# The second forbidden provider. Without a case, dropping it from FORBIDDEN_PROVIDERS is
# invisible -- and OpenSSL is the one qpdf reaches for when GnuTLS is absent.
printf 'padding\n_ZN18QPDFCrypto_openssl6MD5_initEv\n_ZN17QPDFCrypto_native6MD5_initEv\n' \
  >"$work/openssl.wasm"
check "a linked QPDFCrypto_openssl fails" 1 "QPDFCrypto_openssl is LINKED" "$work/openssl.wasm"

# A module with no readable strings at all. A text fixture cannot reach the empty-output
# guard, so this one is deliberately binary: NULs and control bytes only, nothing `strings`
# will emit. Without the guard, an unreadable module passes every absence check.
printf '\x00\x01\x02\x03\x00\x01\x02\x03\x00\x01\x02\x03' >"$work/opaque.wasm"
check "a module yielding no strings fails, rather than passing vacuously" \
  1 "read NO strings" "$work/opaque.wasm"

# --- Case 7: nothing to examine is a failure, not a pass --------------------------------
check "a path that does not exist fails" 1 "COMPLETE symbol table" "$work/does-not-exist.a"

# --- the per-run probe gate must exist --------------------------------------------------
#
# The checker verifies each of its three rules against a fixture and a near-miss on every
# invocation. That gate is only observable when a rule is broken, so deleting it left every
# case above green. This breaks a rule in a COPY and asserts the copy refuses, naming the
# reason. The copy sits beside the original because the script resolves `repo` from its own
# location -- a copy in /tmp exits non-zero for the wrong reason, which an exit-code-only
# assertion would report as a pass.
check_probe_gate() {
  local copy="$here/.probe-gate-fixture.sh"
  sed "s/FORBIDDEN_SYMBOL_RE='gnutls_|/FORBIDDEN_SYMBOL_RE='ZZZnomatch|/" "$checker" >"$copy"
  chmod +x "$copy"
  if cmp -s "$copy" "$checker"; then
    echo "  FAIL fixture: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  local got
  if got="$("$copy" "$work/clean.a" 2>&1)"; then
    echo "  FAIL a rule that matches nothing does not stop the checker"
    fail=$((fail + 1))
  elif grep -qF "does not match its own fixture" <<<"$got"; then
    echo "  ok   a rule that matches nothing stops the checker before it examines anything"
    pass=$((pass + 1))
  else
    echo "  FAIL it failed, but not for the stated reason"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
  rm -f "$copy"
}
check_probe_gate

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
