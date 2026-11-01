#!/usr/bin/env bash
# Fetch main's latest `web-size-lines` CI artifact, with no third-party action (DECISIONS rule 18,
# owner (85)(3), (92); PR-B). The artifact is the x86_64 build's size lines, written by
# tools/check-size-lines.sh and uploaded by ci.yml's `web (chromium)` on every run and by
# fuzz-nightly.yml's `web-size-lines` on main. tools/check-size-vs-main.sh compares against it.
#
#   tools/fetch-main-size-lines.sh --out <dir>
#
# WRITES <dir>/size-lines.json   the artifact, extracted (only that one member, size-capped)
#        <dir>/size-budget.json  main's recording AT THE ARTIFACT'S COMMIT, not at main's tip, so a
#                                merge that landed after the artifact cannot make a true recording
#                                look false
#        <dir>/meta.json         run_id, head_sha, event, workflow, artifact_id, created_at, expires_at
#
# WHICH RUN. The newest run on branch `main` that SUCCEEDED, of ci.yml with event `push`, or a
# completed run of fuzz-nightly.yml with event `schedule` whose `web size lines (main)` JOB succeeded
# (the run itself is red whenever any fuzz job is, which must not hide this job's artifact), other
# than the run this job belongs to. Pull-request and
# merge-group runs upload the same artifact and are excluded on purpose: their sizes are not main's,
# and a baseline taken from an unmerged PR would let that PR approve itself. A red main run is not a
# baseline either. Runs are walked newest first; one with no artifact, or an expired one, is passed
# over and counted, because retention is 30 days and a run older than that is older than every
# newer run too.
#
# FAILS CLOSED, NEVER A PASS. Exit 1, naming the reason, when: the Actions API fails (listing runs,
# a nightly run's jobs, or artifacts); no usable main run is listed; no run in the window has a live artifact (counting how many had none and how
# many had expired); the artifact does not belong to the run it was listed under or is not on
# `main`; the download fails; the zip is not exactly one `size-lines.json` within 1 MiB; main's
# recording cannot be fetched at the run's commit. An expired or missing artifact is a STOP rather
# than a skip, per the owner: "a missing or expired main artifact is a hard stop" (92)(Q5).
#
# No file content: it reads run and artifact metadata and one small JSON with the job's own token
# (`actions: read`), and fetches a commit of this repository.
#
# Needs: GH_TOKEN, GITHUB_REPOSITORY. Optional: GITHUB_RUN_ID (excluded), BURROW_SIZE_GIT_DIR (the
# checkout to fetch main's recording into; default: the repository containing this script),
# BURROW_SIZE_RUN_WINDOW (runs listed per workflow, default 30).
# Not run locally: there is no main artifact on a developer machine. `tools/ci-local.py` carries
# the argued exemption and runs the stubbed-`gh` self-test, tools/test-fetch-main-size-lines.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="${BURROW_SIZE_GIT_DIR:-$(cd "$here/.." && pwd)}"
window="${BURROW_SIZE_RUN_WINDOW:-30}"
artifact_name="web-size-lines"
size_job_name="web size lines (main)"

stop() {
  echo "::error::fetch-main-size-lines: STOP -- $*" >&2
  exit 1
}
usage() {
  echo "usage: tools/fetch-main-size-lines.sh --out <dir>" >&2
  exit 2
}

[ "$#" -eq 2 ] && [ "$1" = "--out" ] || usage
out="$2"
: "${GITHUB_REPOSITORY:?not in Actions: GITHUB_REPOSITORY is unset}"
this_run="${GITHUB_RUN_ID:-}"
# A failed fetch leaves NOTHING behind: a stale <dir> from an earlier run is exactly the file a
# comparison would otherwise read as main's.
rm -rf "$out"
mkdir -p "$out"
case "$window" in '' | *[!0-9]*) stop "BURROW_SIZE_RUN_WINDOW is not a number: $window" ;; esac

