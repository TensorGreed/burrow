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

# A GATE OUTSIDE `tools/`. The extractor matched only `tools/*.sh` until the force-push hook's
# self-test needed wiring in, and `.claude/hooks/` is where a hook's self-test has to live --
# a hook path is what `.claude/settings.json` names. Parity REFUSED that wiring, correctly and
# for the wrong reason: CI ran the script, the runner claimed to cover it, and the extractor
# saw neither, so the claim read as unbacked. That is CLAUDE.md's fifth row -- a gate the
# extractor cannot see -- caught by the check this time rather than by a red CI run. This case
# is the re-plant: a hook script CI runs and nothing local covers must be refused.
check "a gate under .claude/hooks/ with no local counterpart is refused" \
  "||
      - name: A hook self-test nothing local runs
        run: .claude/hooks/test-something-new.sh
" \
  ".claude/hooks/test-something-new.sh" 1

# A GATE INSIDE A LOCAL COMPOSITE ACTION. The wasm-engine build moved into
# `.github/actions/build-web-payload/action.yml` so CI and the deploy could not drift -- they
# already had, in four places including the GnuTLS assertion. The moment it moved, the gates
# inside it vanished from `ci.yml`'s text and parity refused four TRUE local claims as
# unbacked. CLAUDE.md's fifth row a third time: a gate the extractor cannot see.
#
# The extractor follows `uses: ./...` now. This is the re-plant: a gate that exists only
# inside a composite action must be visible, and one with no local counterpart must be
# refused.
check "a gate inside a local composite action with no local counterpart is refused" \
  "||
      - name: Run the shared build
        run: tools/check-inside-a-composite.sh
" \
  "tools/check-inside-a-composite.sh" 1

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
  "wasm-pack:bindings/brandnew:pkg" 1

# THE SEVENTH OF THE CLASS, AND THE FIRST WHERE THE CRATE WAS NOT THE DISTINGUISHER. ADR 0026
# builds `bindings/burrow-wasm` TWICE, under mutually exclusive cargo features, into two output
# directories -- both emit `burrow_wasm_bg.wasm`, so one directory is not an option. Keyed on
# the crate alone the two collapse to one gate, and a local sweep running only the base build
# would report full coverage while staging whatever the render directory happened to hold: the
# `pkg/`-is-gitignored miss that put this pattern here in the first place, one argument along.
check "a SECOND build of a crate CI already builds is refused when only the first is covered" \
  "||
      - name: Build the same binding a different way
        run: wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg-other --release
" \
  "wasm-pack:bindings/burrow-wasm:pkg-other" 1

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

# --- A step NAME is prose, and must not invent a gate ----------------------------------------
#
# The patterns used to run over `- name:` lines as well as `run:` bodies, so a step called
# "The installed node and pnpm match the pins" yielded the token `pnpm:match` and parity
# reported a gate with no local counterpart -- a phantom satisfiable only by inventing a job
# to cover it. Found by adding exactly such a step. A label can never be a gate.
check "a command-shaped step NAME does not invent a gate" \
  "||
      - name: Run pnpm typecheck for the web app
        run: echo nothing
" \
  "" 0

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

# --- ci.yml's workflow-level `env:` must actually reach the local jobs -----------------------
#
# It carries `RUSTFLAGS: -D warnings`, and this runner did not apply it, so every local cargo
# job ran under weaker lints than CI. Measured: `tools/test-prune-is-reached.sh` passed a full
# green local sweep and failed in CI, because its mutation left a struct field unread and
# `-D dead-code` is only fatal on the CI side. A local runner that is quieter than CI is the
# exact failure this file exists to make impossible.
#
# The assertion is that the value is READ FROM ci.yml rather than hardcoded, so a sentinel is
# planted and has to come back out.
cp "$backup" "$ci"
python3 - "$ci" <<'PYENV'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "RUSTFLAGS: -D warnings"
assert old in text, "ci.yml no longer sets RUSTFLAGS at workflow level"
path.write_text(text.replace(old, "RUSTFLAGS: -D warnings --cfg burrow_env_probe", 1))
PYENV
if grep -q 'burrow_env_probe' "$ci"; then
  out="$("$here/ci-local.py" --only fmt 2>&1 || true)"
  if grep -q 'ci.yml env applied' <<<"$out" && grep -q 'burrow_env_probe' <<<"$out"; then
    echo "  ok   ci.yml's workflow env reaches the local jobs, read from the file"
    pass=$((pass + 1))
  else
    echo "  FAIL ci.yml's workflow env did not reach the local jobs"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
else
  echo "  FAIL the env sentinel did not apply, so this case measured nothing"
  fail=$((fail + 1))
fi
cp "$backup" "$ci"

# --- Deleting the env block must REFUSE, not silently drop -D warnings -----------------------
#
# The modification case above proves the value is read from ci.yml. It does not prove that
# LOSING the block is noticed -- and that is the same regression arriving by deletion rather
# than by drift, which is the failure mode the parity table itself is built around.
cp "$backup" "$ci"
python3 - "$ci" <<'PYDEL'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "RUSTFLAGS: -D warnings"
assert old in text, "ci.yml no longer sets RUSTFLAGS at workflow level"
# Remove the whole workflow-level env block by renaming its key, which is exactly how a
# refactor would lose it.
path.write_text(text.replace("\nenv:\n", "\nnot-env:\n", 1))
PYDEL
out="$("$here/ci-local.py" --only fmt 2>&1 || true)"
if grep -q 'no workflow-level' <<<"$out"; then
  echo "  ok   a missing ci.yml env block is refused, not defaulted away"
  pass=$((pass + 1))
