#!/usr/bin/env bash
# Fail if any TRACKED file looks like generated output.
#
# WHY THIS EXISTS, AND WHY A RULE WAS NOT ENOUGH
#
# Twice in one session generated output reached `main` while the ignore rule that should have
# stopped it existed:
#
#   1. `tools/__pycache__/check-engine-licences.cpython-312.pyc` (PR #29). Written by the
#      interpreter while a tool was being verified, then swept up by `git add -A`. `.gitignore`
#      gained `__pycache__/` afterwards -- which cannot untrack a file already staged.
#   2. 24 Playwright sweep logs under `spikes/wasm-memory-ceiling/results/` (PR #37). The rule
#      for those lived in a **branch-local** `.gitignore` inside the spike directory. Checking
#      out `main` removed that tracked file from the working tree, so the rule was silently
#      absent at exactly the moment it was needed.
#
# Both were found by a human reading a diff. `CLAUDE.md` now carries the habit -- root
# `.gitignore` only, no `git add -A` after running or building anything -- but a habit that has
# already failed twice is not a control. This is the control.
#
# IT CHECKS THE WHOLE TRACKED TREE, NOT THE DIFF.
#
# Deliberately. A diff-based check needs a merge base, behaves differently on a push than on a
# pull request, and passes forever once something has landed. Scanning `git ls-files` is the
# same on every branch, locally and in CI, needs no history, and stays red until the file is
# actually removed. The tree is clean today, so it passes from the first run.
#
# Adding a legitimate exception means editing ALLOWED below with a reason. There are none.
#
# Usage: check-no-generated-files.sh [path ...]
# With no arguments, checks every tracked file. Paths may be given for testing.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

# THREE PARALLEL ARRAYS, not one delimited string.
#
# The first version packed `regex|label` into one entry and split on the first `|`. Every
# regex here contains `|` -- `(^|/)` -- so `${spec%%|*}` truncated all but one of them to `(^`
# and **15 of 16 patterns were inert while the checker reported "16 pattern(s)"**. It caught a
# planted `.pyc` (the one pattern with no alternation) and silently missed everything else.
#
# That is the exact failure CLAUDE.md's coverage rule names, in the checker written to enforce
# CLAUDE.md. So PROBES below are not decoration: every pattern must match its probe, asserted
# on every run. A pattern that stops matching is a failure, not a silent gap.
#
# Anchored loosely on purpose: a pattern that only matched at the repository root would miss
# `apps/web/dist/` and `tools/__pycache__/`, which is where both real incidents happened.
PATTERNS=(
  '\.pyc$'
  '(^|/)__pycache__/'
  '(^|/)results/'
  '(^|/)test-results/'
  '(^|/)playwright-report/'
  '(^|/)node_modules/'
  '(^|/)target/'
  '(^|/)dist/'
  '(^|/)dist-[A-Za-z0-9-]+/'
  '(^|/)pkg/'
  '\.wasm$'
  '\.data$'
  '\.generated\.(pdf|json|txt)$'
  '(^|/)engines/vendor/'
  '(^|/)\.astro/'
  '(^|/)src/generated/'
)
LABELS=(
  'Python bytecode'
  'Python bytecode directory'
  "a tool or test run's output directory"
  'test runner output'
  'Playwright report output'
  'installed npm dependencies'
  'cargo build output'
  'a build output directory'
  'a build output directory'
  'wasm-pack output'
  'a compiled WebAssembly module'
  'an Emscripten data file'
  'generated test data'
  'the fetched engine tree'
  'Astro cache'
  'generated source'
)
# One path each pattern MUST match. Both real incidents appear here by name.
PROBES=(
  'tools/__pycache__/check-engine-licences.cpython-312.pyc'
  'tools/__pycache__/x.py'
  'spikes/wasm-memory-ceiling/results/pdfium-512.txt'
  'apps/web/test-results/run.json'
  'apps/web/playwright-report/index.html'
  'apps/web/node_modules/left-pad/index.js'
  'target/debug/burrow'
  'apps/web/dist/index.html'
  'apps/web/dist-production-check/index.html'
  'bindings/burrow-wasm/pkg/burrow_wasm.js'
  'apps/web/public/engines/pdfium.abc.wasm'
  'spikes/x/engine.data'
  'corpus/files/big.generated.pdf'
  'engines/vendor/wasm/lib/pdfium.js'
  'apps/web/.astro/types.d.ts'
  'apps/web/src/generated/engines.js'
)