# --- the candidate runs, newest first -------------------------------------------------------------
candidates=""
# ci.yml is taken on the RUN's conclusion. fuzz-nightly.yml is taken on the conclusion of its
# `web size lines (main)` JOB (below): that run is red whenever any fuzz job is, and a red fuzz job
# says nothing about this job's artifact. The run only has to be completed.
for spec in "ci.yml:push:success" "fuzz-nightly.yml:schedule:completed"; do
  IFS=: read -r workflow event run_status <<<"$spec"
  status=0
  if [ "$run_status" = success ]; then
    conclusion_test='.conclusion == "success"'
  else
    conclusion_test='.status == "completed"'
  fi
  listed="$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/$workflow/runs?branch=main&event=$event&status=$run_status&per_page=$window" \
    --jq ".workflow_runs[] | select(.head_branch == \"main\" and .event == \"$event\" and $conclusion_test) | [.created_at, (.id | tostring), .head_sha, \"$workflow\", \"$event\"] | @tsv")" || status=$?
  [ "$status" -eq 0 ] || stop "the Actions API failed listing $workflow runs (status $status)"
  [ -z "$listed" ] || candidates="$candidates$listed"$'\n'
done
candidates="$(printf '%s' "$candidates" | grep . | sort -r || true)"
[ -n "$candidates" ] || stop "no successful run of ci.yml (push) or fuzz-nightly.yml (schedule) on main is listed; there is no main artifact to compare against"

# --- the first run with a live artifact -------------------------------------------------------------
examined=0
no_artifact=0
expired=0
size_job_bad=0
chosen=""
while IFS=$'\t' read -r created run_id head_sha workflow event; do
  [ -n "$run_id" ] || continue
  [ "$run_id" != "$this_run" ] || continue # never this run's own artifact: that would be comparing with itself
  case "$run_id" in '' | *[!0-9]*) stop "a run id is not numeric: $run_id" ;; esac
  case "$head_sha" in *[!0-9a-f]* | '') stop "run $run_id has a malformed head sha: $head_sha" ;; esac
  [ "${#head_sha}" -eq 40 ] || stop "run $run_id has a malformed head sha: $head_sha"
  examined=$((examined + 1))
  if [ "$workflow" = "fuzz-nightly.yml" ]; then
    # The nightly's run may be red for an unrelated job; what matters is the job that made the file.
    status=0
    job_conclusion="$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run_id/jobs?per_page=100" \
      --jq "[.jobs[] | select(.name == \"$size_job_name\") | .conclusion] | first // \"absent\"")" || status=$?
    [ "$status" -eq 0 ] || stop "the Actions API failed listing run $run_id's jobs (status $status)"
    if [ "$job_conclusion" != "success" ]; then
      size_job_bad=$((size_job_bad + 1))
      echo "fetch-main-size-lines: run $run_id ($event, ${head_sha:0:9}): the '$size_job_name' job is '$job_conclusion', not success; passing over it" >&2
      continue
    fi
  fi
  status=0
  art="$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run_id/artifacts?name=$artifact_name&per_page=100" \
    --jq ".artifacts[] | select(.name == \"$artifact_name\") | [(.id | tostring), (.expired | tostring), .created_at, .expires_at, (.workflow_run.head_branch // \"\"), ((.workflow_run.id // 0) | tostring)] | @tsv")" || status=$?
  [ "$status" -eq 0 ] || stop "the Actions API failed listing run $run_id's artifacts (status $status)"
  if [ -z "$art" ]; then
    no_artifact=$((no_artifact + 1))
    echo "fetch-main-size-lines: run $run_id ($event, ${head_sha:0:9}) has no $artifact_name artifact; passing over it" >&2
    continue
  fi
  # The newest artifact of that name in the run (a re-run uploads another), live or not.
  IFS=$'\t' read -r art_id art_expired art_created art_expires art_branch art_run <<<"$(printf '%s\n' "$art" | sort -t$'\t' -k3 -r | head -n 1)"
  if [ "$art_expired" != "false" ]; then
    expired=$((expired + 1))
    echo "fetch-main-size-lines: run $run_id's $artifact_name artifact $art_id expired $art_expires; passing over it" >&2
    continue
  fi
  [ "$art_branch" = "main" ] || stop "artifact $art_id is listed under run $run_id but belongs to branch '$art_branch', not main"
  [ "$art_run" = "$run_id" ] || stop "artifact $art_id is listed under run $run_id but belongs to run $art_run"
  chosen="$run_id"
  break