else
  echo "  FAIL a missing ci.yml env block did not refuse"
  echo "$out" | tail -8
  fail=$((fail + 1))
fi
cp "$backup" "$ci"

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

# EVERYTHING THE CASES BELOW CREATE, RESTORED ON ANY EXIT. `rust-toolchain.toml` in particular:
# leaving a mutated compiler pin behind would make every later cargo command in this tree build
# with something nobody chose, which is a worse mess than the one this file tests for.
broken=""
tbackup=""
allow_fixture=""
nvmrc_backup=""
implies_fixture=""
# `-s`, NOT `-e`, ON THE TOOLCHAIN BACKUP. `tbackup="$(mktemp)"` creates a ZERO-BYTE file, and
# the trap is armed before the `cp` that fills it -- so in that window `[ -e ]` was true and the
# restore would have TRUNCATED the tracked compiler pin to nothing. Security review caught it;
# the original code at the top of this file deliberately copies before arming, and the new
# cases reversed that order. `-s` makes the window harmless either way.
#
# AND THE SIGNALS EXIT MISSES. Measured on bash 5: an EXIT trap runs on INT and TERM but NOT on
# HUP -- exit 129, body never executed. A sweep in a terminal that goes away would leave
# `rust-toolchain.toml` at `99.99.99`, and every later cargo command in the tree would try to
# resolve a toolchain that does not exist.
restore_all() {
  trap - EXIT INT TERM HUP
  cp "$backup" "$ci"
  rm -f "$backup" "$broken" "$allow_fixture" "$implies_fixture"
  if [ -n "$tbackup" ] && [ -s "$tbackup" ]; then
    cp "$tbackup" "$repo/rust-toolchain.toml"
    rm -f "$tbackup"
  fi
  # `.nvmrc` is the node pin, and a leftover `99` would make `nvm use` resolve a version
  # nobody has. Same `-s` guard as above: `mktemp` creates it empty before the `cp` fills it.
  if [ -n "$nvmrc_backup" ] && [ -s "$nvmrc_backup" ]; then
    cp "$nvmrc_backup" "$repo/.nvmrc"
    rm -f "$nvmrc_backup"
  fi
  if [ -n "$hidden" ] && [ -e "$hidden" ]; then
    mv "$hidden" "${hidden%.hidden-by-test-ci-local}"
  fi
  return 0
}
trap restore_all EXIT INT TERM HUP

# --- The environment preflight: a missing TOOL refuses, rather than failing every job --------
#
# The second of this class, and the reason it is a DERIVED gate rather than another flag.
# `needs_qpdf_cli` above covers one hand-declared tool; this covers every program the selected
# jobs invoke, read out of their own `run` strings.
#
# Measured twice: M0's rustfmt hook failed SILENTLY when `~/.cargo/bin` was off PATH, and in the
# split-page batch the same absence produced a sweep with seven failures inside
# `tools/test-check-no-network-deps.sh`, every one of them reading `cargo tree failed` -- a
# report about the network-dependency checker, of which nothing had been measured.
#
# Same empty-PATH technique as the qpdf case: an interpreter and nothing else, so `which`
# genuinely finds nothing. `--only fmt` because `fmt` needs cargo and needs no qpdf.
echo
empty="$(mktemp -d)"
ln -s "$py" "$empty/python3"
status=0
out="$(PATH="$empty" "$py" "$here/ci-local.py" --only fmt 2>&1)" || status=$?
rm -rf "$empty"
if [ "$status" -eq 1 ] && grep -q "environment not ready" <<<"$out" && grep -q "cargo not found" <<<"$out"; then
  echo "  ok   a job whose cargo is missing is refused by name, not run"
  pass=$((pass + 1))
else
  echo "  FAIL a missing cargo was not refused by name (status $status)"
  echo "$out" | tail -8
  fail=$((fail + 1))
fi

# AND IT MUST NOT REFUSE A JOB THAT NEEDS NONE OF THE MISSING TOOLS. Without this, the case
# above is satisfied by a preflight that refuses everything -- a different bug with the same
# green tick, which would also silently pre-empt the qpdf case above, whose whole setup is an
# empty PATH plus a job that needs no cargo.
# `--preflight`, NOT `--check`. This case ran `--check`, which returns at the parity gate
# BEFORE the preflight -- so it reached nothing, and code review proved it vacuous against a
# mutant whose `preflight()` refused everything unconditionally and still exited 0. The case
# whose own comment warns about a refuse-everything preflight could not see one.
#
# AN EXIT CODE IS NOT AN OUTCOME, so the report line is asserted too: exit 0 is also what a
# preflight that never ran would give.
#
# `fuzz-seed`, NOT `prune-is-reached`: the latter was the fixture until the preflight became
# transitive, at which point it correctly started requiring cargo through
# `tools/test-prune-is-reached.sh` and this case went red. That is the transitivity fix working
# and the fixture going stale, not a regression -- `fuzz-seed` runs `python3` and a script that
# invokes nothing, so it is genuinely satisfied by the stub PATH.
empty="$(mktemp -d)"
ln -s "$py" "$empty/python3"
status=0
out="$(PATH="$empty" "$py" "$here/ci-local.py" --only fuzz-seed --preflight 2>&1)" || status=$?
rm -rf "$empty"
if [ "$status" -eq 0 ] && grep -q "program(s) required by 1 job(s)" <<<"$out"; then
  echo "  ok   the preflight is scoped to the selected jobs, not to the whole table"
  pass=$((pass + 1))
