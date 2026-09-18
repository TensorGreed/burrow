#!/usr/bin/env bash
# Test that make-sbom.py --check actually fails, and fails for the right reason.
#
# An SBOM that is generated and never verified is a supply-chain document with the same shape
# as a check that examines nothing -- and this repository has measured four of those. So the
# verifier gets a test, and the test asserts WHICH rule fired rather than only that something
# did: a tool that failed for an unrelated reason would otherwise read as coverage.
#
# THREE THINGS THIS FILE LEARNED FROM ITS OWN FIRST VERSION, each measured by a review:
#   * It asserted no exit status. CI gates on the tool's exit code; every case here grepped
#     stdout and swallowed the status with `|| true`, so a tool that printed its problems and
#     returned 0 passed all five cases.
#   * Its most important case was a source grep. It read make-sbom.py looking for a literal
#     string, which cannot tell whether the branch is reachable, whether its message is true, or
#     what it exits with -- and it survived an always-succeed mutant.
#   * It skipped a case when engines/vendor was absent and still printed "4 passed, 0 failed".
#     "4 of 15" reads as success. The count is knowable here, so the run is gated on it.
#
# Every case redirects the tool's inputs with BURROW_ENGINE_LICENSES, BURROW_SBOM and
# BURROW_ENGINE_VENDOR. The committed manifest and SBOM are never modified -- a self-test that
# edits them is one that leaves the repository broken whenever it fails part way.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
tool="$here/make-sbom.py"
# The mutant lives BESIDE the original, not in a temp directory: both tools resolve REPO from
# their own __file__, so a copy anywhere else resolves every path wrongly and exits non-zero for
# a reason that has nothing to do with the rule under test.
mutant="$here/make-sbom.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

#: Every case below must run. A skipped case is the failure this file exists to prevent.
EXPECTED_CASES=15

pass=0
fail=0

ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

# Run a checker with redirected inputs, capturing BOTH the output and the status. The status is
# the thing CI gates on, so a case that does not assert it is not testing the gate.
status=0
run_check() {
  local tool_path="$1" manifest="$2" sbom="$3"
  shift 3
  set +e
  output="$(BURROW_ENGINE_LICENSES="$manifest" BURROW_SBOM="$sbom" \
    python3 "$tool_path" --check "$@" 2>&1)"
  status=$?
  set -e
}

expect_refusal() {
  local name="$1" manifest="$2" sbom="$3" needle="$4"
  shift 4
  run_check "$tool" "$manifest" "$sbom" "$@"
  if [ "$status" -eq 0 ]; then
    bad "$name -- the tool exited 0; CI gates on the status, so this rule would not stop a push"
    sed 's/^/         /' <<<"$output" >&2
  elif grep -qF "$needle" <<<"$output"; then
    ok "$name"
  else
    bad "$name"
    echo "       expected the output to contain: $needle" >&2
    echo "       got (exit $status):" >&2
    sed 's/^/         /' <<<"$output" >&2
  fi
}

expect_pass() {
  local name="$1" manifest="$2" sbom="$3"
  shift 3
  run_check "$tool" "$manifest" "$sbom" "$@"
  if [ "$status" -eq 0 ] && grep -q "^OK" <<<"$output"; then
    ok "$name"
  else
    bad "$name -- these inputs must pass, or every case below is vacuous (exit $status)"
    sed 's/^/         /' <<<"$output" >&2
  fi
}

echo "make-sbom.py --check:"

# THE VENDOR TREE IS REQUIRED, NOT OPTIONAL. Two of the cases below exercise the source that is
# not hand-maintained, and skipping them while printing a pass count is exactly the failure this
# file is written against.
# `nm` and `strings` are what the scan IS. Without them symbol_text/string_text return "" and
# every artifact looks empty -- which case 7 would then pass for entirely the wrong reason.
for required in nm strings; do
  command -v "$required" >/dev/null || {
    echo "  FAIL $required is not on PATH; the scan would return nothing and case 7 would pass" >&2
    exit 1
  }
done

if [ ! -d "$repo/engines/vendor" ]; then
  echo "  FAIL engines/vendor is absent; cases 3 and 6 cannot run" >&2
  echo "       run engines/fetch.sh && engines/build-native.sh -- this suite does not skip" >&2
  exit 1
fi

# THE POSITIVE CONTROL, FIRST. If the real inputs do not pass, every refusal below could be
# the tool being broken rather than the rule biting.
expect_pass "the committed inputs agree" "$repo/engines/licenses.toml" "$repo/sbom/burrow.cdx.json"

#: The native artifact this machine actually has. CI builds native-x86_64, this box builds
#: native-aarch64, and the cases below name it in their needles.
native="engines/vendor/native-$(uname -m)/lib/libpdfium.so"
if [ ! -f "$repo/$native" ]; then
  echo "  FAIL $native is absent; the native gate cases below cannot run" >&2
  echo "       run engines/build-native.sh -- this suite does not skip" >&2
  exit 1
