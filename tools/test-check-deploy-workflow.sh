#!/usr/bin/env bash
# Adversarial self-test for tools/check-deploy-workflow.py.
#
# That checker guards a credential that can write to the live site, so every case here is a
# mutation of the REAL workflow, fed to the real checker, required to refuse and to name the
# reason. The fixtures are copies: a self-test that edits `.github/workflows/deploy.yml` is one
# interrupted run away from committing a workflow with a `pull_request` trigger.
#
# WHAT THIS SUITE LEARNED THE HARD WAY. Its first version re-planted only spellings the
# checker already handled, so it passed 19/19 while a security review walked past the checker
# five ways -- a quoted key, a space before a colon, a sibling `tags:` filter, a
# workflow-level `env:`, and a comment read as an invocation. A self-test that only exercises
# the cases its author thought of measures the author, not the checker. Every one of those five
# is a case below, by name.
#
# AND EVERY PLANT IS VERIFIED IN THE PARSED STRUCTURE, not just written to the file. Two of
# those five appeared to pass on the first attempt because the plant inserted a SECOND
# top-level `env:` and YAML keeps the last duplicate -- so the mutation never reached the
# document the checker reads. A plant that did not apply is indistinguishable from a rule that
# works.
#
# Usage: tools/test-check-deploy-workflow.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-deploy-workflow.py"
real="$repo/.github/workflows/deploy.yml"
workroot=""

