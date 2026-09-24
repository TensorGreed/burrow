#!/usr/bin/env bash
#
# `tools/check-rustdoc.sh` refuses a doc tree that documents less than it should, naming the reason.
#
# The build half is rustdoc's own `-D warnings`, and a broken intra-doc link is its business. What
# this tests is the half added by #174: the report of WHAT was documented, gated on a count
# derived from the source. Each case plants a synthetic doc tree and points the checker at it
# through `BURROW_DOC_DIR`, so no case pays for a documentation build.
#
# The derivation is broken in a COPY of the checker, beside the original, for the reason
# `CLAUDE.md` records: a copy in a temp directory resolves its own paths wrongly and exits
# non-zero for the wrong reason, which an exit-code-only assertion reports as a pass.
set -uo pipefail

cd "$(dirname "$0")/.."
original="tools/check-rustdoc.sh"
copy="tools/.check-rustdoc.undertest.sh"
tree="$(mktemp -d)"
trap 'rm -f "$copy"; rm -rf "$tree"' EXIT

pass=0
fail=0

# A doc tree with every page the checker expects today.
plant_complete() {
  rm -rf "$tree" && mkdir -p "$tree"
  for crate in burrow_types burrow_engines burrow_ops burrow_core burrow_ffi burrow_wasm; do
    mkdir -p "$tree/$crate" && : >"$tree/$crate/index.html"
  done
  for module in pdfium qpdf redact_verify; do
    mkdir -p "$tree/burrow_engines/$module" && : >"$tree/burrow_engines/$module/index.html"
  done
  : >"$tree/burrow_engines/fn.glyphs_on_first_page.html"
  : >"$tree/burrow_engines/fn.page_frame.html"
}

# `expect <name> <checker> <text> <must-pass>` -- run against the planted tree.
expect() {
  local name="$1" checker="$2" text="$3" must_pass="$4" out status
  out=$(BURROW_DOC_DIR="$tree" "$checker" 2>&1)
  status=$?
  if [ "$must_pass" = yes ]; then
    if [ $status -eq 0 ] && grep -qF "$text" <<<"$out"; then
      echo "  ok   $name"
      pass=$((pass + 1))
    else
      echo "  FAIL $name: expected a pass reporting \"$text\""
      sed 's/^/        /' <<<"$out" | tail -4
      fail=$((fail + 1))
    fi
  elif [ $status -eq 0 ]; then
    echo "  FAIL $name: the checker accepted it"
    fail=$((fail + 1))
  elif grep -qF "$text" <<<"$out"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason ($text)"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
  fi
}

# The baseline: a complete tree passes, and says how much it examined.
plant_complete
expect "a complete tree passes and reports 6 of 6 crates, 5 of 5 gated items" "$original" \
  "6 of 6 workspace crate(s) documented; 5 of 5 public engine-gated" yes

# THE #174 CASE: what `cargo doc` without --all-features writes -- every crate, and none of the
# engine-gated items. The old job printed OK over exactly this.
plant_complete
rm -rf "$tree/burrow_engines/pdfium" "$tree/burrow_engines/qpdf" "$tree/burrow_engines/redact_verify" \
  "$tree/burrow_engines/fn.glyphs_on_first_page.html" "$tree/burrow_engines/fn.page_frame.html"
expect "a tree documented without the engines is refused, naming what is missing" "$original" \
  "mod redact_verify" no

# One gated FUNCTION missing, so the fn half of the page mapping is exercised on its own.
plant_complete
rm -f "$tree/burrow_engines/fn.page_frame.html"
expect "one missing gated function is refused by name" "$original" "fn page_frame" no

# A crate missing.
plant_complete
rm -rf "$tree/burrow_ops"
expect "a missing crate page is refused by name" "$original" "crate(s) with no doc page: burrow_ops" no

# THE DERIVATION, BROKEN IN A COPY: a gate pattern that matches nothing derives no items, and a
# check with nothing to check must not read as a pass.
python3 - "$original" "$copy" <<'PY'
import sys
src, dst = sys.argv[1:3]
text = open(src).read()
old = 'feature\\s*=\\s*"native-engines"'
assert old in text, f"the mutation target is not in {src}"
open(dst, "w").write(text.replace(old, 'feature\\s*=\\s*"no-such-feature"', 1))
PY
if [ $? -ne 0 ]; then
  echo "  FAIL the derivation mutation did not apply, so its case would prove nothing"
  fail=$((fail + 1))
else
  chmod +x "$copy"
  plant_complete
  expect "a gate pattern that matches nothing is refused, not passed" "$copy" \
    "no public engine-gated item was derived" no
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed"
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