fi

# 1. DRIFT: a crate leaves the committed document.
python3 - "$repo/sbom/burrow.cdx.json" "$work/no-serde.json" <<'PY'
import json, sys
document = json.load(open(sys.argv[1], encoding="utf-8"))
before = len(document["components"])
document["components"] = [c for c in document["components"] if c["name"] != "serde"]
assert len(document["components"]) == before - 1, "serde is no longer in the SBOM; update this case"
json.dump(document, open(sys.argv[2], "w", encoding="utf-8"), indent=2)
PY
expect_refusal "a crate missing from the committed SBOM is named" \
  "$repo/engines/licenses.toml" "$work/no-serde.json" \
  "not in the committed SBOM: serde"

# 2. DRIFT THE OTHER WAY: the document lists something the build does not have.
python3 - "$repo/sbom/burrow.cdx.json" "$work/ghost.json" <<'PY'
import json, sys
document = json.load(open(sys.argv[1], encoding="utf-8"))
document["components"].append(
    {"type": "library", "name": "a-crate-that-is-not-here", "version": "9.9.9",
     "purl": "pkg:cargo/a-crate-that-is-not-here@9.9.9", "scope": "required"}
)
json.dump(document, open(sys.argv[2], "w", encoding="utf-8"), indent=2)
PY
expect_refusal "a component in the SBOM and not in the build is named" \
  "$repo/engines/licenses.toml" "$work/ghost.json" \
  "in the committed SBOM and no longer in the build: a-crate-that-is-not-here"

# 3. THE ONE THAT MATTERS: a component that is INSIDE a shipped binary and declared nowhere.
#    This is HarfBuzz's real history -- linked into PDFium through an entire ADR cycle with no
#    entry in the manifest -- reproduced against a manifest with that entry removed.
python3 - "$repo/engines/licenses.toml" "$work/no-harfbuzz.toml" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
match = re.search(
    r'\[\[component\]\]\nname = "harfbuzz \(bundled in pdfium\)"(?:.|\n)*?(?=\n\[\[component\]\]|\n# ---)',
    text,
)
if match is None:
    raise SystemExit("the harfbuzz entry moved; this case must be updated rather than skipped")
open(sys.argv[2], "w", encoding="utf-8").write(text[: match.start()] + text[match.end() :])
PY
expect_refusal "a component inside a shipped binary and declared nowhere is named" \
  "$work/no-harfbuzz.toml" "$repo/sbom/burrow.cdx.json" \
  "not declared anywhere: harfbuzz"

# 4. AN ABSENT VENDOR TREE IS A FAILURE, and this case is BEHAVIOURAL. Its predecessor grepped
#    the tool's source for the message, which could not tell whether the branch was reachable,
#    what it exited with, or -- as it turned out -- whether the message was even true.
mkdir -p "$work/empty-vendor"
BURROW_ENGINE_VENDOR="$work/empty-vendor" \
  expect_refusal "an absent vendor tree fails rather than passing quietly" \
  "$repo/engines/licenses.toml" "$repo/sbom/burrow.cdx.json" \
  "NO artifacts found under"

# 5. ...and it is a DELIBERATE opt-out, not a silent one.
BURROW_ENGINE_VENDOR="$work/empty-vendor" \
  expect_pass "--allow-no-artifacts passes, and says it is passing anyway" \
  "$repo/engines/licenses.toml" "$repo/sbom/burrow.cdx.json" --allow-no-artifacts

# 6. INERT FINGERPRINTS, WITH THE REAL VENDOR TREE PRESENT. Measured as a live hole in the first
#    version of this tool: every fingerprint disabled, the tree fully populated, and --check
#    printed "the vendor tree is absent" and exited 0 -- both halves false. The probe gate is
#    the only thing standing between "nothing is in the binary" and "the scanner broke", and
#    importing the detector skips the copy of that gate in its own main().
set +e
output="$(python3 - "$here" 2>&1 <<'PY'
import importlib.util, sys
from pathlib import Path

here = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("make_sbom", here / "make-sbom.py")
sbom = importlib.util.module_from_spec(spec)
sys.modules["make_sbom"] = sbom
spec.loader.exec_module(sbom)

detector = sbom.load_detector()
original = detector.detect
detector.detect = lambda syms, strs: {}
# ASSERT THE MUTATION APPLIED before believing anything the suite reports. A str/attr swap that
# quietly matched nothing is indistinguishable from a defence that holds.
assert detector.detect("", "") == {} and original("FT_Init_FreeType", "") != {}, \
    "the mutation did not apply; this case would have proved nothing"
sbom.load_detector = lambda: detector

