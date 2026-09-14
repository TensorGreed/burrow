#!/usr/bin/env bash
# Adversarial self-test for tools/check-handle-identity.py.
#
# The first case is THE ACTUAL BUG, re-planted: `reorder`'s comparison of two raw handles,
# which made every permutation fail with `qpdf_e_pages`. A rule written from a defect that
# cannot be shown to catch that defect is a rule nobody has tested.
#
# Every case asserts BOTH the exit status and that the message names the right thing. Exit
# status alone would let the checker fail for an unrelated reason and still look correct --
# which is how a checker in this repository once passed by resolving its own paths wrongly.
#
# Usage: tools/test-check-handle-identity.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-handle-identity.py"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

# `name expect_status expect_text <<rust source>>`
check() {
  local name="$1" expect_status="$2" expect_text="$3" source="$4"
  local file="$work/case.rs" out status
  printf '%s\n' "$source" >"$file"
  set +e
  out="$(python3 "$checker" "$file" 2>&1)"
  status=$?
  set -e

  if [ "$status" -ne "$expect_status" ]; then
    echo "  FAIL $name: expected exit $expect_status, got $status"
    sed 's/^/        /' <<<"$out"
    fail=$((fail + 1))
    return
  fi
  if [ -n "$expect_text" ] && ! grep -qF "$expect_text" <<<"$out"; then
    echo "  FAIL $name: exit status was right but the message did not mention:"
    echo "        $expect_text"
    sed 's/^/        /' <<<"$out"
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# --- Case 1: THE BUG, exactly as it was written ------------------------------------------
check "reorder's original handle comparison is refused" 1 "[compared]" \
'fn permute() {
    if current.raw() == wanted_page.raw() {
        continue;
    }
}'

check "the correction passes" 0 "no handle compared as an identity" \
'fn permute() {
    if current.object() == wanted_page.object() {
        continue;
    }
}'

# --- Case 2: the same mistake, spelled every other way -----------------------------------
check "an inequality is refused too" 1 "[compared]" \
'fn f() { if a.raw() != b.raw() { g(); } }'

check "a comparison with the handle on the RIGHT is refused" 1 "[compared]" \
'fn f() { if wanted == page.raw() { g(); } }'

check "the struct field, not only the accessor, is refused" 1 "[compared]" \
'fn f(&self) { if self.handle == other.handle { g(); } }'

check "a freshly issued handle compared in place is refused" 1 "[compared]" \
'fn f() { if unsafe { ffi::qpdf_get_page_n(d, 3) } == held { g(); } }'

# --- Case 3: searching, which is equality wearing a different hat ------------------------
#
# `split`'s pruning and redaction both decide membership. A `contains` on raw handles is
# always false, which prunes NOTHING -- a leak that produces a valid PDF.
check "a raw handle used as a search needle is refused" 1 "[searched]" \
'fn f() { if kept.contains(&page.raw()) { g(); } }'

check "a position search on a raw handle is refused" 1 "[searched]" \
'fn f() { let at = pages.iter().position(|p| p.raw() == wanted); }'

check "searching on .object() passes" 0 "no handle compared as an identity" \
'fn f() { if kept.contains(&page.object()) { g(); } }'

# --- Case 4: collecting, where every element is distinct by construction ------------------
check "inserting a raw handle into a set is refused" 1 "[collected]" \
'fn f() { kept.insert(page.raw()); }'

check "sorting by a raw handle is refused" 1 "[collected]" \
'fn f() { pages.sort_by_key(|p| p.raw()); }'

# --- Case 5: pattern matching ------------------------------------------------------------
check "matching on a raw handle is refused" 1 "[matched]" \
'fn f() { let b = matches!(page.raw(), other); }'

# --- Case 5b: THE ONE-LET REFACTOR OF THE ORIGINAL DEFECT ---------------------------------
#
# Security review's finding: `let a = x.raw(); let b = y.raw(); if a == b` is the exact bug the
# checker was written from, with one binding in front of it, and the first version of every
# rule above missed it. A regex cannot follow a value across lines, so the rule is on the
# BINDING instead -- `ObjectHandle::raw` is documented as handing the value to one call and
# never storing it, so binding it is already against the rule.
check "binding a raw handle to a local is refused" 1 "[bound]" \
'fn f() {
    let a = current.raw();
    let b = wanted.raw();
    if a == b { g(); }
}'

check "binding it with a type annotation is refused too" 1 "[bound]" \
'fn f() { let mine: QpdfObjectHandle = page.raw(); }'

# --- Case 5c: the other shapes review found unflagged --------------------------------------
check "a map lookup keyed on a raw handle is refused" 1 "[searched]" \
'fn f() { let v = seen.get(&page.raw()); }'

check "a binary search on a raw handle is refused" 1 "[searched]" \
'fn f() { let at = pages.binary_search(&page.raw()); }'

check "an ordering comparator on raw handles is refused" 1 "[collected]" \
'fn f() { let o = a.raw().cmp(&b.raw()); }'

check "extending a collection with a raw handle is refused" 1 "[collected]" \
'fn f() { kept.extend([page.raw()]); }'

check "assert_eq! on raw handles is refused" 1 "[compared]" \
'fn f() { assert_eq!(a.raw(), b.raw()); }'

