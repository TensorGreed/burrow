#!/usr/bin/env bash
# Assert that every shipped qpdf artifact carries the native crypto provider and no other.
#
# WHY THIS EXISTS SEPARATELY FROM THE BUILD SCRIPTS
#
# ROADMAP M1 item 3 asks for two things: "CI fails if the crypto summary changes OR
# `QPDFCrypto_gnutls` appears in the link". Only the first was built.
#
# `engines/build-{native,wasm}.sh` grep qpdf's CMake configure summary for
# "GNU TLS crypto enabled: OFF". That is a good check and it is not this one: it asserts what
# CMake DECIDED, not what reached the artifact. Those differ whenever the decision does not
# take effect -- a stale build directory, a cached object, a CMake cache entry that survived
# a flag change.
#
# AND, THE PART THAT MATTERS MORE: the configure assertion lives inside a build step gated on
# `if: steps.engines-cache.outputs.cache-hit != 'true'`. On an ordinary PR the engine cache
# hits, the build does not run, and **the crypto assertion did not run either**. This script
# reads artifacts, so a restored cache is still examined -- the same reasoning the `test` job
# already applies to `engines/fetch.sh`.
#
# WHY IT HAS A LICENCE CONSEQUENCE, NOT JUST A SECURITY ONE
#
# **GnuTLS is LGPL-2.1-or-later**, and qpdf links it BY DEFAULT. ADR 0008 names it as the
# reason `USE_IMPLICIT_CRYPTO=OFF` and `REQUIRE_CRYPTO_NATIVE=ON` are load-bearing rather
# than tidy. A build that quietly picked it up would put a copyleft library inside a
# statically linked binary and a WebAssembly module, and **no other check here would catch
# it**: cargo-deny sees Rust crates, and engines/licenses.toml is hand-maintained and would
# simply not mention it.
#
# THREE WAYS A CHECK LIKE THIS PASSES WITHOUT EXAMINING ANYTHING
#
# All three were found by security review of the first version of this script, and all three
# are closed below. They are recorded because each is easy to reintroduce:
#
#   1. **`--defined-only` kills the symbol sweep.** gnutls/OpenSSL symbols are DEFINED in
#      libgnutls/libssl and are only ever `U` in libqpdf.a. Measured: `deflate` appears 0
#      times under `--defined-only` and 3 times in the full table. So the sweep runs over the
#      FULL table; only the provider checks use defined-only.
#   2. **A partial read looks like a clean one.** `nm` prints "file format not recognized" to
#      stderr, skips that member, and still exits 0. If the skipped member were
#      `QPDFCrypto_gnutls.cc.o`, every absence check would pass. So a reader must exit 0 AND
#      account for every archive member.
#   3. **A member header is not a symbol.** `nm` emits `QPDFCrypto_native.cc.o:` as a header
#      line, which satisfies an unanchored grep for the provider name with zero symbols
#      behind it. The presence check is anchored to a real symbol line.
#
# It also scans the LINKED artifacts, not only the archives: `qpdf.wasm` is what a user
# downloads, and a library introduced at the emcc link step is invisible in `libqpdf.a`.
#
# Adversarial self-test: tools/test-check-qpdf-crypto.sh, following the convention set by
# tools/test-detect-engine-components.sh. A checker with no negative test is a checker nobody
# has shown can fail.
#
# Usage: tools/check-qpdf-crypto.sh [path ...]
# With no arguments, checks every libqpdf.a and qpdf*.wasm under the staged vendor prefixes.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

REQUIRED_PROVIDER="QPDFCrypto_native"
FORBIDDEN_PROVIDERS=("QPDFCrypto_gnutls" "QPDFCrypto_openssl")

# Symbols that betray a linked TLS library even if the provider wrapper were renamed.
# Deliberately broad: a false positive is a conversation, a false negative is a licence
# violation. Verified to produce zero hits across every current artifact.
FORBIDDEN_SYMBOL_RE='gnutls_|_gnutls|EVP_CipherInit|EVP_DigestInit|SSL_CTX_new|OPENSSL_init'

# PROBES. Every rule below must catch its own fixture and reject a near-miss, on EVERY run.
#
# tools/test-check-qpdf-crypto.sh already covers these adversarially -- but it runs in CI, and
# these run wherever the checker runs, including a developer's machine with no self-test in
# sight. The distinction earned its keep elsewhere in this repository: a pattern set that
# reported "16 patterns" while 15 were inert, and an ICU fingerprint whose own documented
# example did not match it. Both were found by fixtures, not by reading.
probe_problems=0
probe() {
  local name="$1" text="$2" regex="$3" want="$4"
  if grep -qE "$regex" <<<"$text"; then
    [ "$want" = "match" ] || { echo "::error::$name matches text it must reject: $text" >&2; probe_problems=$((probe_problems + 1)); }
  else
    [ "$want" = "reject" ] || { echo "::error::$name does not match its own fixture: $text" >&2; probe_problems=$((probe_problems + 1)); }
  fi
}

