#!/usr/bin/env bash
# Adversarial self-test for tools/check-no-error-text-in-deploy.sh (#285, DECISIONS.md rule 13).
#
# Each case plants one defect and requires the check to refuse it BY NAME -- a refusal for any
# other reason (a missing dist, an unresolvable graph) would read as a pass if only the exit code
# were checked. The feature case is the owner's: turn `fuzzing` on in the deploy build and the
# check must go red. It edits `bindings/burrow-wasm/Cargo.toml` in place, because the deploy graph
# is the workspace's -- a copy elsewhere would resolve a different graph -- and restores it and
# `Cargo.lock` byte for byte, asserting the restore.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
check="$here/check-no-error-text-in-deploy.sh"
dist="$repo/apps/web/dist"
work="$(mktemp -d)"
manifest="$repo/bindings/burrow-wasm/Cargo.toml"
ffi_manifest="$repo/bindings/burrow-ffi/Cargo.toml"
lock="$repo/Cargo.lock"
cp "$manifest" "$work/Cargo.toml.orig"
cp "$ffi_manifest" "$work/ffi-Cargo.toml.orig"
cp "$lock" "$work/Cargo.lock.orig"
restore() {
  cp "$work/Cargo.toml.orig" "$manifest"
  cp "$work/ffi-Cargo.toml.orig" "$ffi_manifest"
  cp "$work/Cargo.lock.orig" "$lock"
  rm -rf "$work"
}
trap restore EXIT

EXPECTED_CASES=6
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

# Runs the check, requiring exit $2 and $3 in its output.
expect() {
  local name="$1" status_wanted="$2" needle="$3"
  shift 3
  set +e
  out="$("$check" "$@" 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq "$status_wanted" ] && grep -qF -- "$needle" <<<"$out"; then
    ok "$name"
  else
    bad "$name (exit $status, wanted $status_wanted and: $needle)"
    sed 's/^/         /' <<<"$out" >&2
  fi
}

[ -d "$dist/engines" ] || { echo "no $dist/engines -- build the web app first" >&2; exit 1; }

echo "check-no-error-text-in-deploy.sh:"

# 1. The real build passes, and says what it examined.
expect "the deploy build as it is passes" 0 "5 of 5 shipped graph(s) checked for 'fuzzing' (positive control: 1)" "$dist"

# 2. A shipped artifact carrying an error-text name is refused, naming the file and the name.
cp -r "$dist" "$work/dist-planted"
victim="$(ls "$work/dist-planted/engines/"burrow_wasm_bg.*.wasm)"
printf 'qpdf_get_error_message_detail' >> "$victim"
expect "an artifact carrying qpdf_get_error_message_detail is refused by name" 1 \
  "carries 'qpdf_get_error_message_detail'" "$work/dist-planted"

# 3. A build missing one production module is refused: a scan of fewer files is not the build's.
cp -r "$dist" "$work/dist-short"
rm "$work/dist-short/engines/"pdfium.*.wasm
expect "a build missing a production module is refused" 1 "expected exactly one pdfium.*.wasm" \
  "$work/dist-short"

# 4. A SCAN THAT WENT BLIND: qpdf.wasm with its exported `qpdf_get_error_code` unreadable -- as a
# compressed artifact would be -- must be refused for blindness, not passed as clean.
cp -r "$dist" "$work/dist-blind"
blind="$(ls "$work/dist-blind/engines/"qpdf.*.wasm)"
python3 - "$blind" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1]); b = p.read_bytes()
assert b"qpdf_get_error_code" in b, "the control name is not in qpdf.wasm; update this case"
p.write_bytes(b.replace(b"qpdf_get_error_code", b"qpdf_get_errox_code"))
PY
if grep -aqF qpdf_get_error_code "$blind"; then
  bad "the blinding did not apply, so this case measured nothing"
else
  expect "a scan that cannot see qpdf's exported error code is refused as blind" 1 \
    "it is blind, so a clean scan means nothing" "$work/dist-blind"
fi

# 5. `fuzzing` switched on in the mobile graph: burrow-ffi, as the day a native app links qpdf.
python3 - "$ffi_manifest" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1]); s = p.read_text()
anchor = "[dependencies]\nburrow-core.workspace = true\n"
assert s.count(anchor) == 1, "burrow-ffi's dependency table moved; update this case"
s = s.replace(anchor, anchor + 'burrow-engines = { workspace = true, features = ["fuzzing"] }\n', 1)
p.write_text(s)
PY
if cmp -s "$ffi_manifest" "$work/ffi-Cargo.toml.orig"; then
  bad "the burrow-ffi mutation did not apply, so this case measured nothing"
else
  expect "fuzzing switched on in burrow-ffi's graph is refused by name" 1 \
    "burrow-ffi [aarch64-linux-android] enables burrow-engines' 'fuzzing' feature" "$dist"
fi
cp "$work/ffi-Cargo.toml.orig" "$ffi_manifest"
cp "$work/Cargo.lock.orig" "$lock"

# 6. THE OWNER'S MUTATION: `fuzzing` switched on in the deploy build's graph.
python3 - "$manifest" <<'PY'
import sys
from pathlib import Path
p = Path(sys.argv[1]); s = p.read_text()
anchor = "[dependencies]\n"
assert s.count(anchor) == 1, "the dependency table moved; update this case"
s = s.replace(anchor, anchor + 'burrow-engines = { workspace = true, features = ["fuzzing"] }\n', 1)
p.write_text(s)
PY
if cmp -s "$manifest" "$work/Cargo.toml.orig"; then
  bad "the fuzzing mutation did not apply, so this case measured nothing"
else
  expect "fuzzing switched on in the deploy build is refused by name" 1 \
    "enables burrow-engines' 'fuzzing' feature" "$dist"
fi
restore
trap - EXIT
git -C "$repo" diff --quiet -- bindings/burrow-wasm/Cargo.toml bindings/burrow-ffi/Cargo.toml Cargo.lock \
  || { echo "FAILED -- the mutation was not restored" >&2; exit 1; }

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ]
