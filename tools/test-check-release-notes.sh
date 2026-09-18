#!/usr/bin/env bash
# Test that check-release-notes.py refuses a published body that lost what matters -- and that
# its own probe gate refuses a rule that has been made inert.
#
# The gate this guards is the one the user asked for by name: the release notes must carry the
# exact `cosign verify-blob` invocation, and the run must FAIL if the published notes do not.
# A checker whose rules all match everything passes everything, so the rules are probed on every
# invocation and the probe gate itself is tested here, in a copy beside the original.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
mutant="$here/check-release-notes.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

#: DERIVED FROM THE TOOL, not typed here. A renamed rule was already caught (the lookup raises),
#: but an ADDED one was not: a review added a sixth rule to a scratch copy and this suite passed
#: 8 of 8, because the number it gated on was a constant. One case per rule, plus the positive
#: control and the two meta-cases.
mapfile -t RULES < <(python3 "$here/check-release-notes.py" --list-rules)
EXPECTED_CASES=$(( ${#RULES[@]} + 3 ))
if [ "${#RULES[@]}" -lt 5 ]; then
  echo "  FAIL the tool reports only ${#RULES[@]} rule(s); it had 6 when this was written" >&2
  exit 1
fi

pass=0
fail=0
archive="burrow-web-deploy-2026-01-01-abc1234.tar.gz"
tag="deploy-2026-01-01-abc1234"

ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

python3 "$here/check-release-notes.py" --write "$work/notes.md" --archive "$archive" --tag "$tag" \
  >/dev/null

run_check() {
  set +e
  output="$(python3 "${2:-$here/check-release-notes.py}" --check "$1" --archive "$archive" \
    --tag "$tag" 2>&1)"
  status=$?
  set -e
}

# THE POSITIVE CONTROL, FIRST.
run_check "$work/notes.md"
if [ "$status" -eq 0 ]; then
  ok "the generated notes pass"
else
  bad "the generated notes pass -- they must, or every case below is vacuous"
  sed 's/^/         /' <<<"$output" >&2
fi

# ONE CASE PER RULE. Each removes exactly that rule's text from a published body and requires a
# refusal NAMING it: a checker that failed for an unrelated reason would otherwise read as
# coverage. The needles come from the tool's own rule names, so a renamed rule fails here rather
# than silently losing its case.
for rule in "${RULES[@]}"; do
  python3 - "$here" "$work/notes.md" "$work/dropped.md" "$rule" "$archive" "$tag" <<'PY'
import importlib.util, sys
from pathlib import Path

here, source, target, rule, archive, tag = sys.argv[1:7]
spec = importlib.util.spec_from_file_location("crn", Path(here) / "check-release-notes.py")
mod = importlib.util.module_from_spec(spec)
sys.modules["crn"] = mod
spec.loader.exec_module(mod)

rules = dict(mod.rules(archive))
rules.update([mod.tag_rule(tag)])
if rule not in rules:
    raise SystemExit(f"the tool lists {rule!r} but no rule of that name exists; the list and the "
                     f"rules have diverged")
text = rules[rule]
body = Path(source).read_text(encoding="utf-8")
if text not in body:
    raise SystemExit(f"the generated notes do not contain {rule!r}; this case cannot run")
Path(target).write_text(body.replace(text, "", 1), encoding="utf-8")
PY
  run_check "$work/dropped.md"
  if [ "$status" -ne 0 ] && grep -qF "$rule" <<<"$output"; then
    ok "a body missing $rule is refused, and named"
  else
    bad "a body missing $rule is refused, and named (exit $status)"
    sed 's/^/         /' <<<"$output" >&2
  fi
done

# THE META-TEST. Break a rule in a COPY beside the original -- beside, because the tool resolves
# its own paths from __file__ and a copy in a temp directory would fail for the wrong reason --
# and require the probe gate to refuse, naming it.
python3 - "$here/check-release-notes.py" "$mutant" <<'PY'
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = '("the OIDC issuer", ISSUER),'
assert old in source, "the rule list moved; this meta-test must be updated, not deleted"
mutated = source.replace(old, '("the OIDC issuer", ""),', 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
set +e
probe_output="$(python3 "$mutant" --probe 2>&1)"
probe_status=$?
set -e
if [ "$probe_status" -ne 0 ] && grep -qF "the OIDC issuer" <<<"$probe_output"; then
  ok "the probe gate refuses an inert rule, and names it"
else
  bad "the probe gate refuses an inert rule, and names it (exit $probe_status)"
  sed 's/^/         /' <<<"$probe_output" >&2
fi

# ...and the mutant must fail for THAT reason rather than by crashing: an exit-code-only
# assertion would pass on a copy that could not run at all.
if grep -qF "rule(s) probed" <<<"$probe_output"; then
  ok "the mutant refused as a checker, not as a traceback"
else
  bad "the mutant refused as a checker, not as a traceback"
  sed 's/^/         /' <<<"$probe_output" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
