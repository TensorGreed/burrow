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

probes=0
refused=0
accepted=0

probe() {
  local label="$1" literal="$2" expect="$3"
  probes=$((probes + 1))
  if [ "$expect" = "no" ]; then refused=$((refused + 1)); else accepted=$((accepted + 1)); fi
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

# --- THE READ SIDE, which had the same bug and now has the same guard ---------------------
#
# `ObjectHandle::name` returns a `Name`, and a comparison takes one. So asking "is this
# `Type0`?" with the bare spelling is a compile error exactly as writing `getKey("Parent")` is.
#
# It was live: every `/Subtype` check in `resources.rs` compared a slashed name against a bare
# byte string, so a Type 0 font read as a simple one and the OCR fixture refused with entirely
# the wrong rule. `prune.rs` had always compared against `b"/Form"` and was never affected,
# which is the difference a convention makes versus a type.
read_probe() {
  local label="$1" comparison="$2" expect="$3"
  probes=$((probes + 1))
  if [ "$expect" = "no" ]; then refused=$((refused + 1)); else accepted=$((accepted + 1)); fi
  cat > "$work/read.rs" <<RS
#[derive(Clone)]
enum Name { Literal(&'static [u8]), Read(Vec<u8>) }
impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool { self.bytes() == other.bytes() }
}
impl Name {
    fn bytes(&self) -> &[u8] {
        match self { Self::Literal(b) => b, Self::Read(o) => o }
    }
    const fn literal(bytes: &'static [u8]) -> Self {
        assert!(matches!(bytes, [b'/', _, .., 0]), "bad name");
        Self::Literal(bytes)
    }
}
fn subtype() -> Name { Name::Read(b"/Type0\0".to_vec()) }
fn main() { let _ = $comparison; }
RS
  if rustc --edition 2021 --crate-type bin -o "$work/read" "$work/read.rs" >"$work/rout" 2>&1; then
    built=yes
  else
    built=no
  fi
  if [ "$built" != "$expect" ]; then
    echo "  FAIL $label: built=$built, expected=$expect" >&2
    sed 's/^/        /' "$work/rout" >&2
    return 1
  fi
  echo "  ok   $label (builds=$built)"
}

echo
echo "and the read side:"
read_probe "comparing a name against a bare byte string"  'subtype() == *b"Type0"'          no  || failures=$((failures+1))
read_probe "comparing against a slashed byte string"      'subtype() == *b"/Type0"'         no  || failures=$((failures+1))
# THE RESIDUE AND THE CONST POSITION, both measured. `Name::literal` is a plain `const fn`, so
# in a FUNCTION BODY a missing slash compiles and panics at run time -- which core/CLAUDE.md's
# "nothing panics" rule forbids in library code and no shell gate can see. The compile-time
# guarantee is about `const` items, and every library call site is one.
#
# A residue nobody measures is a residue nobody notices growing, so it gets a probe too.
const_probe() {
  local label="$1" literal="$2" expect="$3"
  probes=$((probes + 1))
  if [ "$expect" = "no" ]; then refused=$((refused + 1)); else accepted=$((accepted + 1)); fi
  cat > "$work/constpos.rs" <<RS
enum Name { Literal(&'static [u8]) }
impl Name {
    const fn literal(bytes: &'static [u8]) -> Self {
        assert!(matches!(bytes, [b'/', _, .., 0]), "bad name");
        Self::Literal(bytes)
    }
}
const SUBTYPE: Name = Name::literal($literal);
fn main() { let Name::Literal(b) = &SUBTYPE; let _ = b; }
RS
  if rustc --edition 2021 --crate-type bin -o "$work/constpos" "$work/constpos.rs" >"$work/cout" 2>&1; then
    built=yes
  else
    built=no
  fi
  if [ "$built" != "$expect" ]; then
    echo "  FAIL $label: built=$built, expected=$expect" >&2
    return 1
  fi
  echo "  ok   $label (builds=$built)"
}

const_probe "a const Name without its slash is refused"   'b"Type0\0"'  no  || failures=$((failures+1))
const_probe "THE CONTROL: a const Name with its slash"    'b"/Type0\0"' yes || failures=$((failures+1))
read_probe "THE RESIDUE: a non-const call still compiles" 'subtype() == Name::literal(b"Type0\0")' yes || failures=$((failures+1))
read_probe "THE CONTROL: comparing two Names"             'subtype() == Name::literal(b"/Type0\0")' yes || failures=$((failures+1))

if [ "$failures" -ne 0 ]; then
  echo "" >&2
  echo "FAILED -- $failures of 10 probe(s) behaved wrongly. A rule that accepts everything is" >&2
  echo "not a rule, and one that accepts nothing refuses the correct spellings too." >&2
  exit 1
fi
echo "OK -- $probes probe(s): $refused spelling(s) refused, $accepted accepted (the controls plus the one recorded residue);"
echo "     and the predicate is asserted in $crate, not merely mentioned in a comment."
