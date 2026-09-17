#!/usr/bin/env bash
# Adversarial self-test for tools/check-referenced-paths.py.
#
# That checker is one absence rule and one presence rule, which is the shape that passes on an
# empty repository, a mistyped glob, or a pattern that stopped matching. So each is broken in
# turn and required to refuse, NAMING the reason -- an exit-code-only assertion would pass on a
# checker that failed for an unrelated reason.
#
# THE REAL DEFECT IS REPLANTED, not a synthetic one: ADR 0026 renamed
# `check-no-pdfium-on-the-web.sh` and updated `ci.yml` and not `deploy.yml`, and the symptom
# would have been the deploy job dying at its last gate before upload, after merge to main.
#
# Usage: tools/test-check-referenced-paths.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-referenced-paths.py"

work=""
cleanup() {
  trap - EXIT INT TERM HUP
  [ -n "${work:-}" ] && rm -rf "$work"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0

# `name <expected text> <workflow body>` -- run against a COPY of the repository's git index,
# so nothing here can touch the real workflows.
expect_refusal() {
  local name="$1" expect="$2" body="$3"
  printf '%s' "$body" >"$work/.github/workflows/planted.yml"
  git -C "$work" add -A >/dev/null 2>&1
  local out status=0
  out="$(cd "$work" && python3 "$work/tools/check-referenced-paths.py" 2>&1)" || status=$?
  rm -f "$work/.github/workflows/planted.yml"
  git -C "$work" add -A >/dev/null 2>&1
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -3
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# A COPY OF THE TRACKED TREE, with its own git index, because the checker reads
# `git ls-files`. `git worktree` would be lighter and is wrong here: it shares the repository,
# so `git add` inside it would stage against the real index.
work="$(mktemp -d)"
# THE WORKING TREE'S TRACKED FILES, NOT `git archive HEAD`, and the difference is not academic:
# the first version of this suite copied HEAD and every case failed at the baseline, because
# the branch that RENAMES a gate has the new name in its workflows and the old tree does not
# have the file. A self-test that can only pass against already-merged work is a self-test that
# runs after it would have been useful.
#
# `git ls-files` so untracked build output cannot reach the fixture, and `-o` so a script added
# in this very change is present.
(cd "$repo" && git ls-files -z --cached --others --exclude-standard | tar --null -T - -cf -) |
  tar -x -C "$work"
mkdir -p "$work/.github/workflows" "$work/tools"
git -C "$work" init -q 2>/dev/null || true
git -C "$work" add -A >/dev/null 2>&1

# THE BASELINE. Without it every case below could be passing against a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
if (cd "$work" && python3 "$work/tools/check-referenced-paths.py" >/dev/null 2>&1); then
  echo "  ok   the real workflows pass, so a refusal below means something"
  pass=$((pass + 1))
else
  echo "  FAIL the real workflows do not pass, so no case below means anything"
  (cd "$work" && python3 "$work/tools/check-referenced-paths.py" 2>&1) | tail -4 | sed 's/^/        /'
  exit 1
fi

# --- Rule 1: a script that does not exist ----------------------------------------------------
#
# THE MEASURED DEFECT, replanted: a workflow left naming a gate after the gate was renamed.
expect_refusal "a workflow naming a script that does not exist is refused" \
  "which does not exist" \
  'name: planted
on: workflow_dispatch
jobs:
  planted:
    runs-on: ubuntu-latest
    steps:
      - name: A gate that was renamed out from under this workflow
        run: tools/check-no-pdfium-on-the-web.sh
'

# --- Rule 2: a script that exists and cannot be run -------------------------------------------
chmod -x "$work/tools/check-pdfium-is-render-only.sh"
expect_refusal "a workflow running a script that is not executable is refused" \
  "which is not executable" \
  'name: planted
on: workflow_dispatch
jobs:
  planted:
    runs-on: ubuntu-latest
    steps:
      - run: tools/check-pdfium-is-render-only.sh
'
chmod +x "$work/tools/check-pdfium-is-render-only.sh"

# --- Rule 3: prose is not an invocation -------------------------------------------------------
#
# THE NEAR-MISS, and the direction that matters most: a check that fires on a comment is a
# check somebody turns off. `ci.yml` discusses scripts by path constantly, including ones it
# does not run.
printf '%s' 'name: planted
on: workflow_dispatch
jobs:
  planted:
    runs-on: ubuntu-latest
    steps:
      # tools/this-one-does-not-exist.sh is discussed here and deliberately not run.
      - run: echo ok
' >"$work/.github/workflows/planted.yml"
git -C "$work" add -A >/dev/null 2>&1
if (cd "$work" && python3 "$work/tools/check-referenced-paths.py" >/dev/null 2>&1); then
  echo "  ok   a script named only in a COMMENT is not treated as an invocation"
  pass=$((pass + 1))
else
  echo "  FAIL a comment naming a missing script was treated as an invocation"
  fail=$((fail + 1))
fi
rm -f "$work/.github/workflows/planted.yml"
git -C "$work" add -A >/dev/null 2>&1

# --- Rule 4: a tool constructing a repository path that does not exist ------------------------
#
# THE SECOND MEASURED DEFECT, replanted. `tools/check-qpdf-trapped.py` hard-coded
# `apps/web/src/worker/bridge.js` and `bindings/burrow-wasm/src/bridge.rs`; ADR 0026 split both
# in two. The loud half was a crash. The quiet half was behind it: a checker reading ONE of two
# halves of a hand-written audit surface reports a clean run over half of it.
printf '%s' 'import pathlib
REPO = pathlib.Path(__file__).resolve().parent.parent
STALE = REPO / "apps" / "web" / "src" / "worker" / "bridge.js"
' >"$work/tools/planted-tool.py"
git -C "$work" add -A >/dev/null 2>&1
out=""
status=0
out="$(cd "$work" && python3 "$work/tools/check-referenced-paths.py" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "which does not exist" <<<"$out"; then
  echo "  ok   a tool constructing a path that does not exist is refused"
  pass=$((pass + 1))
else
  echo "  FAIL a tool constructing a missing path was not refused (status $status)"
  fail=$((fail + 1))
fi
rm -f "$work/tools/planted-tool.py"
git -C "$work" add -A >/dev/null 2>&1

# --- Rule 5: a DIRECTORY is not a finding -----------------------------------------------------
#
# THE NEAR-MISS, and the direction that decides whether this rule is usable. Plenty of
# constructions name a directory created later or a gitignored build output --
# `engines/vendor/...` is the obvious one -- and refusing on those would make this fail on a
# clean checkout, which is how a check becomes one people learn to skip.
printf '%s' 'import pathlib
REPO = pathlib.Path(__file__).resolve().parent.parent
BUILT = REPO / "engines" / "vendor" / "no-such-arch" / "lib"
' >"$work/tools/planted-tool.py"
git -C "$work" add -A >/dev/null 2>&1
if (cd "$work" && python3 "$work/tools/check-referenced-paths.py" >/dev/null 2>&1); then
  echo "  ok   a constructed DIRECTORY that does not exist is not treated as a finding"
  pass=$((pass + 1))
else
  echo "  FAIL a constructed directory was refused; this rule would fail on a clean checkout"
  fail=$((fail + 1))
fi
rm -f "$work/tools/planted-tool.py"
git -C "$work" add -A >/dev/null 2>&1

# --- THE PROBE GATES -------------------------------------------------------------------------
#
# Every case above shows the checker refusing a planted defect. None shows WHICH rule refused.
# These break one rule at a time in a copy, replant that rule's defect, and require the gutted
# copy to PASS -- which is only true if that rule was the thing doing the work.
probe_gate() {
  local name="$1" mutation="$2" body="$3"
  sed "$mutation" "$checker" >"$work/tools/.probe.py"
  if cmp -s "$work/tools/.probe.py" "$checker"; then
    echo "  FAIL probe-gate/$name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  printf '%s' "$body" >"$work/.github/workflows/planted.yml"
  git -C "$work" add -A >/dev/null 2>&1
  if (cd "$work" && python3 "$work/tools/check-referenced-paths.py" >/dev/null 2>&1); then
    echo "  FAIL probe-gate/$name: the unmutated checker did not refuse, so nothing was planted"
    fail=$((fail + 1))
  elif (cd "$work" && python3 "$work/tools/.probe.py" >/dev/null 2>&1); then
    echo "  ok   probe-gate/$name: that rule alone is what catches it"
    pass=$((pass + 1))
  else
    echo "  FAIL probe-gate/$name: the defect is still caught with the rule removed"
    fail=$((fail + 1))
  fi
  rm -f "$work/.github/workflows/planted.yml" "$work/tools/.probe.py"
  git -C "$work" add -A >/dev/null 2>&1
}

# THE PLANT IS A MISSING `.py`, NOT A MISSING `.sh`, and the first attempt at this case is why
# that matters. It planted the real defect -- a missing `check-no-pdfium-on-the-web.sh` -- and
# the gutted copy still refused: with the existence rule removed, control falls through to the
# executable rule, `os.access` on a file that is not there is false, and the `.sh` suffix makes
# it fire. Two rules catching one defect is defence in depth and a useless probe, because it
# measures whichever rule runs first. The executable rule is `.sh`-only by construction, so a
# `.py` isolates the existence rule.
probe_gate "the existence rule" \
  's/if not target.is_file():/if False:/' \
  'name: planted
on: workflow_dispatch
jobs:
  planted:
    runs-on: ubuntu-latest
    steps:
      - run: python3 tools/check-no-such-checker.py
'

printf '#!/usr/bin/env bash\nexit 0\n' >"$work/tools/not-executable-fixture.sh"
chmod -x "$work/tools/not-executable-fixture.sh"
probe_gate "the executable rule" \
  's/elif not os.access(target, os.X_OK)/elif False/' \
  'name: planted
on: workflow_dispatch
jobs:
  planted:
    runs-on: ubuntu-latest
    steps:
      - run: tools/not-executable-fixture.sh
'

# THE PLANT IS A `.py`, so the workflow rules cannot reach it: they scan `.github/` only, and
# this file is under `tools/`. Nothing else can be what refuses.
probe_gate_tool() {
  local name="$1" mutation="$2"
  sed "$mutation" "$checker" >"$work/tools/.probe.py"
  if cmp -s "$work/tools/.probe.py" "$checker"; then
    echo "  FAIL probe-gate/$name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  printf '%s' 'import pathlib
REPO = pathlib.Path(__file__).resolve().parent.parent
STALE = REPO / "apps" / "web" / "src" / "worker" / "bridge.js"
' >"$work/tools/planted-tool.py"
  git -C "$work" add -A >/dev/null 2>&1
  if (cd "$work" && python3 "$work/tools/check-referenced-paths.py" >/dev/null 2>&1); then
    echo "  FAIL probe-gate/$name: the unmutated checker did not refuse, so nothing was planted"
    fail=$((fail + 1))
  elif (cd "$work" && python3 "$work/tools/.probe.py" >/dev/null 2>&1); then
    echo "  ok   probe-gate/$name: that rule alone is what catches it"
    pass=$((pass + 1))
  else
    echo "  FAIL probe-gate/$name: the defect is still caught with the rule removed"
    fail=$((fail + 1))
  fi
  rm -f "$work/tools/planted-tool.py" "$work/tools/.probe.py"
  git -C "$work" add -A >/dev/null 2>&1
}

probe_gate_tool "the constructed-path rule" 's/if not target.exists():/if False:/'

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
