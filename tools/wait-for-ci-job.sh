#!/usr/bin/env bash
# Wait, inside a CI job, for another job of the SAME workflow run to finish, and fail unless it
# succeeded.
#
# WHY (owner (83)(1), 2026-10-29). The web jobs used to `needs: [test]`, because the browser
# differential compares against two files only a job with the NATIVE engines can build: the
# conformance record and the generated redaction corpus. `needs:` delays the whole job. The owner's
# answer is a short `native-record` job that builds just those two, with the web jobs starting at
# the same time as it and only the browser comparison waiting. GitHub Actions has no way to wait
# for another job in the middle of a job, so this is that wait: it asks the Actions API for the
# named job's state in this run, polls until it completes, and exits 0 only on `success`.
#
# FAILS CLOSED. Any other conclusion, the job still missing from the run after a few polls, API errors that persist (5 in a row), or
# the deadline passing exits non-zero, and the web job goes red. The artifact downloads that follow
# would also fail on a missing artifact; this exists so that a red native job reads as that rather
# than as a confusing download error, and so the wait has a deadline.
#
# No file content: it reads job metadata of this run, with the job's own token (`actions: read`).
#
# Usage: tools/wait-for-ci-job.sh <job name as the run shows it> [deadline seconds, default 2700]
#   Needs GH_TOKEN, GITHUB_REPOSITORY and GITHUB_RUN_ID (set in Actions). Not run locally: there,
#   `tools/ci-local.py` builds the two files in jobs that run before `web-e2e`.
set -euo pipefail

name="${1:?usage: wait-for-ci-job.sh <job name> [deadline seconds]}"
deadline_s="${2:-2700}"
: "${GITHUB_REPOSITORY:?not in Actions: GITHUB_REPOSITORY is unset}"
: "${GITHUB_RUN_ID:?not in Actions: GITHUB_RUN_ID is unset}"
interval="${BURROW_WAIT_INTERVAL_S:-15}"

start=$SECONDS
errors=0
missing=0
max_missing="${BURROW_WAIT_MISSING_POLLS:-5}"
while :; do
  status=0
  # `filter=latest`: on a re-run, the latest attempt of each job, which is the one whose
  # artifacts a re-run produces.
  state="$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID/jobs?filter=latest&per_page=100" \
    --jq ".jobs[] | select(.name == \"$name\") | \"\(.status) \(.conclusion)\"")" || status=$?
  if [ "$status" -ne 0 ]; then
    errors=$((errors + 1))
    echo "wait-for-ci-job: API error $errors in a row reading this run's jobs (status $status)" >&2
    [ "$errors" -lt 5 ] || { echo "::error::wait-for-ci-job: the Actions API failed $errors times in a row; giving up" >&2; exit 1; }
  else
    errors=0 # a successful read ends a run of errors: only CONSECUTIVE errors count
    count="$(printf '%s\n' "$state" | grep -c . || true)"
    if [ "$count" -eq 0 ]; then
      # A job that has not been scheduled yet is not in the list: tolerate that for a few polls,
      # BOUNDED, so a misspelt name still fails (closed) instead of waiting out the deadline.
      missing=$((missing + 1))
      echo "wait-for-ci-job: no job named '$name' yet in run $GITHUB_RUN_ID (poll $missing of $max_missing)" >&2
      if [ "$missing" -ge "$max_missing" ]; then
        echo "::error::wait-for-ci-job: no job named '$name' in run $GITHUB_RUN_ID after $missing polls" >&2
        exit 1
      fi
    else
      if [ "$count" -ne 1 ]; then
        echo "::error::wait-for-ci-job: $count jobs named '$name' in run $GITHUB_RUN_ID; expected one" >&2
        exit 1
      fi
      read -r job_status conclusion <<<"$state"
      if [ "$job_status" = "completed" ]; then
        if [ "$conclusion" = "success" ]; then
          echo "wait-for-ci-job: '$name' succeeded, after $((SECONDS - start)) s of waiting"
          exit 0
        fi
        echo "::error::wait-for-ci-job: '$name' concluded '$conclusion', so its files cannot be trusted" >&2
        exit 1
      fi
      echo "wait-for-ci-job: '$name' is $job_status ($((SECONDS - start)) s)"
    fi
  fi
  if [ $((SECONDS - start)) -ge "$deadline_s" ]; then
    echo "::error::wait-for-ci-job: '$name' had not completed after ${deadline_s} s" >&2
    exit 1
  fi
  sleep "$interval"
done