manifest = sbom.read_manifest()
raise SystemExit(sbom.check(sbom.build(manifest), manifest, allow_no_artifacts=False))
PY
)"
status=$?
set -e
if [ "$status" -eq 0 ]; then
  bad "inert fingerprints are caught rather than read as an empty binary -- the tool exited 0"
  sed 's/^/         /' <<<"$output" >&2
elif ! grep -qF "artifact(s); " <<<"$output"; then
  bad "inert fingerprints are caught rather than read as an empty binary"
  echo "       nothing was scanned, so this case did not test the pairing it names:" >&2
  echo "       a broken scanner WITH a real tree, which is the hole that was measured" >&2
  sed 's/^/         /' <<<"$output" >&2
elif grep -qF "detector fingerprint:" <<<"$output"; then
  ok "inert fingerprints are caught rather than read as an empty binary"
else
  bad "inert fingerprints are caught rather than read as an empty binary"
  echo "       expected the probe gate to name the broken fingerprints; got (exit $status):" >&2
  sed 's/^/         /' <<<"$output" >&2
fi

# 7. AN ARTIFACT WITH NOTHING IN IT. The backstop behind the probe gate: if the fingerprints
#    ever pass their probes and still match nothing in a real binary, "no third-party code in a
#    PDF engine" must be a failure rather than a clean scan. A mutation that downgraded this to
#    a note survived every other case in this file, which is why it has its own.
mkdir -p "$work/hollow-vendor/native-hollow/lib"
printf 'not a real engine' > "$work/hollow-vendor/native-hollow/lib/libpdfium.so"
BURROW_ENGINE_VENDOR="$work/hollow-vendor" \
  expect_refusal "an artifact with nothing detected in it fails" \
  "$repo/engines/licenses.toml" "$repo/sbom/burrow.cdx.json" \
  "the fingerprints no longer match this artifact"

# 8. THE NATIVE `linked_in` EQUALITY GATE, both directions. A native symbol table is complete,
#    so the manifest's linked_in and the scan agree exactly -- 8 of 8 on libpdfium.so, measured
#    before this gate was written. Both halves get a case, because a gate that only fires one way
#    would pass a manifest claiming a component is inside a binary it is not in.
python3 - "$repo/engines/licenses.toml" "$work/lcms-not-linked.toml" "$work/libpng-overclaimed.toml" <<'CASE8'
import re, sys

text = open(sys.argv[1], encoding="utf-8").read()

# (a) lcms IS in libpdfium.so; drop the claim that it is.
import os

# The id for THIS machine's native artifact: CI is x86_64, a dev box may be aarch64. Dropping the
# arm64 id on an x86_64 runner would mutate a claim nothing scans, and the case would go green
# while proving nothing.
here = "pdfium-linux-x64" if os.uname().machine == "x86_64" else "pdfium-linux-arm64"

needle = 'linked_in = ["pdfium-wasm", "pdfium-linux-arm64", "pdfium-linux-x64"]\nlinked = true\nnotes = "cmsCreateTransform'
if needle not in text:
    raise SystemExit("lcms's linked_in line moved; update this case rather than skipping it")
open(sys.argv[2], "w", encoding="utf-8").write(
    text.replace(needle, needle.replace(f'"{here}", ', "").replace(f', "{here}"', ""), 1)
)

# (b) libpng is NOT in libpdfium.so -- the manifest says so at length. Claim that it is.
match = re.search(r'(name = "libpng \(bundled in pdfium\)"(?:.|\n)*?\nlinked_in = )\[\]', text)
if match is None:
    raise SystemExit("libpng's entry moved; update this case rather than skipping it")
open(sys.argv[3], "w", encoding="utf-8").write(
    text[: match.start()] + match.group(1) + f'["{here}"]' + text[match.end() :]
)
CASE8

expect_refusal "a component found in a native binary and not declared linked_in is named" \
  "$work/lcms-not-linked.toml" "$repo/sbom/burrow.cdx.json" \
  "found in $native and not declared linked_in: lcms"

expect_refusal "a component declared linked_in and absent from the native binary is named" \
  "$work/libpng-overclaimed.toml" "$repo/sbom/burrow.cdx.json" \
  "declared linked_in and NOT found in $native: libpng"

# 9. A NATIVE ARTIFACT THE MANIFEST DOES NOT NAME. The gate above is keyed on an id resolved
#    from a hand-edited `files` list, so it can be switched off by an ordinary manifest edit --
#    measured by a review, which rewrote one path and watched the tool print OK with nothing
#    gated. A gate that disables itself silently is the failure this whole file is written
#    against, so the unresolved id is a failure rather than a note for native artifacts.
python3 - "$repo/engines/licenses.toml" "$work/unnamed-native.toml" <<'CASE9'
import sys

import os

text = open(sys.argv[1], encoding="utf-8").read()

