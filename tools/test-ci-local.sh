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

# THE SIXTH, AND THE ONLY ONE THAT SLIPPED PAST THIS CHECK RATHER THAN PAST A HABIT.
#
# The subsetting gate was a `run:` block whose body was `cargo test -p burrow-ops … --test X --
# --exact Y`. The extractor maps that to the `cargo:test` token the local `test` job already
# covers, so parity reported FULL COVERAGE while the gate itself had no local counterpart -- and
# the branch that closed #54 went red on it after a clean `tools/ci-local.py`.
#
# The fix was structural rather than a new pattern: the gate moved into
# `tools/check-subsetting-gate.sh`, which the extractor sees as its own command. This case is the
# shape that got through, so that a future gate written as a bare `cargo test` line is refused
# rather than absorbed.
#
# It is DELIBERATELY a `cargo test` invocation with distinguishing arguments. A pattern that
# looked only at the subcommand would swallow it again, which is the whole point.
check "a cargo-test gate CI runs that no local job covers is refused" \
  "||
      - name: A gate nothing local runs
        run: cargo test -p burrow-ops --all-features --test brandnew_gate -- --exact some_case
" \
  "test:brandnew_gate" 1

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

# --- The environment, not ci.yml: a job whose tool is missing must REFUSE, not fail ----------
#
# `needs_qpdf_cli` was declared on three jobs and read by nothing. On a machine without the
# CLI those three did not refuse -- they ran, and five tests panicked inside a testsupport
# helper about a missing binary, which reads as "the change broke the leak tests". A flag
# nothing reads is the same defect the reviewers found twice elsewhere in this batch, and it
# is worse in the one file whose purpose is refusing to run a sweep it cannot complete.
#
# This hides BOTH routes to the CLI -- the PATH and the pinned build engines/build-native.sh
# produces -- and requires a refusal that names the tool.
# THE REAL ci.yml, restored first. `check()` restores at the START of each call rather than
# the end, so the last mutation is still in place here -- and this case's refusal happens
# AFTER the parity check, which would fail on it for an unrelated reason.
cp "$backup" "$ci"

echo
hidden=""
built="$(ls -1 "$repo"/engines/vendor/src/build-qpdf-*/qpdf/qpdf 2>/dev/null | head -1 || true)"
if [ -n "$built" ] && [ -x "$built" ]; then
  hidden="$built.hidden-by-test-ci-local"
  mv "$built" "$hidden"
  # RESTORED ON EVERY EXIT, alongside the ci.yml copy the outer trap already handles.
  trap 'cp "$backup" "$ci"; rm -f "$backup"; [ -n "$hidden" ] && [ -e "$hidden" ] && mv "$hidden" "${hidden%.hidden-by-test-ci-local}"' EXIT
fi

# A PATH holding the interpreter and NOTHING ELSE, so `shutil.which("qpdf")` genuinely finds
# nothing. Emptying PATH outright would make the failure "python3 not found", which is a
# different refusal and would have passed a status check while proving nothing.
py="$(command -v python3)"
empty="$(mktemp -d)"
ln -s "$py" "$empty/python3"
status=0
out="$(PATH="$empty" "$py" "$here/ci-local.py" --only subsetting-gate 2>&1)" || status=$?
rm -rf "$empty"
if [ "$status" -eq 1 ] && grep -q "REFUSED" <<<"$out" && grep -q "qpdf" <<<"$out"; then
  echo "  ok   a job whose qpdf CLI is missing is refused, not run"
  pass=$((pass + 1))
else
  echo "  FAIL a job whose qpdf CLI is missing was not refused (status $status)"
  echo "$out" | tail -10
  fail=$((fail + 1))
fi

if [ -n "$hidden" ] && [ -e "$hidden" ]; then
  mv "$hidden" "${hidden%.hidden-by-test-ci-local}"
  hidden=""
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
