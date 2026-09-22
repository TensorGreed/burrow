#!/usr/bin/env bash
# A PDF name without its leading slash must be a COMPILE error, not a runtime one.
#
# `qpdf_oh_get_key("Parent")` returns a null object -- no error, no warning -- exactly as it
# does for a key that is genuinely absent. Measured on the first run of `sharing.rs`: every
# lookup returned null, the walk reported a document that draws nothing, and for a sharing
# count "draws nothing" reads as UNSHARED, which means edit the form in place and remove its
# text from every other page that draws it.
#
# `Name::literal` is a `const fn` whose `assert!` fails during const evaluation. A test cannot
# assert that, because a test that runs is a test that already compiled. So this script tries
# to build each rejected spelling and requires rustc to refuse it -- per-rule probes, in the
# only form available for a compile-time rule.
set -euo pipefail

cd "$(dirname "$0")/.."
crate="core/burrow-engines/src/qpdf/name.rs"
[ -f "$crate" ] || { echo "::error::$crate is missing"; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The predicate, lifted from `Name::literal` so this probes the real rule rather than a copy
# that could drift.
#
# COMMENTS ARE STRIPPED FIRST, and a review is why. The earlier version grepped the raw file, so
# deleting the `assert!` from `Name::literal` and leaving the predicate in a `//` comment left
# this script printing OK over a type that no longer checked anything -- and the unit test named
# `the_compile_time_check_is_real_and_not_a_comment` passed too. The same class as the two
# comment-satisfiable probes #129 closed.
#
# The check is on the whitespace-stripped, comment-stripped source, and it requires the
# `assert!` and the predicate TOGETHER: a predicate with no assertion around it is a comment
# with extra steps.
dense="$(sed 's://.*::' "$crate" | tr -d '[:space:]')"
needle="assert!(matches!(bytes,[b'/',_,..,0]),"
case "$dense" in
  *"$needle"*) ;;
  *)
    echo "::error::$crate no longer asserts the predicate this script probes." >&2
    echo "::error::Looked for, after stripping comments and whitespace: $needle" >&2
    exit 1
    ;;
esac

probe() {
  local label="$1" literal="$2" expect="$3"
  cat > "$work/probe.rs" <<RS
const fn literal(bytes: &'static [u8]) -> &'static [u8] {
    assert!(matches!(bytes, [b'/', _, .., 0]), "bad name");
    bytes
}
const PROBE: &[u8] = literal($literal);
fn main() { let _ = PROBE; }
RS
  if rustc --edition 2021 --crate-type bin -o "$work/probe" "$work/probe.rs" >"$work/out" 2>&1; then
    built=yes
  else
    built=no
  fi
  if [ "$built" != "$expect" ]; then
    echo "  FAIL $label: built=$built, expected=$expect" >&2
    sed 's/^/        /' "$work/out" >&2
    return 1
  fi
  echo "  ok   $label (builds=$built)"
}

echo "probing the compile-time name rule"
failures=0
probe "a name with no leading slash"      'b"Parent\0"'   no  || failures=$((failures+1))
probe "a name with no NUL terminator"     'b"/Parent"'    no  || failures=$((failures+1))
probe "an empty name"                     'b""'           no  || failures=$((failures+1))
probe "a bare slash and NUL"              'b"/\0"'        no  || failures=$((failures+1))
probe "THE CONTROL: a correct name"       'b"/Parent\0"'  yes || failures=$((failures+1))
probe "THE CONTROL: the shortest name"    'b"/N\0"'       yes || failures=$((failures+1))

if [ "$failures" -ne 0 ]; then
  echo "" >&2
  echo "FAILED -- $failures of 6 probe(s) behaved wrongly. A rule that accepts everything is" >&2
  echo "not a rule, and one that accepts nothing refuses the correct spellings too." >&2
  exit 1
fi
echo "OK -- 6 probe(s): 4 rejected spellings refused at compile time, 2 correct ones accepted;"
echo "     and the predicate is asserted in $crate, not merely mentioned in a comment."
