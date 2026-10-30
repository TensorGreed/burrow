#!/usr/bin/env bash
# Self-test for tools/check-ci-ok.py: per-rule probes for all three modes, and asserted mutations of
# the real ci.yml in a copy beside it.
#
#   needs     a complete gate passes with its counts; each structural rule refuses its own plant,
#             by name -- including every way a gated job's `if:` can drift from the rule that
#             judges its skip.
#   scope     run in throwaway git repositories: every FUZZ_PREFIXES / FUZZ_NAMES rule says
#             fuzz=true on a diff touching only it, and its near-miss says fuzz=false; every way
#             the diff can fail to be computed says fuzz=true; push and dispatch say fuzz=true.
#   results   every non-success result fails, named; a skipped-but-required job is refused by name
#             (owner (82)(5): fuzz skipped while core/ changed, web-chromium skipped, firefox on
#             push); a meant skip passes (fuzz on a docs-only diff, firefox/webkit on a PR or merge
#             group); and the diff-could-not-be-computed case, end to end through `scope`, requires
#             fuzz to have run.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
# THE CHECKER UNDER TEST. Overridden only by this script's own mutant section below, which points a
# nested run at a planted copy beside the original and requires that run to fail.
check="${BURROW_CI_OK_CHECK:-$here/check-ci-ok.py}"
real="$repo/.github/workflows/ci.yml"
copy="$here/.ci-ok-probe.yml"
work="$(mktemp -d)"
trap 'rm -rf "$work"; rm -f "$copy"' EXIT
pass=0
fail=0
expected=94
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# expect <label> <want status> <want text> -- <command...>: runs it, requires both.
expect() {
  local label="$1" want="$2" text="$3"
  shift 4
  local status=0 out
  out="$("$@" 2>&1)" || status=$?
  if [ "$status" -eq "$want" ] && grep -qF -- "$text" <<<"$out"; then ok "$label"
  else bad "$label (status $status, wanted $want and '$text'): $(tail -n 3 <<<"$out")"; fi
}

# ---- needs ----------------------------------------------------------------------------------

FUZZ_IF="\${{ !cancelled() && needs.changes.outputs.fuzz != 'false' }}"
WEB_IF="\${{ github.event_name != 'pull_request' && github.event_name != 'merge_group' }}"

# The probe workflow: a plain job, `changes`, the three gated jobs with their recorded `if:`, and
# a gate. Each case writes it and then applies one python `old||new` replacement to it.
probe() {
  cat > "$work/w.yml" <<YML
jobs:
  build: {runs-on: x, steps: [{run: "true"}]}
  changes: {runs-on: x, steps: [{run: "true"}]}
  fuzz:
    runs-on: x
    needs: [changes]
    if: $FUZZ_IF
    steps: [{run: "true"}]
  web-firefox:
    runs-on: x
    if: $WEB_IF
    steps: [{run: "true"}]
  web-webkit:
    runs-on: x
    if: $WEB_IF
    steps: [{run: "true"}]
  ci-ok:
    runs-on: x
    if: always()
    needs: [build, changes, fuzz, web-firefox, web-webkit]
    steps:
      - run: python3 tools/check-ci-ok.py results
        env: {NEEDS: x, EVENT: "\${{ github.event_name }}"}
YML
  if [ -n "${1:-}" ]; then
    python3 - "$work/w.yml" "$1" <<'PY' || return 1
import pathlib, sys
path, mutation = pathlib.Path(sys.argv[1]), sys.argv[2]
old, new = mutation.split("||", 1)
text = path.read_text()
assert old in text, f"MUTATION DID NOT APPLY: {old!r}"
path.write_text(text.replace(old, new, 1))
PY
  fi
}

# needs_case <label> <mutation or ""> <want status> <want text>
needs_case() {
  if ! probe "$2"; then bad "$1: the probe mutation did not apply"; return; fi
  expect "$1" "$3" "$4" -- python3 "$check" needs "$work/w.yml"
}