# --- Case 6: the legitimate uses, which must NOT be flagged -------------------------------
#
# If these fail, the rule is unusable and would be turned off -- which is the same outcome as
# not having it. `raw()` exists to be PASSED to qpdf; that is the whole point of the method.
check "passing a raw handle to qpdf passes" 0 "no handle compared as an identity" \
'fn f() {
    let removed = unsafe { ffi::qpdf_remove_page(document.data, wanted_page.raw()) };
    let added = unsafe { ffi::qpdf_add_page_at(d, d, wanted_page.raw(), TRUE, current.raw()) };
}'

check "handle.rs reading through self.handle passes" 0 "no handle compared as an identity" \
'fn object(&self) -> (c_int, c_int) {
    let id = unsafe { ffi::qpdf_oh_get_object_id(self.data, self.handle) };
    (id, unsafe { ffi::qpdf_oh_get_generation(self.data, self.handle) })
}'

# The narrowed binding rule. `examples/measure-*.rs` deal in the C API directly, own no
# `ObjectHandle`, and have to bind a handle to pass it on. The broad first version reported
# three of them -- and a rule that flags correct code gets turned off, which is the same
# outcome as not having the rule at all.
check "binding a raw FFI handle to pass it on passes" 0 "no handle compared as an identity" \
'fn f() {
    let page = qpdf_get_page_n(src, n);
    qpdf_add_page(dest, src, page, QPDF_FALSE);
}'

check "a comment quoting the wrong spelling passes" 0 "no handle compared as an identity" \
'// Never write `current.raw() == wanted.raw()`: qpdf issues a fresh handle per call.
fn f() { g(); }'

# --- Case 7: the escape hatch needs a reason ----------------------------------------------
check "an argued exemption is accepted and printed" 0 "argued exemption" \
'fn f() { if a.raw() == b.raw() { g(); } } // handle-identity-ok: both come from one call'

check "a waiver with no reason does not silence the rule" 1 "[compared]" \
'fn f() { if a.raw() == b.raw() { g(); } } // handle-identity-ok:'

# --- Case 8: examining nothing is a failure, not a pass ------------------------------------
printf 'fn f() { g(); }\n' >"$work/plain.rs"
check "a file with no handles at all passes" 0 "no handle compared as an identity" \
'fn f() { g(); }'

# --- the per-run probe gate must exist ------------------------------------------------------
#
# The checker verifies each rule against a fixture and a near-miss on EVERY invocation. That
# gate is only observable when a rule is broken, so deleting it leaves every case above green.
# This breaks a rule in a COPY and asserts the copy refuses, naming the reason.
#
# The copy sits BESIDE the original, because the checker resolves REPO from its own location:
# a copy in a temp directory would fail for the wrong reason, which an exit-code-only
# assertion reports as a pass. Measured in this repository, more than once.
# Mutate the "compared" rule's PATTERN in a copy of the checker, then assert the copy refuses
# to run. `python3 -c` rather than `sed`, because the pattern is full of regex metacharacters
# and a `sed` that quietly matched nothing would leave an identical file -- which is the exact
# shape of the two silent mutation failures this repository has already had.
mutate() {
  local copy="$1" replacement="$2"
  python3 - "$checker" "$copy" "$replacement" <<'PYEOF'
import pathlib, sys
src, dst, replacement = sys.argv[1], sys.argv[2], sys.argv[3]
text = pathlib.Path(src).read_text()
old = '''rf"{HANDLE}{GAP}[=!]=|[=!]={GAP}{HANDLE}"
            rf"|assert(?:_eq|_ne)!\\s*\\([^;]*{HANDLE}"'''
if old not in text:
    sys.exit("the mutation target is not in the checker; this case would prove nothing")
pathlib.Path(dst).write_text(text.replace(old, replacement, 1))
PYEOF
}

# `name replacement expected_reason ok_message fail_message`
probe_gate() {
  local name="$1" replacement="$2" reason="$3"
  local copy="$here/.handle-identity-probe-fixture.py"
  if ! mutate "$copy" "$replacement"; then
    echo "  FAIL $name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  # AND THE FILE ACTUALLY CHANGED. The write above could succeed and produce identical bytes.
  if cmp -s "$copy" "$checker"; then
    echo "  FAIL $name: the mutated copy is identical to the original"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi

  local out status
  set +e
  out="$(python3 "$copy" "$work/plain.rs" 2>&1)"
  status=$?
  set -e
  rm -f "$copy"

  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: the broken checker ran to completion and reported OK"
    fail=$((fail + 1))
  elif grep -qF "$reason" <<<"$out"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it failed, but not for the stated reason ($reason)"
    sed 's/^/        /' <<<"$out" | head -5
    fail=$((fail + 1))
  fi
}

# The gate runs on EVERY invocation, so deleting it leaves every case above green. Both halves
# are tested: a rule that matches nothing passes everything, and a rule that matches
# everything fails everything. Neither is visible from a green run.
probe_gate "a rule matching nothing stops the checker before it examines anything" \
  "r'ZZZNOMATCH'" "does not match its own fixture"
probe_gate "a rule matching the CORRECT code stops the checker too" \
  "r'[=!]='" "near-miss"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
