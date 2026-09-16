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
# AND WHY THERE IS A THIRD ARGUMENT NOW. A CUSTOM DOMAIN SPLITS THE QUESTION AGAIN.
#
# With `BURROW_SITE=https://notonlypdf.com`, wrangler still prints an alias on the PROJECT's
# `pages.dev` host -- `https://f55a5097.burrow-f2s.pages.dev` -- because that is where Cloudflare
# puts deployments. The custom domain is a routing fact layered on top, and wrangler's output
# does not mention it. So the alias can no longer be compared against the site origin: with a
# custom domain there is NO correct deploy that satisfies that comparison, which is exactly the
# defect this file already records once, in a new costume.
#
# The three facts were two and are now three, and they stay separate because they are separately
# true:
#
#   * `BURROW_SITE`          -- a BUILD input. Baked into the CSP and the worker's absolute URLs.
#   * `BURROW_PAGES_PROJECT` -- a DEPLOY input. Which project receives the bytes.
#   * `BURROW_PAGES_HOST`    -- a fact ABOUT that project. Where its deployments are addressable,
#                               and therefore the only thing wrangler's alias can be checked
#                               against.
#
# Omit the third argument and the site origin is used as the base, which is the no-custom-domain
# case and the behaviour every existing caller had.
#
# WHAT THIS CHECK NO LONGER ANSWERS, said plainly rather than implied: with a custom domain it
# confirms the upload reached the right PROJECT, and it cannot confirm that the project serves
# the custom domain. Nothing in wrangler's output can. The step after it asks the live origin
# for its headers, and that is the only thing that establishes the domain is actually wired up.
#
# Usage: tools/check-deployment-url.sh <reported-url> <expected-origin> [project-host]
# Self-test: tools/test-check-deployment-url.sh

set -euo pipefail

reported="${1:-}"
expected="${2:-}"
project_host="${3:-}"

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

# THE BASE THE ALIAS IS JUDGED AGAINST. The project host when one is given -- because with a
# custom domain the alias lives there and not on the site origin -- and the site origin
# otherwise, which is the case where they are the same host.
base="$want"
if [ -n "$project_host" ]; then
  case "$project_host" in
    https://*) fail "the project host is a HOST, not a URL; got $project_host" ;;
    *://*) fail "the project host is a HOST, not a URL; got $project_host" ;;
    */*) fail "the project host must carry no path; got $project_host" ;;
  esac

  # AND IT MUST BE A PROJECT'S HOST, NOT THE ZONE ITSELF. This value became the whole of the
  # comparison the moment it was supplied -- the site origin is no longer related to the
  # deployment by this check at all -- so a base that is too SHORT silently widens what counts
  # as "ours". Security review measured it: `pages.dev` was accepted, and then
  # `https://anything.pages.dev` is "an alias of pages.dev", which is every Cloudflare Pages
  # project in the world. Nobody but someone who can edit deploy.yml on main can set it, so
  # this is an unvalidated trust anchor rather than a reachable attack -- and validating it
  # costs two lines.
  case "$project_host" in
    *.pages.dev) : ;;
    *) fail "the project host must be a <project>.pages.dev name; got $project_host. \
`pages.dev` alone is the zone, under which every Pages project in the world would be ours" ;;
  esac
  # AND A NON-EMPTY PROJECT LABEL. `.pages.dev` matches the pattern above with an empty first
  # label, and would then accept `https://anything.pages.dev` as "one label in front of it".
  [ -n "${project_host%%.*}" ] ||
    fail "the project host has an empty first label: $project_host"
  base="$project_host"
fi

# THE WHOLE HOST, or the whole host with exactly one label in front of it. `${host#*.}` strips
# up to and including the FIRST dot, which is a label boundary -- so `f55a5097.burrow-f2s.pages.dev`
# yields `burrow-f2s.pages.dev` and matches, while `evil-burrow-f2s.pages.dev` yields
# `pages.dev` and does not. That is the difference between splitting on structure and matching
# on characters.
if [ "$host" = "$base" ]; then
  echo "deployment URL is the project host itself: $reported"
  exit 0
fi

rest="${host#*.}"
first="${host%%.*}"
if [ "$rest" = "$base" ] && [ -n "$first" ] && [ "$rest" != "$host" ]; then
  echo "deployment URL is an alias of $base: $reported"
  exit 0
fi

fail "the deploy reported $reported, which is not $base nor a deployment alias of it. A build \
is bound to one origin (ADR 0014 §4), so on another host connect-src refuses every engine fetch \
and every tool is dead. Either BURROW_PAGES_PROJECT names a different project than expected, \
BURROW_PAGES_HOST names the wrong host for it, or the upload went somewhere unexpected."