needs_case "a complete gate passes, counting jobs and gated jobs" "" 0 "5 of 5 job(s)"
needs_case "  ... and reports the gated jobs it examined" "" 0 "3 of 3 gated job(s) carry their recorded"
needs_case "a job missing from needs is refused, named" \
  "needs: [build, changes, fuzz, web-firefox, web-webkit]||needs: [build, changes, fuzz, web-firefox]" 1 "does not need web-webkit"
needs_case "a gate not under always() is refused" "    if: always()||    if: success()" 1 "if: always()"
needs_case "a gate with no results step is refused" "check-ci-ok.py results||true" 1 "no step running"
needs_case "a gate needing an absent job is refused, named" \
  "needs: [build, changes,||needs: [build, nope, changes," 1 "does not have: nope"
needs_case "a results step without EVENT is refused" 'EVENT: "${{ github.event_name }}"||OTHER: "x"' 1 "does not set EVENT"
needs_case "an ungated job carrying an unknown if: is refused, named" \
  '  build: {runs-on: x, steps||  build: {runs-on: x, if: "github.ref == 1", steps' 1 '`build` carries `if: github.ref == 1`, which no skip rule'
needs_case "fuzz's if: without !cancelled() is refused" "if: \${{ !cancelled() && ||if: \${{ " 1 '`fuzz` carries'
needs_case "fuzz's if: on a different output value is refused" "fuzz != 'false' }}||fuzz == 'true' }}" 1 '`fuzz` carries'
needs_case "web-firefox's if: removed is refused, named" \
  "  web-firefox:
    runs-on: x
    if: $WEB_IF
||  web-firefox:
    runs-on: x
" 1 '`web-firefox` has no `if:`'
needs_case "web-webkit's if: narrowed to pull_request only is refused" \
  "  web-webkit:
    runs-on: x
    if: $WEB_IF||  web-webkit:
    runs-on: x
    if: \${{ github.event_name != 'pull_request' }}" 1 '`web-webkit` carries'
needs_case "fuzz not needing changes is refused" "    needs: [changes]
||" 1 'does not need `changes`'
needs_case "a workflow with no changes job is refused" \
  '  changes: {runs-on: x, steps: [{run: "true"}]}