cleanup() {
  trap - EXIT INT TERM HUP
  [ -n "$workroot" ] && rm -rf "$workroot"
  rm -f "$here/.deploy-workflow-probe-fixture.py"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0

[ -f "$real" ] || {
  echo "  SKIP-REFUSED: no .github/workflows/deploy.yml to copy. Refusing rather than" >&2
  echo "                passing vacuously." >&2
  exit 1
}

workroot="$(mktemp -d)"
work="$workroot/deploy.yml"

# `mutate <name> <python>` — the python receives the file path as argv[1], edits it, and MUST
# assert its own effect on the parsed document.
mutate() {
  cp "$real" "$work"
  python3 -c "$2" "$work"
}

expect_refusal() {
  local name="$1" expect="$2" code="$3"
  if ! mutate "$name" "$code"; then
    echo "  FAIL $name: the plant did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  local out status=0
  out="$("$checker" "$work" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -2
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

expect_pass() {
  local name="$1" code="$2"
  if ! mutate "$name" "$code"; then
    echo "  FAIL $name: the plant did not apply"
    fail=$((fail + 1))
    return
  fi
  if "$checker" "$work" >/dev/null 2>&1; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: refused something it must accept"
    "$checker" "$work" 2>&1 | tail -2 | sed 's/^/        /'
    fail=$((fail + 1))
  fi
}

# THE BASELINE. Without it every case below could be passing against a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
cp "$real" "$work"
if "$checker" "$work" >/dev/null 2>&1; then
  echo "  ok   the real workflow passes, so a refusal below means something"
  pass=$((pass + 1))
else
  echo "  FAIL the real workflow does not pass, so no case below means anything"
  "$checker" "$work" 2>&1 | tail -4 | sed 's/^/        /'
  exit 1
fi

# --- the five a security review walked past, each by name -------------------------------------
expect_refusal "a QUOTED trigger key is seen (grep did not see it)" "triggers are" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("  push:\n", "  \"pull_request\":\n    branches: [main]\n  push:\n", 1)
p.write_text(s)
assert "pull_request" in yaml.safe_load(s)[True], "plant did not reach the parsed document"
'

expect_refusal "a SPACE BEFORE THE COLON is seen (valid YAML, invisible to a regex)" "triggers are" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("  push:\n", "  pull_request :\n    branches: [main]\n  push:\n", 1)
p.write_text(s)
assert "pull_request" in yaml.safe_load(s)[True], "plant did not reach the parsed document"
'

expect_refusal "a sibling tags: filter is refused (presence is not exclusivity)" "expected exactly" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("    branches: [main]\n", "    branches: [main]\n    tags: [\"**\"]\n", 1)
p.write_text(s)
assert "tags" in yaml.safe_load(s)[True]["push"], "plant did not reach the parsed document"
'

expect_refusal "a WORKFLOW-LEVEL env: secret is refused (it reaches every job)" "OUTSIDE the jobs block" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
import re
m = re.search(r"^  BURROW_SITE: .*\n", s, re.M)
assert m, "MUTATION DID NOT APPLY: no workflow-level BURROW_SITE line to anchor on"
old = m.group(0)
s = s.replace(old, old + "  CF_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}\n", 1)
p.write_text(s)
assert "CF_TOKEN" in yaml.safe_load(s)["env"], "plant did not reach the parsed document"
'

expect_refusal "the BRACKET spelling is refused, which carries no secrets. substring" "OUTSIDE the jobs block" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
import re
m = re.search(r"^  BURROW_SITE: .*\n", s, re.M)
assert m, "MUTATION DID NOT APPLY: no workflow-level BURROW_SITE line to anchor on"
old = m.group(0)
s = s.replace(old, old + "  CF_TOKEN: ${{ secrets[format(\x27{0}\x27,\x27CLOUDFLARE_API_TOKEN\x27)] }}\n", 1)
p.write_text(s)
assert "CF_TOKEN" in yaml.safe_load(s)["env"], "plant did not reach the parsed document"
'

expect_pass "a COMMENT naming the gate script is not an invocation" '
import sys, pathlib
p = pathlib.Path(sys.argv[1])
p.write_text(p.read_text().rstrip() + "\n      # see tools/check-deployable-build.sh for what this asserted\n")
'

# --- THE ALIAS ANCHOR, a trust anchor that arrived with the custom domain ----------------------
#
# Once `BURROW_PAGES_HOST` is supplied it carries the whole post-upload comparison, so a value
# that is too SHORT silently widens it rather than breaking anything.
expect_refusal "removing BURROW_PAGES_HOST is refused" "BURROW_PAGES_HOST is not set" '
import sys, pathlib, yaml, re
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s, n = re.subn(r"^  BURROW_PAGES_HOST: .*\n", "", s, count=1, flags=re.M)
assert n == 1, "MUTATION DID NOT APPLY: no BURROW_PAGES_HOST line to delete"
p.write_text(s)
assert "BURROW_PAGES_HOST" not in (yaml.safe_load(s).get("env") or {}), "plant did not apply"
'

expect_refusal "the pages.dev ZONE as the anchor is refused" "not a <project>.pages.dev name" '
import sys, pathlib, yaml, re
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s, n = re.subn(r"^  BURROW_PAGES_HOST: .*$", "  BURROW_PAGES_HOST: pages.dev", s, count=1, flags=re.M)
assert n == 1, "MUTATION DID NOT APPLY: no BURROW_PAGES_HOST line to rewrite"
p.write_text(s)
assert yaml.safe_load(s)["env"]["BURROW_PAGES_HOST"] == "pages.dev", "plant did not apply"
'

expect_refusal "an anchor nothing reads is refused as decoration" "this variable is decoration" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
old = "\"$BURROW_SITE\" \"$BURROW_PAGES_HOST\""
assert old in s, "MUTATION DID NOT APPLY: the read-back does not pass the anchor"
s = s.replace(old, "\"$BURROW_SITE\"", 1)
p.write_text(s)
runs = [st.get("run", "") for st in yaml.safe_load(s)["jobs"]["publish"]["steps"]]
assert not any("$BURROW_PAGES_HOST" in r for r in runs), "plant did not apply"
'

# --- THE PROJECT NAME, which a derivation got right for the wrong reason ------------------------
#
# `burrow-f2s.pages.dev` is the subdomain; the project is called `burrow`. Cloudflare matches
# them by convention, not by rule -- it generated `burrow-f2s` because `burrow.pages.dev` was
# taken. The first deploy derived the name from the hostname and failed with
# `The Pages project "burrow-f2s" does not exist`.
expect_refusal "removing BURROW_PAGES_PROJECT is refused" "BURROW_PAGES_PROJECT is not set" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("  BURROW_PAGES_PROJECT: burrow\n", "", 1)
p.write_text(s)
assert "BURROW_PAGES_PROJECT" not in (yaml.safe_load(s).get("env") or {}), "plant did not apply"
'

expect_refusal "a project name that is not a Pages project name is refused" "not a Cloudflare Pages project name" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("  BURROW_PAGES_PROJECT: burrow\n", "  BURROW_PAGES_PROJECT: Not A Name\n", 1)
p.write_text(s)
assert yaml.safe_load(s)["env"]["BURROW_PAGES_PROJECT"] == "Not A Name", "plant did not apply"
'

# RESTATING IT IS THE DRIFT THIS PREVENTS. The name is written once and read once; a literal in
# the wrangler command is a second statement of the same fact, and two statements are how the
# pair goes out of step. Note the near-miss below: the two values DIFFERING is correct and must
# not be refused -- that is the whole finding.
expect_refusal "restating the project name in the upload step is refused" "states the project name a second time" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace(chr(34) + "$BURROW_PAGES_PROJECT" + chr(34), "burrow", 1)
p.write_text(s)
steps = yaml.safe_load(s)["jobs"]["publish"]["steps"]
assert not any("BURROW_PAGES_PROJECT" in str(x.get("run", "")) for x in steps), "plant did not apply"
'

expect_pass "the project name and the hostname LEGITIMATELY differ, and that is not refused" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
# They already differ in the real file (`burrow` vs `burrow-f2s`). Make them differ more, so
# the case cannot pass by accident if somebody renames the project to match one day.
s = s.replace("  BURROW_PAGES_PROJECT: burrow\n", "  BURROW_PAGES_PROJECT: something-else-entirely\n", 1)
p.write_text(s)
assert yaml.safe_load(s)["env"]["BURROW_PAGES_PROJECT"] == "something-else-entirely"
'

# --- the gate the first checker could not tell existed -----------------------------------------
expect_refusal "deleting the PUBLISH job's gate is refused (the build job's copy is not it)" "never runs in the \`publish\` job" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
step = "      - name: Re-verify the payload that is actually being uploaded\n        run: tools/check-deployable-build.sh \"$BURROW_SITE\"\n"
assert step in s
p.write_text(s.replace(step, "", 1))
steps = yaml.safe_load(p.read_text())["jobs"]["publish"]["steps"]
assert not any("check-deployable-build" in str(x.get("run", "")) for x in steps), "plant did not apply"
'

# --- THE READ-BACK MUST EXIST, AND MUST FOLLOW THE UPLOAD -------------------------------------
#
# The first version of that step asserted `wrangler's URL == $BURROW_SITE`, which no correct
# production deploy can satisfy -- wrangler prints a per-deployment alias. It failed a run whose
# upload had succeeded. Deleting it entirely would leave nothing reading back where the bytes
# went, which is the failure `CLAUDE.md` names three measured times.
#
# THE STEP IS REMOVED BY LOCATING IT, not by matching its literal text: the step body contains
# a regex full of quotes and backslashes, and embedding that in a shell-quoted Python string is
# how a plant silently stops applying.
expect_refusal "deleting the post-upload read-back is refused" "never runs in the \`publish\` job" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
start = next(i for i, l in enumerate(lines) if "The deployment belongs to the project" in l)
end = next(
    i for i in range(start + 1, len(lines))
    if lines[i].startswith("      - name:") or lines[i].startswith("      - uses:")
)
p.write_text("".join(lines[:start] + lines[end:]))
steps = yaml.safe_load(p.read_text())["jobs"]["publish"]["steps"]
assert not any("check-deployment-url" in str(x.get("run", "")) for x in steps), "plant did not apply"
'

# THE SECOND REQUIRED POST-UPLOAD CHECK, which had no probe of its own. `REQUIRED_POST_UPLOAD_CHECKS`
# became a dict precisely so each entry refuses with ITS OWN reason -- and an unexercised
# message is a message nobody has read. This plants the deletion of the live-route step and
# requires the refusal to name what that check is for, not what the other one is for.
expect_refusal "deleting the live-route check is refused, naming what IT is for" \
  "check WHAT the live origin serves" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
start = next(i for i, l in enumerate(lines) if "The live origin serves the body we built" in l)
end = next(
    i for i in range(start + 1, len(lines))
    if lines[i].startswith("      - name:") or lines[i].startswith("      - uses:")
)
p.write_text("".join(lines[:start] + lines[end:]))
steps = yaml.safe_load(p.read_text())["jobs"]["publish"]["steps"]
assert not any("check-live-routes" in str(x.get("run", "")) for x in steps), "plant did not apply"
'

# --- the forbidden triggers, each named ---------------------------------------------------------
for trigger in pull_request pull_request_target workflow_call repository_dispatch issue_comment schedule; do
  expect_refusal "a \`$trigger\` trigger is refused" "triggers are" "
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace('  push:\n', '  $trigger:\n    branches: [main]\n  push:\n', 1)
p.write_text(s)
assert '$trigger' in yaml.safe_load(s)[True], 'plant did not reach the parsed document'
"
done

expect_refusal "flow style on: [push, pull_request] is refused" "triggers are" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("on:\n", "on: [push, pull_request]\nx_unused:\n", 1)
p.write_text(s)
assert set(yaml.safe_load(s)[True]) == {"push", "pull_request"}, "plant did not reach the parsed document"
'

expect_refusal "losing workflow_dispatch is refused, not silently accepted" "missing: workflow_dispatch" '
import sys, pathlib, yaml, re
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = re.sub(r"\n  workflow_dispatch:\n(?:    .*\n|\n)*?(?=  push:)", "\n", s, count=1)
p.write_text(s)
assert "workflow_dispatch" not in yaml.safe_load(s)[True], "plant did not apply"
'

# --- the rest --------------------------------------------------------------------------------------
expect_refusal "a secret in the BUILD job is refused" "expected exactly" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
old = "      - uses: ./.github/actions/build-web-payload\n"
assert old in s
s = s.replace(old, "      - name: sneak\n        env:\n          T: ${{ secrets.CLOUDFLARE_API_TOKEN }}\n        run: echo hi\n" + old, 1)
p.write_text(s)
assert any("secrets." in str(x) for x in yaml.safe_load(s)["jobs"]["build"]["steps"]), "plant did not apply"
'

expect_refusal "setting BURROW_HARNESS is refused" "SETS BURROW_HARNESS" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
import re
m = re.search(r"^  BURROW_SITE: .*\n", s, re.M)
assert m, "MUTATION DID NOT APPLY: no workflow-level BURROW_SITE line to anchor on"
old = m.group(0)
s = s.replace(old, old + "  BURROW_HARNESS: \"1\"\n", 1)
p.write_text(s)
assert "BURROW_HARNESS" in yaml.safe_load(s)["env"], "plant did not apply"
'

expect_refusal "a plain-http origin is refused, not only localhost" "must be https://" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("BURROW_SITE: https://", "BURROW_SITE: http://", 1)
p.write_text(s)
assert yaml.safe_load(s)["env"]["BURROW_SITE"].startswith("http://"), "plant did not apply"
'

expect_refusal "a development origin is refused" "development origin" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
import re
s, n = re.subn(r"^  BURROW_SITE: .*$", "  BURROW_SITE: https://localhost:4321", s, count=1, flags=re.M)
assert n == 1, "MUTATION DID NOT APPLY: no BURROW_SITE line to rewrite"
p.write_text(s)
assert "localhost" in yaml.safe_load(s)["env"]["BURROW_SITE"], "plant did not apply"
'

expect_refusal "no origin at all is refused" "BURROW_SITE is not set" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
import re
s, n = re.subn(r"^  BURROW_SITE: .*\n", "", s, count=1, flags=re.M)
assert n == 1, "MUTATION DID NOT APPLY: no BURROW_SITE line to delete"
p.write_text(s)
assert "BURROW_SITE" not in (yaml.safe_load(s).get("env") or {}), "plant did not apply"
'

expect_refusal "a workflow that never deploys is refused" "never runs \`wrangler pages deploy\`" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace("pages deploy", "pages nothing", 1)
p.write_text(s)
'

out="$("$checker" "$workroot/does-not-exist.yml" 2>&1)" || true
if grep -qF "no deploy workflow at" <<<"$out"; then
  echo "  ok   a missing workflow is refused"
  pass=$((pass + 1))
else
  echo "  FAIL a missing workflow was not refused by name"
  fail=$((fail + 1))
fi

# --- the token/credential split, planted three ways ------------------------------------------
# #115's concentration is an OIDC token in the job that holds the Cloudflare credential and runs
# `npx --yes wrangler`. The in-tool probes cover this every run; these are the adversarial
# fixtures against the REAL workflow, and the second one is the case the rule originally passed.
expect_refusal "an OIDC token in the credential job is refused" "hold BOTH" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
old = "    environment: production\n"
assert old in s
s = s.replace(old, old + "    permissions:\n      id-token: write\n", 1)
p.write_text(s)
assert yaml.safe_load(s)["jobs"]["publish"]["permissions"]["id-token"] == "write", "plant did not apply"
'

expect_refusal "an OIDC token INHERITED from workflow level is refused" "hold BOTH" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
old = "permissions:\n  contents: read\n"
assert old in s
s = s.replace(old, "permissions:\n  contents: read\n  id-token: write\n", 1)
p.write_text(s)
d = yaml.safe_load(s)
assert d["permissions"]["id-token"] == "write", "plant did not apply"
assert "permissions" not in d["jobs"]["publish"], "publish must inherit for this case to bite"
'

expect_refusal "a signing job that cannot sign is refused" "cannot sign" '
import sys, pathlib, yaml
p = pathlib.Path(sys.argv[1]); s = p.read_text()
old = "      id-token: write # cosign keyless signing via OIDC -- NOT granted to `publish`\n"
assert old in s
s = s.replace(old, "", 1)
p.write_text(s)
assert "id-token" not in yaml.safe_load(s)["jobs"]["sign"]["permissions"], "plant did not apply"
'

# --- THE PROBE GATE ---------------------------------------------------------------------------------
#
# The checker runs each rule against a counter-fixture and a near-miss on every invocation.
# That gate is only observable when a rule is inert, so this breaks one in a COPY beside the
# original and requires the copy to refuse, naming the reason. Beside, not in a temp directory:
# the script resolves REPO from its own location.
copy="$here/.deploy-workflow-probe-fixture.py"
sed 's|^ALLOWED_TRIGGERS = .*|ALLOWED_TRIGGERS = {"push", "workflow_dispatch", "pull_request"}|' \
  "$checker" >"$copy"
chmod +x "$copy"
cp "$real" "$work"
if cmp -s "$copy" "$checker"; then
  echo "  FAIL probe-gate: the mutation did not apply, so this case would prove nothing"
  fail=$((fail + 1))
elif got="$("$copy" "$work" 2>&1)"; then
  echo "  FAIL probe-gate: a rule that accepts pull_request did not stop the checker"
  fail=$((fail + 1))
elif grep -qF "probe: 'the trigger allowlist'" <<<"$got"; then
  # EITHER HALF COUNTS, and the message says which. Widening the allowlist to admit
  # `pull_request` makes the counter-fixture acceptable AND the near-miss unacceptable, so the
  # probe table can catch it from either side; what matters is that it names the inert rule.
  # Asserting only on the counter-fixture wording failed here for the right reason, which is
  # the probe gate working on its own test.
  echo "  ok   probe-gate: an inert rule stops the checker, naming which rule"
  pass=$((pass + 1))
else
  echo "  FAIL probe-gate: it failed, but not for the stated reason"
  sed 's/^/        /' <<<"$got" | head -3
  fail=$((fail + 1))
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