else
  echo "  FAIL the preflight refused, or never ran, for a job needing none of the missing tools (status $status)"
  echo "$out" | tail -8
  fail=$((fail + 1))
fi

# --- AND IT IS TRANSITIVE: a tool a job reaches only THROUGH a script still counts ------------
#
# The gap that shipped in the first version of this gate and was found by security review. Five
# jobs have a `run` string that is nothing but script names, so the derived requirement was
# "that file exists". `checkers` runs `tools/check-no-network-deps.sh`, which calls `cargo tree`
# eight times -- so with cargo off PATH the preflight passed `checkers` and the job then failed
# with `cargo tree failed`, which is the LITERAL string this tool's docstring cites as the
# incident it exists to prevent. The refusal must name the script, not just the tool.
empty="$(mktemp -d)"
ln -s "$py" "$empty/python3"
status=0
out="$(PATH="$empty" "$py" "$here/ci-local.py" --only checkers --preflight 2>&1)" || status=$?
rm -rf "$empty"
if [ "$status" -eq 1 ] && grep -q "cargo not found" <<<"$out" \
   && grep -q "via tools/check-no-network-deps.sh" <<<"$out"; then
  echo "  ok   a tool reached only through a script is required, and names the script"
  pass=$((pass + 1))
else
  echo "  FAIL the preflight is not transitive: checkers passed without cargo (status $status)"
  echo "$out" | tail -8
  fail=$((fail + 1))
fi

# --- A tool at the WRONG VERSION is refused, which is the same failure in disguise -----------
#
# Presence is not enough: a cargo on PATH at the wrong version runs every job and describes
# nothing CI will do. The expected version is READ FROM rust-toolchain.toml, so this plants a
# sentinel there and requires it to come back out -- the same technique as the RUSTFLAGS case,
# and for the same reason: it proves the number is derived rather than written down twice.
toolchain="$repo/rust-toolchain.toml"
tbackup="$(mktemp)"
cp "$toolchain" "$tbackup"
python3 - "$toolchain" <<'PYPIN'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = 'channel = "'
assert old in text, "rust-toolchain.toml no longer pins a channel"
path.write_text(text.replace(old, 'channel = "99.99.99"  # ', 1))
PYPIN
if cmp -s "$toolchain" "$tbackup"; then
  echo "  FAIL the toolchain pin mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$("$here/ci-local.py" --only fmt 2>&1)" || status=$?
  # A BOGUS CHANNEL DOES NOT PRODUCE A MISMATCH, it produces a probe that FAILS -- rustup
  # cannot resolve `99.99.99`, so `cargo --version` exits non-zero. That is its own rule and
  # its own message. Before the probe checked the return code, this surfaced as
  # `cargo is custom, pinned at 99.99.99` -- word one of rustup's error text read as a version
  # -- and this case passed anyway, because it asserted only the pinned half. Code review.
  # The genuine mismatch path is the wasm-pack case below, whose probe still succeeds.
  if [ "$status" -eq 1 ] && grep -q "the version probe did not answer" <<<"$out" \
     && ! grep -q "cargo is custom" <<<"$out"; then
    echo "  ok   a version probe that fails is refused as such, not read as a version"
    pass=$((pass + 1))
  else
    echo "  FAIL a failing version probe was not refused correctly (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
cp "$tbackup" "$toolchain"
rm -f "$tbackup"

# AND AN UNREADABLE PIN IS A REFUSAL, NOT A SKIP. A regex that stops matching because a file
# was reformatted would otherwise leave that tool silently unchecked while the output still
# said OK -- "a check that silently examines nothing is worse than no check".
tbackup="$(mktemp)"
cp "$toolchain" "$tbackup"
python3 - "$toolchain" <<'PYNOPIN'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
assert 'channel = "' in text, "rust-toolchain.toml no longer pins a channel"
path.write_text(text.replace('channel = "', 'chanel = "', 1))
PYNOPIN
if cmp -s "$toolchain" "$tbackup"; then
  echo "  FAIL the unreadable-pin mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$("$here/ci-local.py" --only fmt 2>&1)" || status=$?
  if [ "$status" -eq 1 ] && grep -q "cannot read the pinned version" <<<"$out"; then
    echo "  ok   a pin that cannot be read is refused, not skipped"
    pass=$((pass + 1))
  else
    echo "  FAIL an unreadable pin did not refuse (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
cp "$tbackup" "$toolchain"
rm -f "$tbackup"