||' 1 'no `changes` job'
needs_case "a GATED job the workflow lacks is refused, named" \
  "  web-webkit:
    runs-on: x
    if: $WEB_IF
    steps: [{run: \"true\"}]
||" 1 'GATED names `web-webkit`'

# ---- scope ----------------------------------------------------------------------------------

# A repository whose first commit holds one README; each probe is a commit touching ONE path.
scope_repo="$work/scope"
mkdir -p "$scope_repo"
git -C "$scope_repo" init -q
git -C "$scope_repo" -c user.name=t -c user.email=t@t commit -q --allow-empty -m root
echo base > "$scope_repo/README"
git -C "$scope_repo" add README
git -C "$scope_repo" -c user.name=t -c user.email=t@t commit -q -m base
base_sha="$(git -C "$scope_repo" rev-parse HEAD)"

# commit_touching <path>: a commit on top of base touching only <path>; prints its SHA.
commit_touching() {
  git -C "$scope_repo" checkout -q --detach "$base_sha"
  mkdir -p "$scope_repo/$(dirname "$1")"
  echo "$1" > "$scope_repo/$1"
  git -C "$scope_repo" add -- "$1"
  git -C "$scope_repo" -c user.name=t -c user.email=t@t commit -q -m "touch $1"
  git -C "$scope_repo" rev-parse HEAD
}

# commit_touching_many <path>...: a commit on top of base touching exactly these paths; prints its SHA.
commit_touching_many() {
  git -C "$scope_repo" checkout -q --detach "$base_sha"
  local f
  for f in "$@"; do
    mkdir -p "$scope_repo/$(dirname "$f")"
    echo "$f" > "$scope_repo/$f"
    git -C "$scope_repo" add -- "$f"
  done
  git -C "$scope_repo" -c user.name=t -c user.email=t@t commit -q -m "touch $*"
  git -C "$scope_repo" rev-parse HEAD
}

# scope_says <label> <event> <base> <head> <want fuzz> [<dir>]
scope_says() {
  local dir="${6:-$scope_repo}" status=0 out
  : > "$work/output"
  out="$(cd "$dir" && EVENT="$2" BASE="$3" HEAD="$4" GITHUB_OUTPUT="$work/output" python3 "$check" scope 2>&1)" || status=$?
  if [ "$status" -eq 0 ] && [ "$(cat "$work/output")" = "fuzz=$5" ]; then ok "$1"
  else bad "$1 (status $status, output '$(cat "$work/output")', wanted fuzz=$5): $(tail -n 2 <<<"$out")"; fi
}

# EVERY RULE, READ FROM THE CHECKER, so a rule added there without a probe here fails the count.
rules="$(python3 - "$check" <<'PY'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("ci_ok", sys.argv[1])
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
for prefix in module.FUZZ_PREFIXES: print(f"prefix {prefix}")
for name in module.FUZZ_NAMES: print(f"name {name}")
PY
)"
probed=0
while read -r kind rule; do
  [ -n "$kind" ] || continue
  if [ "$kind" = prefix ]; then
    positive="${rule}probe.txt"
    near="docs/${rule}probe.txt"           # the same directory, not at the root
  else
    positive="deep/dir/$rule"
    near="deep/dir/$rule.orig"              # the name with a suffix
  fi
  sha="$(commit_touching "$positive")"
  scope_says "rule $kind '$rule': a PR touching only $positive runs fuzz" pull_request "$base_sha" "$sha" true
  sha="$(commit_touching "$near")"
  scope_says "rule $kind '$rule': its near-miss $near does not" pull_request "$base_sha" "$sha" false
  probed=$((probed + 1))
done <<<"$rules"
if [ "$probed" -eq 11 ]; then ok "all 11 of the checker's 11 path rules were probed (owner (83)(2)'s list plus (86)(2)'s tests/ and .cargo/)"
else bad "probed $probed path rules; owners (83)(2) and (86)(2) name 11 -- the checker's table and the owners' list disagree"; fi

# (86)(2): the two added prefixes, named, with near-misses that prove the match is anchored at the
# repository root (the generic loop above only proves `docs/<prefix>`).
sha="$(commit_touching tests/conformance/x.pdf)"
scope_says "tests/conformance/x.pdf (a fixture) runs fuzz" pull_request "$base_sha" "$sha" true
sha="$(commit_touching .cargo/config.toml)"
scope_says ".cargo/config.toml runs fuzz" pull_request "$base_sha" "$sha" true
sha="$(commit_touching docs/tests/x.md)"
scope_says "near-miss docs/tests/x.md does not run fuzz (tests/ is anchored at the root)" pull_request "$base_sha" "$sha" false
sha="$(commit_touching apps/web/.cargo-notes)"
scope_says "near-miss apps/web/.cargo-notes does not run fuzz" pull_request "$base_sha" "$sha" false

# B1 (round 1): git QUOTES a path holding a non-ASCII byte, a tab or a quote unless asked not to
# (`"core/\303\274.rs"`), and a quoted path does not start with `core/`. Each is under core/, so each
# must run fuzz. (Without -z, a tab or a quote is quoted whatever core.quotepath says.)
sha="$(commit_touching "core/ü-probe.rs")"
scope_says "a non-ASCII path under core/ runs fuzz (git would quote it)" pull_request "$base_sha" "$sha" true
sha="$(commit_touching "$(printf 'core/tab\there.rs')")"
scope_says "a path with a tab under core/ runs fuzz (git would quote it)" pull_request "$base_sha" "$sha" true
sha="$(commit_touching 'core/quote"here.rs')"
scope_says "a path with a quote under core/ runs fuzz (git would quote it)" pull_request "$base_sha" "$sha" true

# S2 (round 1): the decision must read EVERY changed path, not the first or the last. git lists
# paths sorted, so `docs/` follows `core/` and `a/` precedes it.
sha="$(commit_touching_many docs/x.md core/x.rs)"
scope_says "docs and core/ together, the fuzz path listed first, runs fuzz" pull_request "$base_sha" "$sha" true
sha="$(commit_touching_many a/x.md core/x.rs)"
scope_says "docs-like and core/ together, the fuzz path listed last, runs fuzz" pull_request "$base_sha" "$sha" true
sha="$(commit_touching_many a/x.md core/x.rs z/x.md)"
scope_says "a fuzz path in the middle of three runs fuzz" pull_request "$base_sha" "$sha" true

docs_sha="$(commit_touching docs/only.md)"
core_sha="$(commit_touching core/burrow-ops/src/lib.rs)"
scope_says "a merge group on a docs-only diff does not run fuzz (BASE..HEAD)" merge_group "$base_sha" "$docs_sha" false
scope_says "a merge group touching core/ runs fuzz" merge_group "$base_sha" "$core_sha" true
scope_says "push always runs fuzz, even on a docs-only diff" push "$base_sha" "$docs_sha" true
scope_says "workflow_dispatch always runs fuzz" workflow_dispatch "" "" true
scope_says "an unset event runs fuzz" "" "$base_sha" "$docs_sha" true
# THE DIFF CANNOT BE COMPUTED: every way, fuzz runs.
scope_says "cannot compute: an empty BASE runs fuzz" pull_request "" "$docs_sha" true
scope_says "cannot compute: an empty HEAD runs fuzz" merge_group "$base_sha" "" true
scope_says "cannot compute: a malformed SHA runs fuzz" pull_request "main" "$docs_sha" true
scope_says "cannot compute: a SHA the clone lacks (fetch failed) runs fuzz" pull_request \
  "0123456789abcdef0123456789abcdef01234567" "$docs_sha" true
scope_says "cannot compute: an empty diff runs fuzz" pull_request "$docs_sha" "$docs_sha" true
mkdir -p "$work/not-a-repo"
scope_says "cannot compute: no git repository runs fuzz" pull_request "$base_sha" "$docs_sha" true "$work/not-a-repo"
expect "without GITHUB_OUTPUT the answer goes to stdout" 0 "fuzz=false" -- \
  env -u GITHUB_OUTPUT -C "$scope_repo" EVENT=pull_request BASE="$base_sha" HEAD="$docs_sha" python3 "$check" scope

# ---- results --------------------------------------------------------------------------------

# results_case <label> <event> <json> <want status> <want text>
results_case() {
  expect "$1" "$4" "$5" -- env NEEDS="$3" EVENT="$2" python3 "$check" results
}
S='{"result":"success"}'
K='{"result":"skipped"}'
CH_FALSE='{"result":"success","outputs":{"fuzz":"false"}}'
CH_TRUE='{"result":"success","outputs":{"fuzz":"true"}}'

for result in failure cancelled skipped; do
  results_case "a $result ungated job fails the gate, named" pull_request \
    "{\"build\":$S,\"test\":{\"result\":\"$result\"}}" 1 "test ($result"
done
results_case "every job succeeding passes the gate" push "{\"build\":$S,\"test\":$S}" 0 "2 of 2 job(s) ran and succeeded"
results_case "a gate that saw no jobs fails" push '{}' 1 "names no jobs"
results_case "an empty \$NEEDS fails, named" push '' 1 "NEEDS is empty"
results_case "an empty \$EVENT fails, named" "" "{\"build\":$S}" 1 "EVENT is empty"
results_case "a garbled \$NEEDS fails, named" push '{"build":' 1 "is not JSON"
results_case "a non-object \$NEEDS fails, named" push '["build"]' 1 "not a JSON object of jobs (it is a list)"
results_case "a job entry that is not a record fails, named" push "{\"build\":$S,\"test\":\"success\"}" 1 "test (no result: not a job record)"
for result in failure cancelled; do
  results_case "a gated job that ran and hit $result fails, named" pull_request \
    "{\"changes\":$CH_FALSE,\"fuzz\":{\"result\":\"$result\"}}" 1 "fuzz ($result)"
done

# THE MEANT SKIPS PASS.
results_case "fuzz skipped on a docs-only PR passes" pull_request \
  "{\"build\":$S,\"changes\":$CH_FALSE,\"fuzz\":$K}" 0 "1 skipped by rule (fuzz)"
results_case "fuzz skipped on a docs-only merge group passes" merge_group \
  "{\"build\":$S,\"changes\":$CH_FALSE,\"fuzz\":$K}" 0 "1 skipped by rule (fuzz)"
results_case "firefox and webkit skipped on a PR pass" pull_request \
  "{\"web-chromium\":$S,\"web-firefox\":$K,\"web-webkit\":$K}" 0 "2 skipped by rule (web-firefox, web-webkit)"
results_case "firefox and webkit skipped in a merge group pass" merge_group \
  "{\"web-chromium\":$S,\"web-firefox\":$K,\"web-webkit\":$K}" 0 "2 skipped by rule (web-firefox, web-webkit)"
results_case "fuzz that ran anyway on a docs-only PR passes" pull_request \
  "{\"changes\":$CH_FALSE,\"fuzz\":$S}" 0 "2 of 2 job(s) ran and succeeded"

# THE SKIPPED-BUT-REQUIRED JOBS ARE REFUSED, BY NAME (owner (82)(5)).
results_case "fuzz skipped while core/ changed (changes said true) is refused, named" pull_request \
  "{\"changes\":$CH_TRUE,\"fuzz\":$K}" 1 "fuzz (skipped, but should have run: \`changes\` said fuzz='true'"
results_case "fuzz skipped when changes failed is refused, named" pull_request \
  "{\"changes\":{\"result\":\"failure\",\"outputs\":{}},\"fuzz\":$K}" 1 "fuzz (skipped, but should have run: \`changes\` did not succeed (failure)"
results_case "fuzz skipped when changes was itself skipped is refused" merge_group \
  "{\"changes\":$K,\"fuzz\":$K}" 1 "fuzz (skipped, but should have run"
results_case "fuzz skipped with no changes output is refused" pull_request \
  "{\"changes\":$S,\"fuzz\":$K}" 1 "fuzz (skipped, but should have run: \`changes\` said fuzz=None"
results_case "fuzz skipped with changes absent from needs is refused" pull_request \
  "{\"build\":$S,\"fuzz\":$K}" 1 "\`changes\` did not succeed (absent)"
results_case "fuzz skipped on push is refused, named" push \
  "{\"changes\":$CH_FALSE,\"fuzz\":$K}" 1 "fuzz (skipped, but should have run: event \`push\` runs every job"
results_case "web-chromium skipped on a PR is refused, named" pull_request \
  "{\"web-chromium\":$K,\"web-firefox\":$K,\"web-webkit\":$K}" 1 "web-chromium (skipped, but should have run: it is not a gated job"
results_case "web-firefox skipped on push is refused, named" push \
  "{\"web-chromium\":$S,\"web-firefox\":$K,\"web-webkit\":$S}" 1 "web-firefox (skipped, but should have run: event \`push\`"
results_case "web-webkit skipped on workflow_dispatch is refused, named" workflow_dispatch \
  "{\"web-chromium\":$S,\"web-firefox\":$S,\"web-webkit\":$K}" 1 "web-webkit (skipped, but should have run"
results_case "native-record skipped on a PR is refused, named" pull_request \
  "{\"native-record\":$K,\"web-chromium\":$S}" 1 "native-record (skipped, but should have run"

# THE DIFF COULD NOT BE COMPUTED, END TO END: `scope` answers, and `results` judges fuzz's skip by
# that answer. A skip after an uncomputable diff is refused.
: > "$work/output"
(cd "$scope_repo" && EVENT=pull_request BASE="" HEAD="$docs_sha" GITHUB_OUTPUT="$work/output" python3 "$check" scope >/dev/null 2>&1)
said="$(sed -n 's/^fuzz=//p' "$work/output")"
if [ "$said" = true ]; then
  results_case "an uncomputable diff, then fuzz skipped, is refused (fuzz had to run)" pull_request \
    "{\"changes\":{\"result\":\"success\",\"outputs\":{\"fuzz\":\"$said\"}},\"fuzz\":$K}" 1 "fuzz (skipped, but should have run"
  results_case "  ... and fuzz having run after it passes" pull_request \
    "{\"changes\":{\"result\":\"success\",\"outputs\":{\"fuzz\":\"$said\"}},\"fuzz\":$S}" 0 "2 of 2 job(s) ran and succeeded"
else
  bad "scope said fuzz='$said' on an uncomputable diff, so the end-to-end cases measured nothing"
  bad "  (second end-to-end case not run)"
fi

# ---- the real ci.yml ------------------------------------------------------------------------

expect "the real ci.yml passes" 0 "3 of 3 gated job(s) carry their recorded" -- python3 "$check" needs "$real"

# mutate_real <label> <python: old> <python: new> <want text>: a COPY beside the real file with one
# replacement, asserted applied, must go red naming the reason.
mutate_real() {
  rm -f "$copy"
  if ! python3 - "$real" "$copy" "$2" "$3" <<'PY'
import pathlib, sys
src, dst, old, new = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3], sys.argv[4]
text = src.read_text()
assert text.count(old) == 1, f"MUTATION DID NOT APPLY: {old!r} occurs {text.count(old)} times"
dst.write_text(text.replace(old, new, 1))
PY
  then bad "$1: the mutation did not apply"; return; fi
  if cmp -s "$real" "$copy"; then bad "$1: the copy is identical, so the case measured nothing"; return; fi
  expect "$1" 1 "$4" -- python3 "$check" needs "$copy"
}
mutate_real "the real ci.yml with audit dropped from ci-ok's needs goes red, naming audit" \
  " deny, audit, web-chromium," " deny, web-chromium," "does not need audit"
