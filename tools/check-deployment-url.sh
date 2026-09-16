#!/usr/bin/env bash
# The URL a deploy reported is a deployment OF the expected origin.
#
# WHY THIS IS A SCRIPT AND NOT THREE LINES IN THE WORKFLOW. It is a gate, and `CLAUDE.md`'s
# rule is that a gate belongs in a script both CI and a test can invoke -- a gate written
# inline is a gate betting that nobody needs to test it. This one was inline, it was wrong, and
# nothing could have caught it.
#
# WHAT WENT WRONG THE FIRST TIME. The workflow asserted `reported == $BURROW_SITE`. `wrangler`
# prints the PER-DEPLOYMENT ALIAS -- `https://f55a5097.burrow-f2s.pages.dev` -- and it prints
# one for a production deploy too. So the check could never pass, and it failed a run whose
# upload had succeeded: the site was live and the deploy was reported red.
#
# Two different questions were being conflated, and they need different answers:
#
#   * WHICH DEPLOYMENT did wrangler just create? Always an alias, always different. That is
#     what this script checks: that the alias belongs to the expected project.
#   * DID PRODUCTION ACTUALLY UPDATE? Not knowable from wrangler's output at all. The step
#     after this one asks the live origin for its headers, which is the only honest way.
#
# AND WHY IT COMPARES LABELS RATHER THAN CHARACTERS. The obvious repair is a suffix match on
# `.burrow-f2s.pages.dev`, and `evil-burrow-f2s.pages.dev` ends with those characters. A host
# is a sequence of LABELS separated by dots and that is the structure the comparison has to
# use. This is the FOURTH deny-or-match rule in this repository where the string form was the
# defect -- after the force-push deny list, the `_headers` origin check using its argument as a
# regex, and the deploy workflow's triggers matched with grep. The pattern is the same every
# time: a string operation standing in for a structural comparison, correct on the examples
# anybody thought to try.
#
# Usage: tools/check-deployment-url.sh <reported-url> <expected-origin>
# Self-test: tools/test-check-deployment-url.sh

set -euo pipefail

reported="${1:-}"
expected="${2:-}"

fail() {
  echo "::error::$*" >&2
  exit 1
}

[ -n "$reported" ] || fail "no deployment URL was reported, so where this went is unknown. \
wrangler prints one on success; its absence means the upload did not report success"
[ -n "$expected" ] || fail "no expected origin given; this check cannot guess one"

case "$expected" in
  https://*) : ;;
  *) fail "the expected origin must be https://, got $expected" ;;
esac
case "$reported" in
  https://*) : ;;
  *) fail "the reported URL must be https://, got $reported" ;;
esac

host="${reported#https://}"
host="${host%%/*}"
want="${expected#https://}"
want="${want%%/*}"

[ -n "$host" ] || fail "the reported URL has no host: $reported"
[ -n "$want" ] || fail "the expected origin has no host: $expected"

# THE WHOLE HOST, or the whole host with exactly one label in front of it. `${host#*.}` strips
# up to and including the FIRST dot, which is a label boundary -- so `f55a5097.burrow-f2s.pages.dev`
# yields `burrow-f2s.pages.dev` and matches, while `evil-burrow-f2s.pages.dev` yields
# `pages.dev` and does not. That is the difference between splitting on structure and matching
# on characters.
if [ "$host" = "$want" ]; then
  echo "deployment URL is the production origin itself: $reported"
  exit 0
fi

rest="${host#*.}"
first="${host%%.*}"
if [ "$rest" = "$want" ] && [ -n "$first" ] && [ "$rest" != "$host" ]; then
  echo "deployment URL is an alias of $want: $reported"
  exit 0
fi

fail "the deploy reported $reported, which is not $want nor a deployment alias of it. A build \
is bound to one origin (ADR 0014 §4), so on another host connect-src refuses every engine fetch \
and every tool is dead. Either BURROW_PAGES_PROJECT names a different project than BURROW_SITE \
serves, or the upload went somewhere unexpected."
