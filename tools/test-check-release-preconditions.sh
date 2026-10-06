#!/usr/bin/env bash
# Adversarial self-test for tools/check-release-preconditions.py.
#
# The decision is what guards the Cloudflare credential on a tag push, so every refusal it is
# meant to make is planted here as a fact set and required to refuse NAMING ITS REASON. The
# baseline is one fact set that passes; each case below is that baseline with exactly one fact
# broken, so a refusal can only come from the gate under test.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-release-preconditions.py"
passed=0
failed=0
SHA=0123456789abcdef0123456789abcdef01234567

# The passing baseline: a v* annotated tag, merged, both runs green on the sha, stamp current.
baseline() {
  cat <<JSON
{
  "ref": "refs/tags/v1.2.3",
  "tag_object_type": "tag",
  "tagged_sha": "$SHA",
  "merged": true,
  "ci_run": {"conclusion": "success", "head_sha": "$SHA", "event": "push",
             "jobs": [{"name": "ci", "conclusion": "success"},
                      {"name": "web", "conclusion": "skipped"}]},
  "build_run": {"conclusion": "success", "head_sha": "$SHA", "event": "push",
                "jobs": [{"name": "build the production payload", "conclusion": "success"}]},
  "stamp_current": true
}
JSON
}

# expect <pass|fail> <label> <needle-in-stderr-when-failing> < facts
expect() {
  local want=$1 label=$2 needle=$3 out status
  out=$("$checker" 2>&1)
  status=$?
  if [ "$want" = pass ]; then
    if [ "$status" -eq 0 ]; then
      echo "  ok   $label"
      passed=$((passed + 1))
    else
      echo "  FAIL $label: expected pass, got $status"
      echo "$out" | sed 's/^/       /'
      failed=$((failed + 1))
    fi
  else
    if [ "$status" -eq 0 ]; then
      echo "  FAIL $label: expected refusal, passed instead"
      failed=$((failed + 1))
    elif grep -qF -- "$needle" <<<"$out"; then
      echo "  ok   $label"
      passed=$((passed + 1))
    else
      echo "  FAIL $label: refused, but not for '$needle'"
      echo "$out" | sed 's/^/       /'
      failed=$((failed + 1))
    fi
  fi
}

# `jq`-free edit of the baseline JSON: python one-liners keyed by the case.
mutate() {
  baseline | python3 -c "import json,sys; d=json.load(sys.stdin); $1; print(json.dumps(d))"
}

echo "check-release-preconditions self-test:"

expect pass "the baseline passes" "" < <(baseline)

# 1. a tag on a commit that isn't on main
expect fail "a commit not on main is refused" "not merged" < <(mutate "d['merged']=False")

# 2. a tag on a main commit whose ci run was red
expect fail "a red ci run is refused" "ci run concluded" \
  < <(mutate "d['ci_run']['conclusion']='failure'")
expect fail "a red ci JOB is refused even if the run says success" "job" \
  < <(mutate "d['ci_run']['jobs'][0]['conclusion']='failure'")

# 2b. ci green but the deploy build red, and the reverse -- both refuse
expect fail "ci green but the build red is refused" "deploy build run concluded" \
  < <(mutate "d['build_run']['conclusion']='failure'")
expect fail "the build green but ci red is refused" "ci run concluded" \
  < <(mutate "d['ci_run']['conclusion']='failure'")

# 3. a lightweight tag
expect fail "a lightweight tag is refused" "annotated tag" \
  < <(mutate "d['tag_object_type']='commit'")

# 4. an expired or missing artifact shows as a build run that cannot be confirmed, or a stamp
#    that is not current -- the workflow turns 'artifact gone' into stamp_current:false.
expect fail "a stamp that does not match the tag is refused" "build stamp is not current" \
  < <(mutate "d['stamp_current']=False")
expect fail "a missing build run is refused" "deploy build run is absent" \
  < <(mutate "del d['build_run']")

# 5. the run is for another commit (headSha mismatch) -- a run started for a different SHA
expect fail "a ci run for another commit is refused" "not the tagged commit" \
  < <(mutate "d['ci_run']['head_sha']='ffffffffffffffffffffffffffffffffffffffff'")

# 6. the run is a pull_request event, not the gated main push
expect fail "a non-push ci event is refused" "not a branch push" \
  < <(mutate "d['ci_run']['event']='pull_request'")

# 7. a non-v tag, and absent facts
expect fail "a non-v tag is refused" "not a refs/tags/v* tag" \
  < <(mutate "d['ref']='refs/tags/release-1'")
expect fail "a missing stamp fact is refused, not assumed true" "build stamp is not current" \
  < <(mutate "del d['stamp_current']")

if [ "$failed" -gt 0 ]; then
  echo "FAILED -- $failed case(s) failed, $passed passed"
  exit 1
fi
echo "OK -- $passed precondition case(s) all behaved as required"