mutate_real "the real ci.yml with fuzz's if: loosened goes red, naming fuzz" \
  "    if: \${{ !cancelled() && needs.changes.outputs.fuzz != 'false' }}" \
  "    if: \${{ needs.changes.outputs.fuzz != 'false' }}" '`fuzz` carries'
mutate_real "the real ci.yml with web-chromium gated to push goes red, naming it" \
  "  web-chromium:
    name: web (chromium)
" "  web-chromium:
    name: web (chromium)
    if: \${{ github.event_name == 'push' }}
" '`web-chromium` carries'
mutate_real "the real ci.yml with ci-ok's EVENT removed goes red" \
  "NEEDS: \${{ toJSON(needs) }}
          EVENT: \${{ github.event_name }}
" "NEEDS: \${{ toJSON(needs) }}
" "does not set EVENT"
# ---- the probes are shown to bite: planted copies of the CHECKER ---------------------------
#
# Each mutant is a copy of check-ci-ok.py BESIDE it (so it resolves its paths as the original
# does), with one rule broken, asserted applied. This script is re-run against it and must FAIL,
# naming the case the broken rule should trip. Skipped inside such a nested run.
if [ -z "${BURROW_CI_OK_MUTANT:-}" ]; then
  mutant="$here/.check-ci-ok.mutant.py"
  trap 'rm -rf "$work"; rm -f "$copy" "$mutant"' EXIT
  mutants=0
  # plant <label> <old> <new> [<old> <new>]... <the FAIL line the nested run must print>
  plant() {
    mutants=$((mutants + 1))
    local label="$1" want="${*: -1}"
    shift
    set -- "${@:1:$#-1}"
    rm -f "$mutant"
    if ! python3 - "$here/check-ci-ok.py" "$mutant" "$@" <<'PY'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
pairs = sys.argv[3:]
text = src.read_text()
for old, new in zip(pairs[::2], pairs[1::2]):
    assert text.count(old) == 1, f"MUTATION DID NOT APPLY: {old!r} occurs {text.count(old)} times"
    text = text.replace(old, new, 1)
dst.write_text(text)
PY
    then bad "mutant '$label': the mutation did not apply"; return; fi
    if cmp -s "$here/check-ci-ok.py" "$mutant"; then bad "mutant '$label': identical copy"; return; fi
    local status=0 out
    out="$(BURROW_CI_OK_MUTANT=1 BURROW_CI_OK_CHECK="$mutant" "$here/test-check-ci-ok.sh" 2>&1)" || status=$?
    if [ "$status" -ne 0 ] && grep -qF -- "FAIL $want" <<<"$out"; then ok "mutant '$label' is caught: FAIL $want"
    else bad "mutant '$label' survived (status $status; no 'FAIL $want')"; fi
  }
  plant "fuzz's skip ignores what changes said" '        if said != "false":' '        if False:' \
    "fuzz skipped while core/ changed (changes said true) is refused, named"
  plant "an empty diff skips fuzz" 'return True, f"git diff {spec} named no files' 'return False, f"git diff {spec} named no files' \
    "cannot compute: an empty diff runs fuzz"
  plant "a git error skips fuzz" 'return True, f"git diff {spec} failed' 'return False, f"git diff {spec} failed' \
    "cannot compute: a SHA the clone lacks (fetch failed) runs fuzz"
  plant "tools/ dropped from the fuzz paths" '".github/workflows/", "tools/",' '".github/workflows/",' \
    "probed 10 path rules"
  plant "tests/ dropped from the fuzz paths (owner (86)(2))" '"engines/", "tests/", ".cargo/",' '"engines/", ".cargo/",' \
    "tests/conformance/x.pdf (a fixture) runs fuzz"
  plant "push counted as a narrowed event" 'NARROWED_EVENTS = ("pull_request", "merge_group")' \
    'NARROWED_EVENTS = ("pull_request", "merge_group", "push")' \
    "web-firefox skipped on push is refused, named"
  plant "a gated job's if: no longer compared" '        elif str(expression).strip() != GATED[name]:' '        elif False:' \
    "fuzz's if: without !cancelled() is refused"
  plant "the old quoting behaviour (no -z, default quotepath, split on newline)" \
    '["git", "-c", "core.quotepath=false", "diff", "--name-only", "--no-renames", "-z", spec],' \
    '["git", "diff", "--name-only", "--no-renames", spec],' \
    'done.stdout.split(b"\0")' 'done.stdout.split(b"\n")' \
    "a non-ASCII path under core/ runs fuzz (git would quote it)"
  plant "git diff without -z (a tab or a quote is quoted again)" '"--no-renames", "-z", spec]' '"--no-renames", spec]' \
    "a path with a tab under core/ runs fuzz (git would quote it)"
  plant "only the first changed path examined" 'hits = [p for p in paths if touches_fuzz(p)]' 'hits = [p for p in paths[:1] if touches_fuzz(p)]' \
    "docs-like and core/ together, the fuzz path listed last, runs fuzz"
  plant "only the last changed path examined" 'hits = [p for p in paths if touches_fuzz(p)]' 'hits = [p for p in paths[-1:] if touches_fuzz(p)]' \
    "docs and core/ together, the fuzz path listed first, runs fuzz"
  if [ "$mutants" -eq 11 ]; then ok "11 of 11 checker mutants were planted"; else bad "planted $mutants of 11 mutants"; fi
  expected=$((expected + mutants + 1))
fi

echo
if [ "$fail" -ne 0 ] || [ "$pass" -ne "$expected" ]; then
  echo "FAILED — $pass of $expected case(s) passed, $fail failed" >&2
  exit 1
fi
echo "OK — $pass case(s) of $expected behaved as required"
