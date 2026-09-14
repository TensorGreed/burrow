#!/usr/bin/env bash
# Adversarial self-test for tools/ci-local.py.
#
# This tool exists because four CI failures in two batches were each a local sweep that had
# skipped a job. Its whole value is that it REFUSES when ci.yml gains a gate nothing local
# covers -- so the cases below re-plant those four misses, plus drift in the other direction.
#
# The mutation is applied to a COPY OF ci.yml, not to the checker, because the checker's job
# is to read that file. The copy lives beside the original so relative paths resolve.
#
# Usage: tools/test-ci-local.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
ci="$repo/.github/workflows/ci.yml"
backup="$(mktemp)"
cp "$ci" "$backup"
trap 'cp "$backup" "$ci"; rm -f "$backup"' EXIT

pass=0
fail=0

# `name <python-mutation-of-ci.yml> <expected text> <expected exit>`
check() {
  local name="$1" mutation="$2" expect="$3" want_status="$4"

  cp "$backup" "$ci"
  python3 - "$ci" "$mutation" <<'PYEOF'
import pathlib, sys
path, mutation = sys.argv[1], sys.argv[2]
text = pathlib.Path(path).read_text()
old, new = mutation.split("||", 1)
if old and old not in text:
    sys.exit(f"the mutation target is not in ci.yml: {old!r}")
pathlib.Path(path).write_text(text.replace(old, new, 1) if old else text + new)
PYEOF

  # THE MUTATION MUST HAVE APPLIED. A replace that matched nothing leaves an identical file and
  # the case proves nothing -- the failure this repository has been caught by twice.
  if cmp -s "$ci" "$backup"; then
    echo "  FAIL $name: the mutation did not apply"
    fail=$((fail + 1))
    return
  fi

  local out status
  set +e
  out="$(python3 "$here/ci-local.py" --check 2>&1)"
  status=$?
  set -e

  if [ "$status" -ne "$want_status" ]; then
    echo "  FAIL $name: expected exit $want_status, got $status"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
    return
  fi
  if [ -n "$expect" ] && ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: exit status was right but the output did not mention: $expect"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# The unmutated file must pass, or every case below measures a broken baseline.
cp "$backup" "$ci"
if out="$(python3 "$here/ci-local.py" --check 2>&1)"; then
  echo "  ok   the real ci.yml is fully covered locally"
  pass=$((pass + 1))
else
  echo "  FAIL the real ci.yml is not fully covered, so no case below means anything"
  sed 's/^/        /' <<<"$out" | tail -6
  exit 1
fi

# --- The four misses, re-planted -----------------------------------------------------------
#
# Each of these is a real CI failure from M1. If this tool had existed, each would have been
# refused locally before the push.

check "a new shell checker with no local counterpart is refused" \
  "||
      - name: A brand new gate
        run: tools/check-something-new.sh
" \
  "tools/check-something-new.sh" 1

check "a new Python checker with no local counterpart is refused" \
  "||
      - name: Another new gate
        run: python3 tools/check-something-else.py
" \
  "tools/check-something-else.py" 1

# THE #63 MISS, EXACTLY. `reorder` was added to fuzz/Cargo.toml and to no run list; here the
# inverse -- a target CI runs that nothing local does.
#
# APPENDS A STEP rather than editing an existing loop, and that was a correction. The first
# version mutated the literal line `for target in qpdf_check rotate reorder; do` -- and when
# seeded fuzzing moved to nightly that line gained two more targets, the mutation target
# vanished, and this case aborted. It failed loudly, which is the behaviour the mutation
# helper was given for exactly this; but a fixture pinned to the current spelling of a line
# somebody else owns will keep breaking. An appended step cannot go stale.
check "a fuzz target CI runs but nothing local does is refused" \
  "||
      - name: A new fuzz target
        run: |
          for target in brandnew; do
            cargo +nightly fuzz run \"\$target\"
          done
" \
  "fuzz:brandnew" 1

# THE pnpm check MISS. A web gate CI runs and the local web job does not.
check "a new pnpm script CI runs is refused" \
  "||
      - name: A new web gate
        run: pnpm typecheck
" \
  "" 1

# THE wasm-pack MISS, and the fifth of the class. `wasm-pack build` is not a cargo subcommand,
# so the pattern list matched nothing and the parity check reported full coverage while the
# local sweep staged whatever `bindings/burrow-wasm/pkg/` happened to hold. ADR 0022 grew that
# binding by 9.5% and CI was the only thing that noticed.
check "a wasm-pack build CI runs but nothing local does is refused" \
  "||
      - name: Build another wasm binding
        run: wasm-pack build bindings/brandnew --target no-modules --out-dir pkg --release
" \
  "wasm-pack:bindings/brandnew" 1

# --- Drift in the other direction -----------------------------------------------------------
#
# A local command covering something CI no longer runs is dead weight that reads as coverage.
check "a local claim CI no longer backs is refused" \
  "python3 tools/check-handle-identity.py||python3 tools/check-handle-identity-renamed.py" \
  "tools/check-handle-identity.py" 1

# --- An action-based gate, which is not in any run: block ------------------------------------
check "removing the cargo-audit action is noticed" \
  "uses: rustsec/audit-check@||uses: rustsec/audit-check-removed@" \
  "cargo:audit" 1

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