done <<<"$candidates"

if [ -z "$chosen" ]; then
  stop "no live $artifact_name artifact among the $examined newest successful main run(s) (excluding this one): $no_artifact had none, $expired had expired, $size_job_bad were nightly runs whose size job had not succeeded. An expired or missing main artifact is a stop, not a pass (owner (92)(Q5)); a run on main re-creates it"
fi

# --- download, extract exactly one member -----------------------------------------------------------
zip="$(mktemp)"
trap 'rm -f "$zip"' EXIT
status=0
gh api "repos/$GITHUB_REPOSITORY/actions/artifacts/$art_id/zip" >"$zip" || status=$?
[ "$status" -eq 0 ] || stop "the download of artifact $art_id failed (status $status)"
status=0
python3 -I - "$zip" "$out/size-lines.json" <<'PY' || status=$?
import sys, zipfile
src, dst = sys.argv[1], sys.argv[2]
try:
    with zipfile.ZipFile(src) as z:
        names = z.namelist()
        if names != ["size-lines.json"]:
            sys.exit("the zip holds %r, expected exactly ['size-lines.json']" % (names,))
        # The cap is on the bytes actually read, not on the size the header declares.
        with z.open("size-lines.json") as member:
            data = member.read((1 << 20) + 1)
        if len(data) < 1 or len(data) > 1 << 20:
            sys.exit("size-lines.json is %s bytes, expected 1 to 1048576" % ("more than 1048576" if len(data) > 1 << 20 else len(data)))
except zipfile.BadZipFile as e:
    sys.exit("not a zip: %s" % e)
with open(dst, "wb") as f:
    f.write(data)
PY
[ "$status" -eq 0 ] || stop "artifact $art_id is not a usable size-lines zip"

# --- main's recording at the artifact's own commit --------------------------------------------------
status=0
git -C "$repo" fetch --no-tags --depth=1 origin "$head_sha" >&2 || status=$?
[ "$status" -eq 0 ] || stop "cannot fetch commit $head_sha (run $chosen) from origin, so main's recording at that commit cannot be read"
status=0
git -C "$repo" show "FETCH_HEAD:apps/web/size-budget.json" >"$out/size-budget.json" || status=$?
[ "$status" -eq 0 ] || stop "commit $head_sha has no apps/web/size-budget.json"
status=0
python3 -I -c 'import json,sys; json.load(open(sys.argv[1]))' "$out/size-budget.json" || status=$?
[ "$status" -eq 0 ] || stop "main's apps/web/size-budget.json at $head_sha is not JSON"

python3 -I - "$out/meta.json" "$chosen" "$head_sha" "$event" "$workflow" "$art_id" "$art_created" "$art_expires" <<'PY'
import json, sys
keys = ["run_id", "head_sha", "event", "workflow", "artifact_id", "created_at", "expires_at"]
with open(sys.argv[1], "w") as f:
    json.dump(dict(zip(keys, sys.argv[2:])), f, indent=2)
    f.write("\n")
PY

echo "fetch-main-size-lines: OK -- run $chosen ($workflow, $event) at ${head_sha}; artifact $art_id created $art_created, expires $art_expires; examined $examined newest successful main run(s), passed over $no_artifact without an artifact and $expired expired" >&2