# --- A GENUINE version mismatch, on a pin whose probe still answers --------------------------
#
# The case above covers a probe that FAILS. This one covers the comparison itself: mutating the
# pin leaves the probe perfectly able to answer, so the refusal must name BOTH halves -- the
# installed version and the pinned one. Without this, nothing tests the branch the whole block
# exists for.
#
# `node`, NOT `wasm-pack`, and that was a CI failure rather than a preference. This self-test
# runs in the `deny` job, which fetches nothing and installs nothing -- so `wasm-pack` is absent
# there, the presence check reported it instead of the version check, and both of these cases
# went red on a runner while passing locally. "Where a check lives in ci.yml is load-bearing",
# arriving on a fixture. `node` is present on any runner and is pinned by `.nvmrc`.
#
# ASSERTED ON THE MESSAGE, never the exit code: `--only web` also needs `pnpm`, which may be
# absent on that runner too, so the exit status is 1 for either reason and only the text
# distinguishes them.
nvmrc_backup="$(mktemp)"
cp "$repo/.nvmrc" "$nvmrc_backup"
python3 - "$repo/.nvmrc" <<'PYWP'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
# THE SHAPE, NOT MERELY NON-EMPTY. Overwriting wholesale meant the pre-mutation content was
# never checked against what ci-local.py can actually read -- so `.nvmrc` holding `lts/*`
# (legal for nvm and setup-node) would leave CI green, every developer run refusing with
# "cannot read the pinned version", and BOTH of these cases still printing ok. Code review.
# `re.search` WITH `re.M`, matching `_pinned_version`'s own regex. `re.match` anchors at the
# string start with no MULTILINE, so a `.nvmrc` the CHECKER can read happily -- a comment line
# above the number -- aborted this whole suite under `set -e`, and every case after this point
# silently did not run. An assertion about a checker must use the checker's own semantics.
# Security review measured it.
assert re.search(r"^\s*v?[0-9]+", text, re.M), ".nvmrc does not state a major version ci-local.py can read"
path.write_text("99\n")
PYWP
if cmp -s "$repo/.nvmrc" "$nvmrc_backup"; then
  echo "  FAIL the node pin mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$("$here/ci-local.py" --only web --preflight 2>&1)" || status=$?
  if grep -qE "node is [0-9][^,]*, pinned at 99" <<<"$out"; then
    echo "  ok   a tool at the wrong version is refused, naming both halves"
    pass=$((pass + 1))
  else
    echo "  FAIL a genuine version mismatch was not refused (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
cp "$nvmrc_backup" "$repo/.nvmrc"

# --- An allowance suppresses a mismatch, and says why ----------------------------------------
#
# VERSION_ALLOWANCES ships EMPTY, which is the right default and also means nothing exercises
# it. An untested escape hatch is one nobody can trust when they need it, so a copy of the
# checker with one entry must accept the same mutation the case above refuses -- and print the
# reason while doing it.
cp "$repo/.nvmrc" "$nvmrc_backup"
python3 - "$repo/.nvmrc" <<'PYWP2'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
# `re.search` WITH `re.M`, matching `_pinned_version`'s own regex. `re.match` anchors at the
# string start with no MULTILINE, so a `.nvmrc` the CHECKER can read happily -- a comment line
# above the number -- aborted this whole suite under `set -e`, and every case after this point
# silently did not run. An assertion about a checker must use the checker's own semantics.
# Security review measured it.
assert re.search(r"^\s*v?[0-9]+", text, re.M), ".nvmrc does not state a major version ci-local.py can read"
path.write_text("99\n")
PYWP2
allow_fixture="$here/.ci-local-allowance-fixture.py"
python3 - "$here/ci-local.py" "$allow_fixture" <<'PYALLOW'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = "VERSION_ALLOWANCES: dict[str, str] = {}"
assert old in text, "VERSION_ALLOWANCES is not spelled as this test expects"
dst.write_text(text.replace(
    old,
    'VERSION_ALLOWANCES: dict[str, str] = {"node": "planted by tools/test-ci-local.sh"}',
    1,
))
PYALLOW
if cmp -s "$here/ci-local.py" "$allow_fixture"; then
  echo "  FAIL the allowance mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 "$allow_fixture" --only web --preflight 2>&1)" || status=$?
  # THE MISMATCH IS GONE AND THE REASON IS PRINTED -- both halves, on the text. Not the exit
  # code: `--only web` also needs pnpm, which a runner that installs nothing does not have, so
  # exit 1 can mean "the allowance failed" or "pnpm is absent" and only the message tells them
  # apart. That conflation is what took this case red in CI.
  if grep -q "planted by tools/test-ci-local.sh" <<<"$out" \
     && ! grep -qE "node is [0-9][^,]*, pinned at 99" <<<"$out"; then
    echo "  ok   an argued allowance suppresses a mismatch and prints its reason"
    pass=$((pass + 1))
  else
    echo "  FAIL an allowance did not suppress the mismatch (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
rm -f "$allow_fixture"
cp "$nvmrc_backup" "$repo/.nvmrc"
rm -f "$nvmrc_backup"
nvmrc_backup=""