# ONE DEFINITION, used by the probe AND by the loop below.
#
# The first version of these probes inlined a COPY of this regex. Mutating the loop's own
# `present_re` then left every probe green -- a probe testing a copy of the rule passes while
# the real rule is broken, which is the precise failure probes exist to prevent. Found by a
# mutation sweep, not by reading.
ARCHIVE_PRESENCE_RE="^[0-9a-fA-F]+ +[A-Za-z] .*$REQUIRED_PROVIDER"

# The archive presence rule: a DEFINING symbol line, which carries a hex address. An
# undefined reference carries none, and must not count as "linked".
probe "the archive presence rule" \
  "0000000000000000 W _ZN17QPDFCrypto_nativeD2Ev" "$ARCHIVE_PRESENCE_RE" match
probe "the archive presence rule" \
  "                 U _ZN17QPDFCrypto_native6MD5_initEv" "$ARCHIVE_PRESENCE_RE" reject
probe "the archive presence rule" \
  "QPDFCrypto_native.cc.o:" "$ARCHIVE_PRESENCE_RE" reject

# The forbidden-provider rule, deliberately unanchored: a member header alone is a finding.
for p in "${FORBIDDEN_PROVIDERS[@]}"; do
  probe "the $p rule" "0000000000000000 T _ZN18${p}6MD5_initEv" "$p" match
  probe "the $p rule" "QPDFCrypto_native.cc.o:" "$p" reject
done

# The TLS symbol sweep, which must fire on an UNDEFINED reference -- the only form these
# symbols can take in a static archive.
probe "the TLS symbol sweep" "                 U gnutls_hash_init" "$FORBIDDEN_SYMBOL_RE" match
probe "the TLS symbol sweep" "0000000000000000 T EVP_CipherInit_ex" "$FORBIDDEN_SYMBOL_RE" match
probe "the TLS symbol sweep" "0000000000000000 T _ZN17QPDFCrypto_native6MD5_initEv" "$FORBIDDEN_SYMBOL_RE" reject

if [ "$probe_problems" -ne 0 ]; then
  echo "check-qpdf-crypto: $probe_problems rule(s) do not behave as declared; refusing to run." >&2
  echo "  A rule that matches nothing passes every artifact. A rule that matches everything" >&2
  echo "  fails every artifact. Neither is a check." >&2
  exit 1
fi

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  # The STAGED prefixes only -- what is linked into something we ship or fuzz.
  # `engines/vendor/src/build-*/` holds intermediates no artifact comes from.
  mapfile -t targets < <(
    find "$repo/engines/vendor/native-"* "$repo/engines/vendor/wasm" \
      \( -name libqpdf.a -o -name 'qpdf*.wasm' \) -type f 2>/dev/null | sort
  )
fi

