#!/usr/bin/env bash
# The fuzz workflows' verdict step: classify one target's crash against the issues we own.
#
#   tools/fuzz-classify.sh <target> <fuzz-step-status> <log> [binary]
#
# Called by `.github/workflows/fuzz-nightly.yml` and `fuzz-reproduce.yml`, which is why it is a
# script: two workflows carrying one copy each of a verdict is two verdicts. Its exit status is
# the job's: 0 no crash or KNOWN, 1 NEW FINDING, 4 INFRASTRUCTURE FAILURE, anything else a
# CLASSIFIER FAILURE -- each reported under its own heading and annotation title, so a broken
# tool never reads as a crash verdict (2026-09-25; see tools/check-known-crashes.py EXIT_*).
#
# BURROW_CLASSIFIER replaces the classifier command, for tools/test-fuzz-report-steps.sh.
# GITHUB_STEP_SUMMARY defaults to stderr outside Actions.
#
# Self-test: tools/test-fuzz-report-steps.sh.
set -euo pipefail  # -e as the workflow step ran it: a failing command ends the step
target="$1"
status="$2"
log="$3"
binary="${4:-fuzz/target/$(uname -m)-unknown-linux-gnu/release/$target}"
: "${GITHUB_STEP_SUMMARY:=/dev/stderr}"
# OUTSIDE ACTIONS THERE IS NO RUNNER_TEMP, and `set -u` then killed this script before it
# classified anything -- found by its own self-test. A scratch directory of its own instead.
if [ -z "${RUNNER_TEMP:-}" ]; then
  RUNNER_TEMP="$(mktemp -d)"
  trap 'rm -rf "$RUNNER_TEMP"' EXIT
fi


if [ "$status" -eq 0 ]; then
  echo "no crash"
  exit 0
fi

# A FAILURE WITH NO CRASH REPORT is a build or infrastructure problem, and it fails --
# `check-known-crashes.py` refuses to classify it rather than guessing, and this must
# not be mistaken for a clean run.
# THE BANNER, ANCHORED, as the classifier and the Describe step read it: `ERROR: libFuzzer: ` inside
# a panic message is input, not a report.
if ! grep -qE '^==[0-9]+== ?ERROR: (AddressSanitizer|libFuzzer)' "$log"; then
  echo "::error title=INFRASTRUCTURE FAILURE (not a crash)::the fuzz step failed without producing a crash report" >&2
  {
    echo "### $target -- INFRASTRUCTURE FAILURE, not a crash"
    echo ""
    echo "- the fuzz step failed and no sanitiser report exists; read the step log"
  } >> "$GITHUB_STEP_SUMMARY"
  # 4, NOT 1: the step's status must not say "new finding" for a run that produced no report.
  exit 4
fi

# KNOWN -> exit 0, and the job is green with the owning issue named in the summary.
# UNMATCHED -> exit 1. That is a new defect, which is what this nightly is for.
# NO PIPE, AND `2>&1`. Both were measured wrong: the default shell is `bash -e`, so
# with `| tee` under `pipefail` a non-zero exit from the classifier aborted the step
# at the pipeline and the summary was NEVER WRITTEN -- in exactly the case this whole
# change exists to surface. And the UNMATCHED explanation goes to stderr, which a pipe
# from stdout does not carry, so the summary would have been silent about the finding
# even if it had been written. `|| verdict=$?` keeps `-e` from firing; CLAUDE.md's
# rule that a command whose status you need never goes on the left of a pipe is the
# same lesson, and the fuzz step above already cites it.
verdict=0
${BURROW_CLASSIFIER:-python3 tools/check-known-crashes.py} \
  --log "$log" \
  --binary "$binary" \
  --target "$target" \
  > "$RUNNER_TEMP/classified.txt" 2>&1 || verdict=$?
cat "$RUNNER_TEMP/classified.txt"

# THREE OUTCOMES, THREE HEADINGS. Until 2026-09-25 the classifier exited 1 for a new
# finding AND for its own failure, so this summary could not say which it was, and six
# nights of a real new finding sat beside every earlier red unread. Now a finding and a
# tooling failure carry different exit codes (tools/check-known-crashes.py EXIT_*), and
# each is reported under its own title, annotation and heading -- a broken classifier
# can never read as a crash verdict, nor a crash verdict as a broken classifier.
case "$verdict" in
  0) heading="KNOWN -- owned by a filed issue" ;;
  1)
    heading="NEW FINDING -- no filed issue owns this crash"
    echo "::error title=NEW FINDING::$target: a crash no filed issue owns" >&2
    ;;
  *)
    heading="CLASSIFIER FAILURE (exit $verdict) -- the tool could not classify; this is NOT a crash verdict"
    echo "::error title=CLASSIFIER FAILURE (tooling, not a crash)::$target: check-known-crashes exited $verdict" >&2
    ;;
esac
{
  echo "### $target -- $heading"
  echo ""
  sed 's/^/- /' "$RUNNER_TEMP/classified.txt"
} >> "$GITHUB_STEP_SUMMARY"
exit "$verdict"