# --- A pin nothing reads is inert, and saying "compared" about it is the worst answer -------
#
# `.nvmrc` and the step that consumes it are different files. `actions/setup-node` PREFERS
# `node-version` over `node-version-file` and only WARNS when both are given -- so putting the
# literal back to test something would leave CI on that version while `.nvmrc` sat inert and
# the preflight went on comparing developer machines against it, reporting
# "2 of 2 pinned version(s) compared". Code review found the gap; this is the regression case.
# THE PIN'S CONSUMER MOVED INTO A COMPOSITE ACTION, and these two cases moved with it.
# `setup-node` now lives in `.github/actions/build-web-payload/action.yml`, shared by CI's
# `web` job and by the deploy so the two cannot drift. `ci.yml` no longer contains the line,
# so mutating `ci.yml` here would assert nothing -- and the guard in `ci-local.py` follows
# local actions for exactly the same reason. Both halves had to move together; either one
# alone is a check looking at a file that does not hold what it is checking.
action="$repo/.github/actions/build-web-payload/action.yml"
action_backup="$(mktemp)"
cp "$action" "$action_backup"

cp "$action_backup" "$action"
python3 - "$action" <<'PYCONSUMER'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "node-version-file: .nvmrc"
assert old in text, "the shared action no longer reads .nvmrc, so this case has nothing to break"
path.write_text(text.replace(old, "node-version: 24", 1))
PYCONSUMER
if cmp -s "$action" "$action_backup"; then
  echo "  FAIL the consumer mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$("$here/ci-local.py" --only web --preflight 2>&1)" || status=$?
  if grep -q "so the pin is inert" <<<"$out"; then
    echo "  ok   a pin ci.yml has stopped reading is refused as inert"
    pass=$((pass + 1))
  else
    echo "  FAIL an inert pin was not noticed (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
cp "$action_backup" "$action"

# --- A version stated INLINE beside the file reference takes precedence, so it is refused ----
#
# The `consumer` case above replaces `node-version-file`, which trips `consumer` and returns
# before `conflict` is ever evaluated -- so that rule had no probe of its own. This is the
# shape that actually happens: somebody adds the literal back to test something and leaves the
# file reference in place. `actions/setup-node` then uses the literal and only WARNS.
cp "$action_backup" "$action"
python3 - "$action" <<'PYCONFLICT'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "        node-version-file: .nvmrc"
assert old in text, "the shared action no longer reads .nvmrc, so this case has nothing to add to"
path.write_text(text.replace(old, old + "\n        node-version: 24", 1))
PYCONFLICT
if cmp -s "$action" "$action_backup"; then
  echo "  FAIL the conflict mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$("$here/ci-local.py" --only web --preflight 2>&1)" || status=$?
  if grep -q "states a version inline" <<<"$out"; then
    echo "  ok   a version stated inline beside the file reference is refused"
    pass=$((pass + 1))
  else
    echo "  FAIL an inline version beside the file reference was not noticed (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
cp "$action_backup" "$action"
rm -f "$action_backup"

# --- The node pin survives the web job no longer spelling `node` -----------------------------
#
# MEASURED FALSE-GREEN, found by security review. `node` reached the requirement set only
# because one command spells it literally. Rewriting it as a pnpm script -- a legitimate
# refactor -- dropped `node` from `required`, shrank the `due` count with it, and reported
# "1 of 1 pinned version(s) compared" and exit 0 on a machine running node 24 against a pin of
# 22. `IMPLIES` closes it with a fact about the tool: pnpm is a node program.
#
# THE MUTATION GOES IN A COPY OF THE CHECKER, not in ci.yml. Removing the step from the
# workflow also makes the local `covers` claim for `tools/report-size-budget.mjs` stale, so
# parity refuses before the preflight ever runs -- the case would have failed for a true but
# unrelated reason. What is under test is the job's run string, so that is what is mutated.
implies_fixture="$here/.ci-local-implies-fixture.py"
python3 - "$here/ci-local.py" "$implies_fixture" <<'PYIMPLIES'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = '"pnpm test && node ../../tools/report-size-budget.mjs dist"'
assert old in text, "the web job's run string is not spelled as this case expects"
dst.write_text(text.replace(old, '"pnpm test && pnpm run size-budget"', 1))
PYIMPLIES
if cmp -s "$here/ci-local.py" "$implies_fixture"; then
  echo "  FAIL the implication mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 "$implies_fixture" --only web --preflight 2>&1)" || status=$?
  # THE DENOMINATOR IS THE ASSERTION, not the numerator. `due` is how many pins this job's
  # requirements call for, so `of 2` says node is still one of them; `of 1` would mean it had
  # dropped out, which is the defect. The numerator counts what was actually COMPARED, and
  # that depends on which tools the runner has -- on the `deny` runner, where this suite
  # runs, pnpm is absent, so it read `1 of 2` and this case failed in CI while passing
  # locally. Third fixture in this file to assume a tool is installed; an assertion here must
  # hold on a runner that installs nothing.
  if grep -qE "of 2 pinned version\(s\) compared" <<<"$out"; then
    echo "  ok   the node pin is still compared when the web job stops spelling node"
    pass=$((pass + 1))
  else
    echo "  FAIL the node pin dropped out when the literal went (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
rm -f "$implies_fixture"

