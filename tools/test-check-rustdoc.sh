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

# THE DERIVATION'S OWN RULES, each on a synthetic `lib.rs`. A code review found that the only
# derivation gate -- attributes equal public plus private -- held by construction, and that a
# swapped predicate order, an unjoined wrapped attribute and a public `struct` all under-counted
# silently. Each case appends one gated item to the real `lib.rs`, with no page for it, so a
# refusal naming the item is the proof it was derived.
lib_case() {
  local name="$1" item="$2" text="$3" must_pass="$4" synthetic="$tree.lib.rs"
  cp core/burrow-engines/src/lib.rs "$synthetic"
  printf '\n%s\n' "$item" >>"$synthetic"
  plant_complete
  local out status
  out=$(BURROW_DOC_DIR="$tree" BURROW_LIB_RS="$synthetic" "$original" 2>&1)
  status=$?
  rm -f "$synthetic"
  if [ "$must_pass" = yes ] && [ $status -eq 0 ]; then
    echo "  ok   $name"; pass=$((pass + 1))
  elif [ "$must_pass" = no ] && [ $status -ne 0 ] && grep -qF "$text" <<<"$out"; then
    echo "  ok   $name"; pass=$((pass + 1))
  else
    echo "  FAIL $name (exit $status, wanted \"$text\")"
    sed 's/^/        /' <<<"$out" | tail -3
    fail=$((fail + 1))
  fi
}
lib_case "a rustfmt-wrapped gate attribute is joined and its item derived" \
  $'#[cfg(all(\n    feature = "native-engines",\n    burrow_native_engines\n))]\npub mod wrapped;' \
  "mod wrapped" no
lib_case "the gate is matched whatever order it names its parts in" \
  $'#[cfg(all(burrow_native_engines, feature = "native-engines"))]\npub fn swapped() {}' \
  "fn swapped" no
lib_case "a public gated struct is checked, not counted as private" \
  $'#[cfg(all(feature = "native-engines", burrow_native_engines))]\npub struct Gated;' \
  "struct Gated" no
lib_case "a public gated item of a kind with no page mapping is refused, not passed" \
  $'#[cfg(all(feature = "native-engines", burrow_native_engines))]\npub use qpdf::Qpdf as Aliased;' \
  "cannot map to a doc page" no
lib_case "near-miss: a gated pub(crate) item is private and needs no page" \
  $'#[cfg(all(feature = "native-engines", burrow_native_engines))]\npub(crate) mod hidden;' \
  "" yes

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

# THE BUILD LINE ITSELF. Every case above sets `BURROW_DOC_DIR` and skips the build, so a review
# deleted `-D warnings` from it and this whole self-test stayed green -- while an unresolved
# intra-doc link, the thing #174 exists for, is a warning without it: `cargo doc` exits 0. The
# rule: the one line that runs `cargo doc` carries `-D warnings`, `--all-features` and
# `--workspace`. Checked on the real checker, and refused on a copy with the flag removed.
build_line_ok() {
  python3 - "$1" <<'PY'
import sys
lines = [l for l in open(sys.argv[1]).read().splitlines()
         if "cargo doc" in l and not l.lstrip().startswith("#")]
wanted = ("-D warnings", "--all-features", "--workspace")
ok = len(lines) == 1 and all(flag in lines[0] for flag in wanted)
print(f"{len(lines)} build line(s): {lines}")
sys.exit(0 if ok else 1)
PY
}
if build_line_ok "$original" >/dev/null; then
  echo "  ok   the real build line runs rustdoc with -D warnings, --all-features and --workspace"
  pass=$((pass + 1))
else
  echo "  FAIL the real build line is missing a flag the gate depends on"
  build_line_ok "$original"
  fail=$((fail + 1))
fi
python3 - "$original" "$copy" <<'PY'
import sys
src, dst = sys.argv[1:3]
text = open(src).read()
old = 'RUSTDOCFLAGS="-D warnings" cargo doc'
assert old in text, "the build line is not spelled as this test expects"
open(dst, "w").write(text.replace(old, "cargo doc", 1))
PY
if [ $? -ne 0 ]; then
  echo "  FAIL the -D warnings mutation did not apply, so its case would prove nothing"
  fail=$((fail + 1))
elif build_line_ok "$copy" >/dev/null; then
  echo "  FAIL a build line without -D warnings was accepted"
  fail=$((fail + 1))
else
  echo "  ok   a build line without -D warnings is refused"
  pass=$((pass + 1))
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed"
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
