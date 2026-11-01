#!/usr/bin/env bash
# Self-test for tools/fetch-main-size-lines.sh: a fake `gh` on PATH serves recorded-shape JSON from a
# fixture directory (the real jq applies the script's own `--jq` filters to it, so the filters are
# exercised, not assumed), and a throwaway git repository stands in for `origin`. No network.
#
# EVERY STOP IS A CASE, and the script is held to a message naming its reason, because the failure
# this guards is a baseline that was silently something else: no main run listed, no live artifact
# (none uploaded vs expired, counted), the API failing, a pull-request or red or non-main run offered
# as main, this run's own artifact, an artifact belonging to another run or branch, a download that
# fails or is not exactly one small size-lines.json, a commit that cannot be fetched, a recording that
# is missing at that commit, a stale output directory.
#
# Mutants: copies of the script BESIDE it (so nothing else moves), one rule broken, asserted
# applied; the same case run against each must no longer behave as the rule requires.
#
# Usage: tools/test-fetch-main-size-lines.sh
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/fetch-main-size-lines.sh"
mutant="$here/.fetch-main-size-lines.mutant.sh"
work="$(mktemp -d)"
cleanup() { trap - EXIT INT TERM HUP; rm -f "$mutant"; rm -rf "$work"; return 0; }
trap cleanup EXIT INT TERM HUP

for tool in jq git python3; do
  command -v "$tool" >/dev/null || { echo "REFUSED: $tool is not installed; the self-test needs it" >&2; exit 1; }
done

pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# --- the fake `gh`: gh api <url> [--jq <expr>] -------------------------------------------------------
mkdir -p "$work/bin" "$work/fix"
cat >"$work/bin/gh" <<'FAKE'
#!/usr/bin/env bash
[ "$1" = api ] || exit 64
url="$2"; shift 2
expr=""
[ "${1:-}" = "--jq" ] && expr="$2"
fix="$FAKE_DIR/fix"
echo "$url" >>"$FAKE_DIR/calls"
path="${url%%\?*}"
case "$path" in
  */actions/workflows/*/runs) key="runs-$(basename "$(dirname "$path")")"; kind=json ;;
  */actions/runs/*/artifacts) key="artifacts-$(basename "$(dirname "$path")")"; kind=json ;;
  */actions/runs/*/jobs) key="jobs-$(basename "$(dirname "$path")")"; kind=json ;;
  */actions/artifacts/*/zip) key="zip-$(basename "$(dirname "$path")")"; kind=zip ;;
  *) exit 65 ;;
esac
[ ! -e "$fix/fail-$key" ] || { echo "HTTP 502" >&2; exit 1; }
if [ "$kind" = zip ]; then cat "$fix/$key.zip"; exit 0; fi
file="$fix/$key.json"
if [ ! -e "$file" ]; then
  case "$key" in
    runs-*) echo '{"workflow_runs":[]}' >"$file" ;;
    # a run whose size job is not described succeeded in it: the default world is a green nightly
    jobs-*) echo '{"jobs":[{"name":"fuzz target","conclusion":"success"},{"name":"web size lines (main)","conclusion":"success"}]}' >"$file" ;;
    *) echo '{"artifacts":[]}' >"$file" ;;
  esac
fi
if [ -n "$expr" ]; then jq -r "$expr" "$file"; else cat "$file"; fi
FAKE
chmod +x "$work/bin/gh"

# --- the origin repository: two commits, the older one (the artifact's) holds the recording ---------
src="$work/origin-src"
git init -q -b main "$src"
git -C "$src" config user.email t@example.com
git -C "$src" config user.name t
mkdir -p "$src/apps/web"
echo '{"recording": "at the artifact commit"}' >"$src/apps/web/size-budget.json"
git -C "$src" add apps/web/size-budget.json
git -C "$src" commit -q -m one
good_sha="$(git -C "$src" rev-parse HEAD)"
git -C "$src" rm -q apps/web/size-budget.json
git -C "$src" commit -q -m two
nobudget_sha="$(git -C "$src" rev-parse HEAD)"
echo x >"$src/f" && git -C "$src" add f && git -C "$src" commit -q -m three
git init -q "$work/clone"
git -C "$work/clone" remote add origin "$src"
absent_sha="$(printf 'dead%.0s' 1 2 3 4 5 6 7 8 9 10)"

# --- fixture builders --------------------------------------------------------------------------------
reset() { rm -rf "$work/fix"; mkdir -p "$work/fix"; rm -f "$work/calls"; }
# runs <workflow> <json array of run objects>
runs() { printf '{"workflow_runs": %s}\n' "$2" >"$work/fix/runs-$1.json"; }
run_obj() { # id created sha event [branch] [conclusion]
  printf '{"id": %s, "created_at": "%s", "head_sha": "%s", "event": "%s", "head_branch": "%s", "status": "completed", "conclusion": "%s"}' \
    "$1" "$2" "$3" "$4" "${5:-main}" "${6:-success}"
}
# jobs <run id> <conclusion of the size job, or "none" for no such job at all>, beside a failed fuzz job
jobs() {
  if [ "$2" = none ]; then printf '{"jobs": [{"name": "fuzz target", "conclusion": "failure"}]}\n' >"$work/fix/jobs-$1.json"
  else printf '{"jobs": [{"name": "fuzz target", "conclusion": "failure"}, {"name": "web size lines (main)", "conclusion": "%s"}]}\n' "$2" >"$work/fix/jobs-$1.json"; fi
}
# artifacts <run id> <artifact id> <expired true|false> [branch] [workflow run id]
artifacts() {
  printf '{"artifacts": [{"id": %s, "name": "web-size-lines", "expired": %s, "created_at": "2026-11-01T00:00:00Z", "expires_at": "2026-12-01T00:00:00Z", "workflow_run": {"id": %s, "head_branch": "%s"}}]}\n' \
    "$2" "$3" "${5:-$1}" "${4:-main}" >"$work/fix/artifacts-$1.json"
}
zipof() { # artifact id, then members as name=size_bytes
  local id="$1"; shift
  python3 -I - "$work/fix/zip-$id.zip" "$@" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w") as z:
    for spec in sys.argv[2:]:
        name, size = spec.rsplit("=", 1)
        body = b'{"format": 1, "marker": "' + name.encode() + b'"}'
        z.writestr(name, body + b" " * max(0, int(size) - len(body)))
PY
}
good_zip() { zipof "$1" "size-lines.json=100"; }

# case <label> <want status 0|1> <want text in stderr or ""> [env k=v ...]   (fixtures already laid out)
fetch() {
  local out="$work/out" status=0
  env PATH="$work/bin:$PATH" FAKE_DIR="$work" GITHUB_REPOSITORY=o/r BURROW_SIZE_GIT_DIR="$work/clone" \
    "$@" "$S" --out "$out" >"$work/stdout" 2>"$work/stderr" || status=$?
  return $status
}
S="$script"
expect() { # <label> <want-status> <text> [env...]
  local label="$1" want="$2" text="$3"; shift 3
  local status=0
  fetch "$@" || status=$?
  if [ "$status" -ne "$want" ]; then bad "$label: exit $status, wanted $want"; sed 's/^/        /' "$work/stderr" | tail -3; return; fi
  if [ -n "$text" ] && ! grep -qF -- "$text" "$work/stderr"; then bad "$label: right exit, but stderr lacks '$text'"; sed 's/^/        /' "$work/stderr" | tail -3; return; fi
  ok "$label"
}
chosen() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["run_id"])' "$work/out/meta.json" 2>/dev/null; }

# A standard world: ci.yml push 300 (newest) and 200, the nightly 250, each with a live artifact.
world() {
  reset
  runs ci.yml "[$(run_obj 300 2026-11-01T12:00:00Z "$good_sha" push), $(run_obj 200 2026-10-30T12:00:00Z "$good_sha" push)]"
  runs fuzz-nightly.yml "[$(run_obj 250 2026-10-31T03:00:00Z "$good_sha" schedule)]"
  for r in 300 200 250; do artifacts "$r" "$((r * 10))" false; good_zip "$((r * 10))"; done
}

echo "the lookup and its stops"
world
expect "the newest successful main run's artifact is taken" 0 "run 300 (ci.yml, push)"
[ "$(chosen)" = 300 ] && ok "meta.json names run 300" || bad "meta.json run_id is '$(chosen)', wanted 300"
cmp -s "$work/out/size-budget.json" <(git -C "$src" show "$good_sha:apps/web/size-budget.json") && ok "size-budget.json is main's recording AT THE ARTIFACT'S COMMIT" || bad "size-budget.json is not the artifact commit's"
grep -q '"marker": "size-lines.json"' "$work/out/size-lines.json" && ok "size-lines.json is the artifact's member" || bad "size-lines.json is not the artifact's"
grep -qF "examined 1 newest successful main run(s)" "$work/stderr" && ok "it reports how many runs it examined" || bad "no examined count"

world
expect "this run's own artifact is excluded" 0 "run 250" GITHUB_RUN_ID=300
[ "$(chosen)" = 250 ] && ok "...so the next newest (the nightly, 250) is taken" || bad "chose '$(chosen)', wanted 250"

world
runs ci.yml "[$(run_obj 300 2026-11-01T12:00:00Z "$good_sha" push), $(run_obj 200 2026-10-30T12:00:00Z "$good_sha" push)]"
runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule)]"; artifacts 350 3500 false; good_zip 3500
expect "a newer nightly run beats an older push run" 0 "run 350 (fuzz-nightly.yml, schedule)"

world
runs ci.yml "[$(run_obj 400 2026-11-02T00:00:00Z "$good_sha" pull_request feature), $(run_obj 390 2026-11-01T23:00:00Z "$good_sha" push feature), $(run_obj 380 2026-11-01T22:00:00Z "$good_sha" push main failure), $(run_obj 370 2026-11-01T21:00:00Z "$good_sha" merge_group main), $(run_obj 300 2026-11-01T12:00:00Z "$good_sha" push)]"
for r in 400 390 380 370; do artifacts "$r" "$((r * 10))" false; good_zip "$((r * 10))"; done
expect "pull-request, other-branch, red and merge-group runs are not baselines" 0 "run 300 (ci.yml, push)"
[ "$(chosen)" = 300 ] && ok "...run 300 was taken, not 400/390/380/370" || bad "chose '$(chosen)'"

world
rm "$work/fix/artifacts-300.json"
expect "a newest run with no artifact is passed over, and counted" 0 "passed over 1 without an artifact"
[ "$(chosen)" = 250 ] && ok "...the next newest is taken (250)" || bad "chose '$(chosen)', wanted 250"

world
for r in 300 200 250; do artifacts "$r" "$((r * 10))" true; done
expect "STOP: every artifact has expired" 1 "3 had expired"
grep -qF "expired or missing main artifact is a stop, not a pass" "$work/stderr" && ok "...and says it is a stop, not a pass" || bad "no stop-not-a-pass wording"
[ ! -e "$work/out/meta.json" ] && ok "...and leaves no meta.json behind" || bad "a failed fetch left meta.json"

world
for r in 300 200 250; do rm "$work/fix/artifacts-$r.json"; done
expect "STOP: no run has an artifact (the pre-#437 history)" 1 "3 had none"

world
reset
expect "STOP: no successful main run is listed at all" 1 "no successful run of ci.yml (push) or fuzz-nightly.yml (schedule) on main"

world
touch "$work/fix/fail-runs-ci.yml"
expect "STOP: the Actions API fails listing runs" 1 "the Actions API failed listing ci.yml runs"

world
touch "$work/fix/fail-artifacts-300"
expect "STOP: the Actions API fails listing a run's artifacts" 1 "the Actions API failed listing run 300's artifacts"

world
touch "$work/fix/fail-zip-3000"
expect "STOP: the download fails" 1 "the download of artifact 3000 failed"

world
zipof 3000 "size-lines.json=100" "extra.json=10"
expect "STOP: a zip with a second member" 1 "not a usable size-lines zip"
world
zipof 3000 "../evil.json=100"
expect "STOP: a zip whose only member is not size-lines.json (a path-escaping name)" 1 "not a usable size-lines zip"
world
zipof 3000 "size-lines.json=2097152"
expect "STOP: a size-lines.json over 1 MiB" 1 "not a usable size-lines zip"
world
zipof 3000 "size-lines.json=1048576"
expect "twin: exactly 1 MiB is accepted" 0 "run 300"
world
printf 'not a zip' >"$work/fix/zip-3000.zip"
expect "STOP: not a zip at all" 1 "not a usable size-lines zip"

world
artifacts 300 3000 false feature
expect "STOP: an artifact listed under a main run but belonging to another branch" 1 "belongs to branch 'feature', not main"
world
artifacts 300 3000 false main 999
expect "STOP: an artifact belonging to a different run than the one listing it" 1 "belongs to run 999"

world
runs ci.yml "[$(run_obj 300 2026-11-01T12:00:00Z "$absent_sha" push)]"; runs fuzz-nightly.yml "[]"
expect "STOP: the artifact's commit cannot be fetched from origin" 1 "cannot fetch commit $absent_sha"
world
runs ci.yml "[$(run_obj 300 2026-11-01T12:00:00Z "$nobudget_sha" push)]"; runs fuzz-nightly.yml "[]"
expect "STOP: the commit has no apps/web/size-budget.json" 1 "has no apps/web/size-budget.json"

world
mkdir -p "$work/out" && echo '{"run_id": "STALE"}' >"$work/out/meta.json"
touch "$work/fix/fail-runs-ci.yml"
expect "a failed fetch removes a stale output directory's contents" 1 "STOP"
[ ! -e "$work/out/meta.json" ] && ok "...the stale meta.json is gone" || bad "a stale meta.json survived a failed fetch"

world
status=0; env PATH="$work/bin:$PATH" FAKE_DIR="$work" GITHUB_REPOSITORY=o/r "$script" >/dev/null 2>&1 || status=$?
[ "$status" -eq 2 ] && ok "no --out is a usage error (exit 2)" || bad "no --out exited $status"
status=0; env -u GITHUB_REPOSITORY PATH="$work/bin:$PATH" "$script" --out "$work/o" >/dev/null 2>&1 || status=$?
[ "$status" -ne 0 ] && ok "no GITHUB_REPOSITORY is refused" || bad "no GITHUB_REPOSITORY was accepted"

# SF-2: the nightly is judged by its size job, not by the run (red whenever any fuzz job is).
world
runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 success
expect "SF-2 a nightly run that is red for another job, whose size job succeeded, is used" 0 "run 350 (fuzz-nightly.yml, schedule)"
world
runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 failure
expect "SF-2 twin: a nightly whose size job failed is passed over, naming the job" 0 "the 'web size lines (main)' job is 'failure', not success"
[ "$(chosen)" = 300 ] && ok "...and the newest push run (300) is taken instead" || bad "chose '$(chosen)', wanted 300"
world
runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 none
expect "SF-2 a nightly run with no size job at all is passed over as 'absent'" 0 "job is 'absent'"
world
runs ci.yml "[]"; runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 failure
expect "SF-2 STOP: the only run is a nightly whose size job failed, counted" 1 "1 were nightly runs whose size job had not succeeded"
world
runs ci.yml "[]"
touch "$work/fix/fail-jobs-250"
expect "SF-2 STOP: the Actions API fails listing a nightly run's jobs" 1 "the Actions API failed listing run 250's jobs"

echo
echo "mutants (a copy of the script with one rule broken must stop behaving as the rule requires)"
mutate() { # <old> <new>
  python3 -I - "$script" "$mutant" "$1" "$2" <<'PY'
import sys
src, dst, old, new = sys.argv[1:]
s = open(src).read()
if old not in s:
    sys.exit("MUTATION DID NOT APPLY: " + old)
t = s.replace(old, new, 1)
if t == s:
    sys.exit("MUTATION CHANGED NOTHING")
open(dst, "w").write(t)
PY
  chmod +x "$mutant"
}
# mutant_stops <name> <old> <new> <world setup fn> <text the mutant must stop saying> [env...]: a STOP
# case where the broken script must no longer stop for that reason.
mutant_stops() {
  local name="$1" old="$2" new="$3" setup="$4" text="$5"; shift 5
  mutate "$old" "$new" || { bad "$name: the mutation did not apply"; return; }
  $setup
  S="$mutant"; fetch "$@"; local status=$?; S="$script"
  if [ "$status" -ne 0 ] && grep -qF -- "$text" "$work/stderr"; then bad "$name: the mutant STILL stops with '$text'"; return; fi
  ok "$name: the mutant no longer stops with '$text' (exit $status)"
}
# mutant_picks <name> <old> <new> <setup> <run id the correct script picks>: the broken script picks another.
mutant_picks() {
  local name="$1" old="$2" new="$3" setup="$4" right="$5"; shift 5
  mutate "$old" "$new" || { bad "$name: the mutation did not apply"; return; }
  $setup
  S="$mutant"; fetch "$@"; local status=$?; S="$script"
  local got; got="$(chosen)"
  if [ "$status" -eq 0 ] && [ "$got" = "$right" ]; then bad "$name: the mutant still picks run $right"; return; fi
  ok "$name: the mutant picks '${got:-nothing}', not $right"
}
w_expired() { world; for r in 300 200 250; do artifacts "$r" "$((r * 10))" true; done; }
w_notzip2() { world; zipof 3000 "size-lines.json=100" "extra.json=10"; }
w_big() { world; zipof 3000 "size-lines.json=2097152"; }
w_branch() { world; artifacts 300 3000 false feature; }
w_runmismatch() { world; artifacts 300 3000 false main 999; }
w_absent() { world; runs ci.yml "[$(run_obj 300 2026-11-01T12:00:00Z "$absent_sha" push)]"; runs fuzz-nightly.yml "[]"; }
w_none() { world; for r in 300 200 250; do rm "$work/fix/artifacts-$r.json"; done; }
w_listfail() { world; touch "$work/fix/fail-runs-ci.yml"; }
w_artfail() { world; touch "$work/fix/fail-artifacts-300"; }
w_stale() { world; mkdir -p "$work/out"; echo '{"run_id": "STALE"}' >"$work/out/meta.json"; touch "$work/fix/fail-runs-ci.yml"; }
w_pr() { world; runs ci.yml "[$(run_obj 400 2026-11-02T00:00:00Z "$good_sha" pull_request feature), $(run_obj 300 2026-11-01T12:00:00Z "$good_sha" push)]"; artifacts 400 4000 false feature 400; good_zip 4000; }
w_red() { world; runs ci.yml "[$(run_obj 380 2026-11-01T22:00:00Z "$good_sha" push main failure), $(run_obj 300 2026-11-01T12:00:00Z "$good_sha" push)]"; artifacts 380 3800 false; good_zip 3800; }
w_nightly() { world; runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule)]"; artifacts 350 3500 false; good_zip 3500; }

mutant_stops "all-expired STOP" 'if [ "$art_expired" != "false" ]; then' 'if false; then' w_expired "3 had expired"
mutant_stops "no-live-artifact STOP" 'if [ -z "$chosen" ]; then' 'if false; then' w_none "no live web-size-lines artifact"
mutant_picks "this run's own artifact is not excluded" '[ "$run_id" != "$this_run" ] || continue' 'true' world 250 GITHUB_RUN_ID=300
mutant_picks "any run offered, not only main push/schedule success (the PR run)" 'select(.head_branch == \"main\" and .event == \"$event\" and $conclusion_test)' 'select(true)' w_pr 300
mutant_picks "a red run offered as a baseline" 'select(.head_branch == \"main\" and .event == \"$event\" and $conclusion_test)' 'select(true)' w_red 300
mutant_picks "newest first becomes oldest first" 'sort -r' 'sort' w_nightly 350
mutant_stops "the artifact-branch rule" '[ "$art_branch" = "main" ] || stop' 'true || stop' w_branch "belongs to branch"
mutant_stops "the artifact-run rule" '[ "$art_run" = "$run_id" ] || stop' 'true || stop' w_runmismatch "belongs to run 999"
mutant_stops "the zip-member rule" 'if names != ["size-lines.json"]:' 'if False:' w_notzip2 "not a usable size-lines zip"
w_nightly_redrun() { world; runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 success; }
w_nightly_badjob() { world; runs fuzz-nightly.yml "[$(run_obj 350 2026-11-01T13:00:00Z "$good_sha" schedule main failure)]"; artifacts 350 3500 false; good_zip 3500; jobs 350 failure; }
w_jobsfail() { world; runs ci.yml "[]"; touch "$work/fix/fail-jobs-250"; }
mutant_picks "SF-2 reverted: the nightly is judged by its RUN's conclusion" "'.status == \"completed\"'" "'.conclusion == \"success\"'" w_nightly_redrun 350
mutant_picks "SF-2 the nightly's size-job check is skipped" 'if [ "$job_conclusion" != "success" ]; then' 'if false; then' w_nightly_badjob 300
mutant_stops "SF-2 the API-failure stop on a nightly run's jobs" '[ "$status" -eq 0 ] || stop "the Actions API failed listing run $run_id'"'"'s jobs (status $status)"' 'true' w_jobsfail "the Actions API failed listing run 250's jobs"
mutant_stops "the zip-size cap" 'len(data) > 1 << 20:' 'len(data) > 1 << 40:' w_big "not a usable size-lines zip"
mutant_stops "the commit fetch" 'git -C "$repo" fetch --no-tags --depth=1 origin "$head_sha" >&2 || status=$?' 'true' w_absent "cannot fetch commit"
mutant_stops "the API-failure stop on the run listing" '[ "$status" -eq 0 ] || stop "the Actions API failed listing $workflow runs (status $status)"' 'true' w_listfail "the Actions API failed listing ci.yml runs"
mutant_stops "the API-failure stop on the artifact listing" '[ "$status" -eq 0 ] || stop "the Actions API failed listing run $run_id'"'"'s artifacts (status $status)"' 'true' w_artfail "the Actions API failed listing run 300's artifacts"
# The stale-directory case is about what is LEFT, not what is said:
mutate 'rm -rf "$out"
mkdir -p "$out"' 'mkdir -p "$out"' && {
  w_stale; S="$mutant"; fetch; S="$script"
  if [ -e "$work/out/meta.json" ]; then ok "the stale-directory cleanup: without it a failed fetch leaves a stale meta.json"; else bad "the stale-directory mutant still removed it"; fi
} || bad "the stale-directory mutation did not apply"

echo
if [ "$fail" -ne 0 ]; then echo "FAILED — $fail case(s) failed, $pass passed" >&2; exit 1; fi
echo "OK — $pass case(s) all behaved as required"