# --- The probe gate itself: break one RULE in a copy and require it to refuse, naming it -----
#
# CLAUDE.md's definition of done: "break a rule in a COPY of the checker and assert it refuses,
# NAMING the reason. Put the copy beside the original -- a copy in a temp directory resolves its
# own paths wrongly and exits non-zero for the wrong reason, which an exit-code-only assertion
# reports as a pass." So the copy goes in tools/, and the assertion reads the message.
#
# The rule broken is the one that was actually wrong first: stripping a leading `VAR=value`
# assignment. With ASSIGNMENT unable to match, `RUSTDOCFLAGS="-D warnings" cargo doc` stops
# yielding `cargo`, so a machine with no cargo would pass the preflight for the `doc` job.
broken="$here/.ci-local-parser-fixture.py"
python3 - "$here/ci-local.py" "$broken" <<'PYBREAK'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = 'ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")'
# THE MUTATION MUST APPLY. A replace that matched nothing leaves a working copy, and a working
# copy passing its own probes reads exactly like the gate holding.
assert old in text, "the assignment rule is not spelled as this test expects"
dst.write_text(text.replace(old, 'ASSIGNMENT = re.compile(r"^(?!)")', 1))
PYBREAK
if cmp -s "$here/ci-local.py" "$broken"; then
  echo "  FAIL the checker copy is identical to the original, so nothing was broken"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 "$broken" --check 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -q "the command parser is broken" <<<"$out"; then
    echo "  ok   a broken parser rule is refused by its own probe, naming the parser"
    pass=$((pass + 1))
  else
    echo "  FAIL a broken parser rule was not caught by the probe table (status $status)"
    echo "$out" | tail -8
    fail=$((fail + 1))
  fi
fi
rm -f "$broken"

# --- `--changed` may only ever NARROW, and each rule that lets it is probed ------------------
#
# A selective sweep that skipped a job it should have run is indistinguishable, from its output,
# from one that ran it and passed. So every rule that permits a skip gets a case, and every case
# names the rule rather than asserting a count.
#
# The scenarios drive `select_changed` directly with a fixed file list: shelling out to git
# would be testing git, and the question here is the derivation.
echo
echo "--changed selects by derived paths, and only ever narrows:"

changed_probe="$here/.ci-local-changed-fixture.py"
cp "$here/ci-local.py" "$changed_probe"

# The driver, as a file rather than a heredoc inside `$( )` -- bash mis-parses the latter and
# warns about an unterminated here-document, which reads like a failure in a test's own output.
scenario_driver="$here/.ci-local-changed-driver.py"
cat > "$scenario_driver" <<'PYEOF'
import importlib.util
import sys

spec = importlib.util.spec_from_file_location("cil", sys.argv[1])
module = importlib.util.module_from_spec(spec)
sys.modules["cil"] = module
spec.loader.exec_module(module)
files = [f for f in sys.argv[2].split(",") if f]
module.changed_paths = lambda base, _f=files: (_f, "a scenario")
jobs, skipped = module.select_changed(list(module.JOBS), None)
print("RUNS:" + ",".join(j["name"] for j in jobs))
print("SKIPS:" + ",".join(n for n, _ in skipped))
PYEOF