if [ ${#targets[@]} -eq 0 ]; then
  echo "check-qpdf-crypto: no qpdf artifacts found under engines/vendor/." >&2
  echo "  Run engines/fetch.sh and engines/build-native.sh (or build-wasm.sh) first." >&2
  echo "  Refusing to pass with nothing examined -- that is the vacuous outcome." >&2
  exit 1
fi

# Read an archive's symbol table, with every member accounted for.
#
# $1 archive, $2 "defined" or "all". Prints the table; returns non-zero if no available
# reader produced a COMPLETE one. `nm` cannot read wasm archives and `llvm-nm` can, so the
# reader is chosen by trying rather than by guessing from the path -- a first version guessed
# from the directory name and silently picked plain `nm` for `build-qpdf-wasm/`.
read_table() {
  local archive="$1" mode="$2" out members headers
  local -a flags=()
  [ "$mode" = "defined" ] && flags=(--defined-only)

  members="$(ar t "$archive" 2>/dev/null | wc -l)"
  if [ "$members" -eq 0 ]; then
    return 1 # `ar` could not read it either; the caller reports and exits.
  fi

  for reader in nm llvm-nm llvm-nm-18 llvm-nm-17 "${EMSDK:-/nonexistent}/upstream/bin/llvm-nm"; do
    command -v "$reader" >/dev/null 2>&1 || [ -x "$reader" ] || continue

    # Exit status is checked rather than swallowed -- but it is a cheap EARLY guard, not an
    # independent defence, and a mutation sweep confirms it: removing it leaves every case in
    # tools/test-check-qpdf-crypto.sh green, because the coverage assertion below already
    # rejects everything this would. Kept because it is one token and it fails faster on the
    # common case; do not read it as a second layer.
    out="$("$reader" "${flags[@]}" "$archive" 2>/dev/null)" || continue
    [ -n "$out" ] || continue

    # COVERAGE. `nm` skips a member it cannot parse, says so only on stderr, and still exits
    # 0 -- which is exactly the shape a missed `QPDFCrypto_gnutls.cc.o` would take. Every
    # member must appear as a header line, or this reader has not answered either.
    headers="$(grep -c ':$' <<<"$out" || true)"
    [ "$headers" -eq "$members" ] || continue

    printf '%s' "$out"
    return 0
  done
  return 1
}

failed=0
examined=0

for target in "${targets[@]}"; do
  rel="${target#"$repo"/}"

  case "$target" in
    *.wasm)
      # A LINKED module, not an archive: no symbol table to read, and `strings` suffices
      # because C++ typeinfo names survive the link. Verified: `QPDFCrypto_native` is present
      # in qpdf.wasm and no gnutls string is.
      symbols="$(strings -a "$target" 2>/dev/null || true)"
      if [ -z "$symbols" ]; then
        echo "check-qpdf-crypto: read NO strings out of $rel; refusing to pass it." >&2
        exit 1
      fi
      defined="$symbols"
      kind="module"
      ;;
    *)
      if ! defined="$(read_table "$target" defined)" || ! symbols="$(read_table "$target" all)"; then
        echo "check-qpdf-crypto: could not read a COMPLETE symbol table for $rel." >&2
        echo "  Tried nm and llvm-nm; a wasm archive needs llvm-nm (emsdk ships one)." >&2
        echo "  A partial read is an unanswered question, not a pass." >&2
        exit 1
      fi
      kind="archive"
      ;;
  esac

  examined=$((examined + 1))
  target_failed=0

  # 1. The provider we require must be LINKED.
  #
  #    For an ARCHIVE this is anchored to a symbol line, because `nm` emits
  #    `QPDFCrypto_native.cc.o:` as a member header and an unanchored match would accept that
  #    with no symbols behind it -- a decoy member named for the provider would pass.
  #
  #    For a linked MODULE there is no symbol table and no member header, so there is nothing
  #    to anchor to and nothing to be fooled by: a plain substring match is both correct and
  #    the only option. Note the name arrives inside a mangled symbol
  #    (`_ZN17QPDFCrypto_native6MD5_initEv`), so a word-boundary match would be WRONG here --
  #    the characters either side are alphanumeric.
  if [ "$kind" = "archive" ]; then
    # A hex ADDRESS is required, not just a type letter. `nm` renders an undefined symbol
    # as `                 U name` with no address, so `^[0-9a-fA-F]* *[A-Za-z] ` would match
    # a mere *reference* to the provider and the comment below would be false. Measured: the
    # address form matches 35 defining lines and 0 undefined ones.
    present_re="$ARCHIVE_PRESENCE_RE"
  else
    present_re="$REQUIRED_PROVIDER"
  fi
  if ! grep -qE "$present_re" <<<"$defined"; then
    echo "  FAIL $rel: $REQUIRED_PROVIDER is not linked (no defining symbol)." >&2
    target_failed=1
  fi

  # 2. No other provider. Deliberately UNANCHORED: a `QPDFCrypto_gnutls.cc.o` member header
  #    alone is already a finding, and matching it is the conservative direction.
  for provider in "${FORBIDDEN_PROVIDERS[@]}"; do
    if grep -q "$provider" <<<"$symbols"; then
      echo "  FAIL $rel: $provider is LINKED." >&2
      echo "        GnuTLS is LGPL-2.1-or-later and must never ship (ADR 0008)." >&2
      echo "        Check USE_IMPLICIT_CRYPTO=OFF and REQUIRE_CRYPTO_NATIVE=ON." >&2
      target_failed=1
    fi
  done

  # 3. No TLS library symbols under any name. Over the FULL table -- these are undefined
  #    references in a static archive, so `--defined-only` would make this check dead.
  if hits="$(grep -oE "$FORBIDDEN_SYMBOL_RE" <<<"$symbols" | sort -u)" && [ -n "$hits" ]; then
    echo "  FAIL $rel: TLS library symbols present: $(tr '\n' ' ' <<<"$hits")" >&2
    target_failed=1
  fi

  # Per target, not global: with one shared flag, every artifact after the first failure
  # printed nothing and read as skipped rather than clean.
  if [ "$target_failed" -eq 0 ]; then
    echo "  ok   $rel"
  else
    failed=1
  fi
done

# Unreachable given the non-empty `targets` guard above, which exits before the loop --
# belt and braces, kept because "examined nothing" is the failure this whole file is shaped
# against and a future edit to the target selection could reintroduce it.
if [ "$examined" -eq 0 ]; then
  echo "check-qpdf-crypto: examined ZERO artifacts; the check would be vacuous." >&2
  exit 1
fi

if [ "$failed" -ne 0 ]; then
  echo "" >&2
  echo "FAILED -- see above. ROADMAP M1 item 3; ADR 0008 names GnuTLS specifically." >&2
  exit 1
fi

echo "OK -- $examined qpdf artifact(s) carry native crypto only"
echo "     (3 rules, each verified against a fixture and a near-miss)"
