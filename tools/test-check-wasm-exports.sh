#!/usr/bin/env bash
# Adversarial self-test for tools/check-wasm-exports.sh.
#
# The checker decides which qpdf C functions JavaScript may call. Its rules are only
# observable when one of them fires, so a rule that stopped working would leave every run
# green -- which is how `qpdf_remove_page` came to sit on main undetected in the first place,
# by a different mechanism.
#
# Two kinds of case here:
#
#   * FIXTURE cases plant a defect in the INPUTS and require the real checker to refuse,
#     naming the reason. One per rule.
#   * A GATE case breaks a rule in a COPY of the checker and requires the copy to refuse.
#     The copy sits beside the original, because the tool resolves the repository from its
#     own location -- a copy in /tmp exits non-zero with "not found", which an exit-code-only
#     assertion reports as a pass (CLAUDE.md).
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
tool="$here/check-wasm-exports.sh"
wasm="$repo/engines/vendor/wasm/lib/qpdf.wasm"
ffi_rs="$repo/core/burrow-engines/src/qpdf/ffi.rs"
not_exported="$repo/engines/qpdf-not-exported.toml"

pass=0
fail=0
work="$(mktemp -d)"
trap 'rm -rf "$work"; rm -f "$here/.wasm-exports-fixture.sh"' EXIT

[ -f "$wasm" ] || {
  # NOT SKIPPED SILENTLY. A self-test that quietly does nothing when its input is missing is
  # the failure this whole file is about.
  echo "FAILED — $wasm is missing; run engines/build-wasm.sh first" >&2
  exit 1
}

# The happy path first: without it, every case below could be failing for the wrong reason.
if out="$("$tool" "$wasm" "$ffi_rs" "$not_exported" 2>&1)"; then
  echo "  ok   the real inputs pass"
  pass=$((pass + 1))
else
  echo "  FAIL the real inputs do not pass, so no case below means anything:"
  sed 's/^/        /' <<<"$out"
  exit 1
fi

# A planted defect in the inputs must be refused, for the stated reason.
plants_it() {
  local name="$1" ffi="$2" toml="$3" expect="$4"
  local got
  if got="$("$tool" "$wasm" "$ffi" "$toml" 2>&1)"; then
    echo "  FAIL $name: the checker accepted it"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$got"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
}

# RULE 1: declared natively, not exported, not argued. The original rule, and the one that
# caught rotate's six and split's one.
cp "$ffi_rs" "$work/ffi-undeclared.rs"
printf '    pub(super) fn qpdf_oh_get_array_item(\n' >>"$work/ffi-undeclared.rs"
plants_it "a new native declaration that is neither exported nor argued is caught" \
  "$work/ffi-undeclared.rs" "$not_exported" \
  "DECLARED NATIVELY BUT NOT EXPORTED"

# RULE 2: recorded as deliberately absent, but actually exported. `qpdf_read_memory` is
# exported and declared, so recording it here is a warning about a hazard that is not there.
{
  cat "$not_exported"
  printf '\n[[function]]\nname = "qpdf_read_memory"\noperation = "test"\nreason = """A deliberately wrong entry: this function IS exported."""\n'
} >"$work/toml-wrongly-present.toml"
plants_it "an entry for a function that IS exported is caught" \
  "$ffi_rs" "$work/toml-wrongly-present.toml" \
  "RECORDED AS DELIBERATELY NOT EXPORTED, BUT IS EXPORTED"

# RULE 3: an entry for a function ffi.rs does not declare -- dead weight that reads as coverage.
{
  cat "$not_exported"
  printf '\n[[function]]\nname = "qpdf_oh_get_array_n_items"\noperation = "test"\nreason = """A deliberately stale entry: nothing declares this natively."""\n'
} >"$work/toml-stale.toml"
plants_it "an entry for a function nobody declares is caught" \
  "$ffi_rs" "$work/toml-stale.toml" \
  "names functions ffi.rs does not declare"

# RULE 4: an entry with a name and no argument. An absence with no reason is an exemption with
# no owner.
#
# Built by DELETING a reason from the real file rather than by appending a new entry: a second
# entry for a function already listed is deduplicated by `sort -u`, so the entry count and the
# name count disagree and rule 5 fires first. The first version of this case did exactly that
# and "passed" against the wrong message.
python3 - "$not_exported" "$work/toml-no-reason.toml" <<'PYEOF'
import re, sys
text = open(sys.argv[1]).read()
# Drop the LAST entry's reason block, leaving its name and operation. The entry count and the
# name count still agree, so rule 5 stays quiet and rule 4 is the only one that can fire.
head, sep, last = text.rpartition("[[function]]")
assert sep, "the fixture file has no entries"
stripped = re.sub(r'reason = """.*?"""\n', "", last, flags=re.S)
assert stripped != last, "the reason block was not removed, so this case would prove nothing"
open(sys.argv[2], "w").write(head + sep + stripped)
PYEOF
plants_it "an entry with no reason is caught" \
  "$ffi_rs" "$work/toml-no-reason.toml" \
  "has no reason"

# RULE 5: a name the entry parser cannot read. The count check is what makes a malformed entry
# a failure rather than an absence nobody is checking.
{
  cat "$not_exported"
  printf '\n[[function]]\nname= "qpdf_remove_page"\nreason = """No space before the equals, so the name parser misses it."""\n'
} >"$work/toml-unparsed.toml"
plants_it "an entry whose name cannot be parsed is caught" \
  "$ffi_rs" "$work/toml-unparsed.toml" \
  "parsed names"

# AND THE GATE ITSELF. Break the ffi.rs parse in a copy and require the copy to refuse.
#
# It is caught by the parse PROBES rather than by the zero-parse floor, and that is the better
# of the two answers: the probes name which fixture stopped matching, while the floor can only
# say the result was empty. This case asserted the floor's message first and failed -- the
# tool was right and the expectation was wrong.
copy="$here/.wasm-exports-fixture.sh"
sed 's|pub(super) fn \\(qpdf\[A-Za-z_0-9\]\*\\)|pub(super) fn \\(qpdfZZZ[A-Za-z_0-9]*\\)|' "$tool" >"$copy"
chmod +x "$copy"
if cmp -s "$copy" "$tool"; then
  echo "  FAIL the parse mutation did not apply, so this case would prove nothing"
  fail=$((fail + 1))
else
  if got="$("$copy" "$wasm" "$ffi_rs" "$not_exported" 2>&1)"; then
    echo "  FAIL a checker whose ffi.rs parse sees nothing ran anyway"
    fail=$((fail + 1))
  elif grep -qF "does not match its own fixture" <<<"$got"; then
    echo "  ok   a broken ffi.rs parse is caught by the parse probes, naming the fixture"
    pass=$((pass + 1))
  else
    echo "  FAIL the broken copy failed, but not via the parse probes"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