# Legitimate exceptions. Each needs a reason, because an unexplained entry here is how a
# control becomes decoration. Empty is the correct state.
ALLOWED=()

if [ "$#" -gt 0 ]; then
  files=("$@")
  source_desc="$# path(s) given on the command line"
else
  mapfile -t files < <(cd "$repo" && git ls-files)
  source_desc="$(printf '%s' "${#files[@]}") tracked file(s) from git ls-files"
fi

if [ "${#files[@]}" -eq 0 ]; then
  echo "check-no-generated-files: nothing to examine; refusing to pass vacuously" >&2
  exit 1
fi

allowed() {
  local candidate="$1" entry
  for entry in ${ALLOWED+"${ALLOWED[@]}"}; do
    [ "$entry" = "$candidate" ] && return 0
  done
  return 1
}

problems=()

# THE THREE ARRAYS MUST STAY ALIGNED. Parallel arrays desynchronise silently: removing one
# pattern without removing its label and probe shifts every later index, so a file gets
# described as the wrong kind of output and probes test the wrong regexes. Measured by
# deleting one pattern -- the self-test went red, but with a confusing message about the
# wrong file. One length check turns that into a clear failure.
if [ "${#PATTERNS[@]}" -ne "${#LABELS[@]}" ] || [ "${#PATTERNS[@]}" -ne "${#PROBES[@]}" ]; then
  echo "check-no-generated-files: PATTERNS/LABELS/PROBES are ${#PATTERNS[@]}/${#LABELS[@]}/${#PROBES[@]}" >&2
  echo "  They must be the same length and in the same order. Add or remove all three." >&2
  exit 1
fi

# LIVENESS: every pattern must match its probe. This is what makes the pattern count below a
# measurement rather than an assertion -- see the note above PATTERNS.
#
# A mutation sweep shows removing this leaves the self-test green, because per-pattern coverage
# there already catches a truncated regex. It is kept anyway, and the distinction is real: the
# self-test runs in CI, this runs on EVERY invocation, including a developer's local run with
# no self-test in sight. It is an early guard, not an independent defence -- do not read it as
# a second layer.
dead=()
for i in "${!PATTERNS[@]}"; do
  if [[ ! "${PROBES[$i]}" =~ ${PATTERNS[$i]} ]]; then
    dead+=("${PATTERNS[$i]}  (probe ${PROBES[$i]} does not match it)")
  fi
done
if [ "${#dead[@]}" -gt 0 ]; then
  echo "check-no-generated-files: ${#dead[@]} pattern(s) match nothing and are inert:" >&2
  for d in "${dead[@]}"; do echo "  - $d" >&2; done
  echo "A pattern that cannot match is a gap that reads as coverage. Fix it before trusting" >&2
  echo "this check." >&2
  exit 1
fi

for i in "${!PATTERNS[@]}"; do
  regex="${PATTERNS[$i]}"
  label="${LABELS[$i]}"
  for f in "${files[@]}"; do
    if [[ "$f" =~ $regex ]]; then
      allowed "$f" && continue
      problems+=("$f -- $label")
    fi
  done
done

# COVERAGE, per CLAUDE.md: say what was examined and against how much, not just a verdict.
echo "check-no-generated-files: examined $source_desc against ${#PATTERNS[@]} pattern(s), all live"
if [ "${#ALLOWED[@]}" -gt 0 ]; then
  echo "  ${#ALLOWED[@]} allowed exception(s) in force"
fi

if [ "${#problems[@]}" -gt 0 ]; then
  echo >&2
  echo "FAILED -- ${#problems[@]} tracked file(s) look like generated output:" >&2
  for p in "${problems[@]}"; do echo "  - $p" >&2; done
  echo >&2
  echo "Generated output does not belong in git. To fix:" >&2
  echo "  git rm -r --cached <path>   # .gitignore cannot untrack what is already tracked" >&2
  echo "and put the ignore rule in the ROOT .gitignore, not a branch-local one -- a rule that" >&2
  echo "lives on one branch is absent the moment you check out another. See CLAUDE.md." >&2
  exit 1
fi

echo "OK -- no tracked file matches a generated-output pattern."