# The path THIS machine's scan will produce. Renaming the aarch64 entry on an x86_64 runner would
# leave the scanned artifact perfectly resolvable and the case would pass having proved nothing.
staged = f"vendor/native-{os.uname().machine}/lib/libpdfium.so"
if staged not in text:
    raise SystemExit(f"no [[artifact]] names {staged}; update this case, do not skip it")
open(sys.argv[2], "w", encoding="utf-8").write(
    text.replace(staged, staged.replace("libpdfium.so", "libpdfium-RENAMED.so"), 1)
)
CASE9

expect_refusal "a native artifact no [[artifact]] names fails instead of skipping the gate" \
  "$work/unnamed-native.toml" "$repo/sbom/burrow.cdx.json" \
  "is a native artifact that no [[artifact]] in the manifest names"

# 10. A LICENCE CHANGING UNDERNEATH US. The component sets match, so the drift rules above stay
#     silent and only the field-level diff fires. The comment beside that rule says a licence
#     expression flipping is the case worth spelling out; this is that case.
python3 - "$repo/sbom/burrow.cdx.json" "$work/serde-relicensed.json" <<'CASE10'
import json, sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
serde = next((c for c in document["components"] if c["name"] == "serde"), None)
if serde is None:
    raise SystemExit("serde is no longer in the SBOM; update this case rather than skipping it")
serde["licenses"] = [{"expression": "GPL-3.0-only"}]
json.dump(document, open(sys.argv[2], "w", encoding="utf-8"), indent=2)
CASE10

expect_refusal "a licence changing with the component set unchanged is named, with the field" \
  "$repo/engines/licenses.toml" "$work/serde-relicensed.json" \
  "the committed SBOM differs: serde: licenses"

# 11. THE MANIFEST DECLARES SOMETHING THE COMMITTED DOCUMENT DOES NOT CARRY. Its own rule, and
#     not redundant with drift: a generator bug that dropped a component would be committed
#     along with the bad output, after which the generated and committed documents agree and
#     drift has nothing to say. This case reaches the rule from the committed side.
python3 - "$repo/sbom/burrow.cdx.json" "$work/no-qtest.json" <<'CASE11'
import json, sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
before = len(document["components"])
document["components"] = [c for c in document["components"] if c["name"] != "qtest"]
if len(document["components"]) != before - 1:
    raise SystemExit("qtest is no longer in the SBOM; update this case rather than skipping it")
json.dump(document, open(sys.argv[2], "w", encoding="utf-8"), indent=2)
CASE11

expect_refusal "a manifest entry missing from the committed SBOM is named by its own rule" \
  "$repo/engines/licenses.toml" "$work/no-qtest.json" \
  "declared in engines/licenses.toml and missing from the committed SBOM: qtest"

# 12. THE LABEL-COLLISION CASE. `manifest_names` are substrings, and `("libjpeg", "jpeg")` used
#     to match "libopenjpeg" -- so the IJG-licensed libjpeg-turbo entry, which carries a
#     mandatory documentation credit, could be deleted from the manifest entirely while a
#     BSD-2-Clause component stood in for it and every gate stayed green. A review measured that
#     end to end. Case 8a uses lcms, which has no collision, so the suite could not see it.
python3 - "$repo/engines/licenses.toml" "$work/no-libjpeg.toml" <<'CASE12'
import re, sys

text = open(sys.argv[1], encoding="utf-8").read()
match = re.search(
    r'\[\[component\]\]\nname = "libjpeg-turbo \(bundled in pdfium\)"(?:.|\n)*?(?=\n\[\[component\]\]|\n# ---)',
    text,
)
if match is None:
    raise SystemExit("the libjpeg-turbo entry moved; this case must be updated rather than skipped")
open(sys.argv[2], "w", encoding="utf-8").write(text[: match.start()] + text[match.end() :])
CASE12

expect_refusal "a component deleted from the manifest cannot hide behind a colliding label" \
  "$work/no-libjpeg.toml" "$repo/sbom/burrow.cdx.json" \
  "found in $native and not declared linked_in: libjpeg"

# 13. THE META-TEST. Break the checker in a copy beside the original and require a case above to
#    go red against it -- otherwise the cases are asserting nothing and this file is a ritual.
python3 - "$tool" "$mutant" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = "    if problems:\n"
assert old in source, "the failure branch moved; this meta-test must be updated, not deleted"
mutated = source.replace(old, "    if False and problems:\n", 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
run_check "$mutant" "$repo/engines/licenses.toml" "$work/no-serde.json"
if [ "$status" -eq 0 ] && ! grep -qF "not in the committed SBOM: serde" <<<"$output"; then
  ok "a checker that cannot fail is caught by these cases"
else
  bad "a checker that cannot fail is caught by these cases -- the mutant still refused (exit $status)"
  echo "       the drift case is not binding the rule it names" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  echo "  A case that does not run reads exactly like a case that passed." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