scenario() {
  local name="$1" files="$2" must_run="$3" must_skip="$4"
  local out status=0
  out="$(python3 "$scenario_driver" "$changed_probe" "$files" 2>&1)" || status=$?
  if [ "$status" -ne 0 ]; then
    echo "  FAIL $name: the probe itself failed"
    echo "$out" | tail -4
    fail=$((fail + 1))
    return
  fi
  local runs skips ok=1
  runs="$(grep '^RUNS:' <<<"$out" | cut -d: -f2-)"
  skips="$(grep '^SKIPS:' <<<"$out" | cut -d: -f2-)"
  for job in ${must_run//,/ }; do
    grep -q "\b$job\b" <<<"$runs" || { echo "  FAIL $name: $job should have run"; ok=0; }
  done
  for job in ${must_skip//,/ }; do
    grep -q "\b$job\b" <<<"$skips" || { echo "  FAIL $name: $job should have been skipped"; ok=0; }
  done
  if [ "$ok" -eq 1 ]; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "       ran:     $runs"
    echo "       skipped: $skips"
    fail=$((fail + 1))
  fi
}

scenario "a Rust change runs the Rust jobs and skips the web ones" \
  "core/burrow-engines/src/pdfsyntax/geometry.rs" "clippy,test,doc" "web,web-e2e,fuzz"
scenario "a web change skips the Rust jobs" \
  "apps/web/src/pages/index.astro" "web,web-e2e" "clippy,test,doc,wasm"
scenario "a change to a dependency selects its dependents" \
  "core/burrow-types/src/lib.rs" "test,ignored-tests,wasm" "web,web-e2e"
scenario "a fuzz-target change runs the fuzz jobs, which nothing else compiles" \
  "fuzz/fuzz_targets/pdfsyntax_geometry.rs" "fuzz,fuzz-seed" "clippy,test"
scenario "an unattributable file narrows nothing" \
  "tests/conformance/fixtures/blank-1page.pdf" "clippy,test,web,web-e2e,fuzz" ""
scenario "a workflow change narrows nothing" \
  ".github/workflows/ci.yml" "clippy,test,web,fuzz" ""
scenario "a checker always runs, whatever changed" \
  "apps/web/src/pages/index.astro" "checkers,checker-self-tests,integration-suites" "test"

# `paths_as` NARROWS ONLY WHILE THE SCRIPT STILL RUNS WHAT IT CLAIMS (#174). The `doc` job wraps
# `cargo doc --workspace` in `tools/check-rustdoc.sh` and declares so, which is what lets a web
# change skip it -- the case above. A declaration the script does not contain must not narrow:
# with it, the same web change runs `doc`.
paths_as_probe="$here/.ci-local-paths-as-fixture.py"
python3 - "$here/ci-local.py" "$paths_as_probe" <<'PYEOF'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text()
old = '"paths_as": "cargo doc --workspace --no-deps --all-features",'
assert old in text, "the doc job's paths_as is not spelled as this test expects"
# A DECLARATION THAT WOULD NARROW IF TRUSTED: it names `--workspace`, so paths derive from it. A
# first version wrote `cargo doc --no-such-flag`, from which nothing derives, so `doc` ran
# anyway and the case passed with the verification deleted -- measured, and fixed here.
pathlib.Path(sys.argv[2]).write_text(
    text.replace(old, '"paths_as": "cargo doc --workspace --no-deps --no-such-flag",', 1)
)
PYEOF
out="$(python3 "$scenario_driver" "$paths_as_probe" "apps/web/src/pages/index.astro" 2>&1)"
rm -f "$paths_as_probe"
if grep -q '^RUNS:.*\bdoc\b' <<<"$out"; then
  echo "  ok   a paths_as the script does not contain narrows nothing: a web change runs doc"
  pass=$((pass + 1))
else
  echo "  FAIL a paths_as the script does not contain still let a web change skip doc"
  echo "$out" | tail -3
  fail=$((fail + 1))
fi

# A COMMENTED-OUT BUILD LINE IS NOT A BUILD. A review commented the `cargo doc` line out of the
# wrapper and the narrowing held, because the check was a substring match. The copy below points
# `doc` at a copy of the wrapper whose build line is a comment, and a web change must run `doc`.
commented_script="$here/.check-rustdoc.commented.sh"
commented_probe="$here/.ci-local-commented-fixture.py"
python3 - "$here/check-rustdoc.sh" "$commented_script" "$here/ci-local.py" "$commented_probe" <<'PYEOF'
import pathlib, sys
script, script_copy, cil, cil_copy = map(pathlib.Path, sys.argv[1:5])
text = script.read_text()
old = '    (cd "$root" && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features)'
assert old in text, "the wrapper's build line is not spelled as this test expects"
script_copy.write_text(text.replace(old, "    : # used to run: cargo doc --workspace --no-deps --all-features", 1))
runner = cil.read_text()
run = '"run": "tools/check-rustdoc.sh && tools/test-check-rustdoc.sh",'
assert run in runner, "the doc job's run is not spelled as this test expects"
cil_copy.write_text(runner.replace(run, '"run": "tools/.check-rustdoc.commented.sh",', 1))
PYEOF
out="$(python3 "$scenario_driver" "$commented_probe" "apps/web/src/pages/index.astro" 2>&1)"
rm -f "$commented_script" "$commented_probe"
if grep -q '^RUNS:.*\bdoc\b' <<<"$out"; then
  echo "  ok   a wrapper whose build line is commented out narrows nothing"
  pass=$((pass + 1))
else
  echo "  FAIL a wrapper whose build line is commented out still let a web change skip doc"
  echo "$out" | tail -3
  fail=$((fail + 1))
fi

# AND AN EDIT TO THE WRAPPER RUNS IT, as a rule rather than as a side effect of no other job
# claiming `tools/`.
scenario "an edit to the doc gate's own script runs doc" \
  "tools/check-rustdoc.sh" "doc" ""

# AND THE RULE THAT MUST NOT BE BREAKABLE: with the fuzz exclusion removed, a fuzz-target
# change must still run them -- but an UNRELATED change must not. A mutation that dropped the
# `touched_fuzz` condition would put a minute of searching on every push, which is the cost
# this mode exists to remove.
python3 - "$changed_probe" <<'PYEOF'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "if job[\"name\"] in NEVER_ON_THE_CHANGED_PATH and not touched_fuzz:"
assert old in text, "the fuzz exclusion is not spelled as this test expects"
path.write_text(text.replace(old, "if False:", 1))
PYEOF
out="$(python3 "$scenario_driver" "$changed_probe" "apps/web/src/pages/index.astro" 2>&1)"
if grep -q '^RUNS:.*\bfuzz\b' <<<"$out"; then
  echo "  ok   removing the fuzz exclusion puts fuzz back on an unrelated change, as the mutation should"
  pass=$((pass + 1))
else
  echo "  FAIL the fuzz exclusion could be removed without any scenario noticing"
  fail=$((fail + 1))
fi
rm -f "$changed_probe" "$scenario_driver"

# --- #149: a sweep refuses a build artifact from another tree, before its first job --------
#
# Which job reads which artifact is DERIVED from the stamp guards the job's scripts run, so the
# cases are about the derivation as much as the refusal: a job that reads nothing must not be
# refused, a job that rebuilds an artifact must not be refused for the stale one it is about to
# replace, and a guard that was commented out or sits in a heredoc must not count.
status=0
out="$(python3 "$here/ci-local.py" --only fmt --artifacts 2>&1)" || status=$?
if [ "$status" -eq 0 ] && grep -qF "artifacts: 0 stamped artifact(s) read by 1 job(s)" <<<"$out"; then
  echo "  ok   a job that reads no stamped artifact is not refused for one"
  pass=$((pass + 1))
else
  echo "  FAIL fmt was refused, or reported reading an artifact (status $status)"
  echo "$out" | tail -6
  fail=$((fail + 1))
fi

status=0
out="$(python3 "$here/ci-local.py" --only wasm-pack --artifacts 2>&1)" || status=$?
if [ "$status" -eq 0 ] && grep -qF "built in this sweep, so not checked: pkg (by wasm-pack), pkg-render (by wasm-pack)" <<<"$out"; then
  echo "  ok   the job that rebuilds both bindings is not refused for the ones it replaces"
  pass=$((pass + 1))
else
  echo "  FAIL wasm-pack was refused for the bindings it rebuilds (status $status)"
  echo "$out" | tail -6
  fail=$((fail + 1))
fi

# THE STATUS DEPENDS ON THE MACHINE and the text does not: this runs in a job with no artifacts
# at all, where the right answer is a refusal, and on a developer machine where they may be
# current. So the derivation is asserted on the text, and a refusal must be the artifact one.
status=0
out="$(python3 "$here/ci-local.py" --only web --artifacts 2>&1)" || status=$?
if grep -qF "artifacts: 3 stamped artifact(s) read by 1 job(s): engines-wasm, pkg, pkg-render" <<<"$out" \
   && { [ "$status" -eq 0 ] || grep -qF "REFUSED — a build artifact the selected jobs read" <<<"$out"; }; then
  echo "  ok   the web job is derived to read all three artifacts (status $status on this machine)"
  pass=$((pass + 1))
else
  echo "  FAIL the web job's reads were not derived as the three artifacts (status $status)"
  echo "$out" | tail -6
  fail=$((fail + 1))
fi

# A PLANTED STALE STAMP, at the real path, restored afterwards. Planted rather than found: a
# case that depends on the machine happening to have a stale artifact tests nothing on the day
# it does not. The stamp is the record of the build with one input's digest changed -- the same
# difference a checkout of another branch makes, seen from the other side.
(
  stamp="$repo/engines/vendor/wasm/.build-stamp"
  created=""
  [ -d "$repo/engines/vendor" ] || created="$repo/engines/vendor"
  [ -n "$created" ] || [ -d "$repo/engines/vendor/wasm" ] || created="$repo/engines/vendor/wasm"
  saved="$(mktemp)"
  had_stamp=""
  [ -f "$stamp" ] && { cp "$stamp" "$saved"; had_stamp=1; }
  trap '[ -n "$had_stamp" ] && cp "$saved" "$stamp" || rm -f "$stamp"; rm -f "$saved"; [ -n "$created" ] && rm -rf "$created"; true' EXIT
  mkdir -p "$(dirname "$stamp")"
  python3 -B "$here/build-stamp.py" begin engines-wasm | python3 -c '
import json, sys
record = json.load(sys.stdin)
assert "file:engines/pins.toml" in record, "the record no longer names engines/pins.toml"
record["file:engines/pins.toml"] = "0" * 64
print(json.dumps({"format": 1, "artifact": "engines-wasm", "inputs": record}))
' >"$stamp"
  status=0
  out="$(python3 "$here/ci-local.py" --only checkers --artifacts 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -qF "changed since the build (1): engines/pins.toml" <<<"$out" \
     && grep -qF "engines-wasm is read by: checkers (via tools/check-wasm-exports.sh)" <<<"$out"; then
    echo "  ok   a stale engine stamp refuses the sweep, naming the input and the job that reads it"
    exit 0
  fi
  echo "  FAIL a stale engine stamp did not refuse checkers by name (status $status)"
  echo "$out" | tail -8
  exit 1
) && pass=$((pass + 1)) || fail=$((fail + 1))

# AND THE DERIVATION'S OWN PROBES: break the guard parse in a copy beside the original, and the
# copy must refuse by naming the case that stopped matching.
guard_fixture="$here/.ci-local-guard-fixture.py"
python3 - "$here/ci-local.py" "$guard_fixture" <<'PYGUARD'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = 'STAMP_GUARD = re.compile(r"build-stamp\\.py\\"? check'
assert old in text, "STAMP_GUARD is not spelled as this test expects"
dst.write_text(text.replace(old, 'STAMP_GUARD = re.compile(r"build-stamp\\.py check', 1))
PYGUARD
if cmp -s "$here/ci-local.py" "$guard_fixture"; then
  echo "  FAIL the guard-parse mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 -B "$guard_fixture" --check 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -qF "stamp guards in" <<<"$out"; then
    echo "  ok   a guard parse that misses the shell spelling is refused by its own case"
    pass=$((pass + 1))
  else
    echo "  FAIL a broken guard parse was not refused by name (status $status)"
    echo "$out" | tail -4
    fail=$((fail + 1))
  fi
fi
rm -f "$guard_fixture"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
