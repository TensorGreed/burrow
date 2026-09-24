#!/usr/bin/env bash
#
# Build the workspace's rustdoc WITH the engines linked, and report what it documented.
#
# WHY --all-features, AND WHY THIS WAS A GAP (#174)
#
# CI ran `cargo doc --workspace --no-deps` without `--all-features`. Every item behind
# `#[cfg(all(feature = "native-engines", burrow_native_engines))]` was therefore never compiled
# by the doc job, and its rustdoc -- intra-doc links included -- was checked by nothing. Measured
# while working on #165: two stale links in `redact_verify.rs`, a file that job never saw. The
# job printed OK over a strictly smaller set than a reader assumes, which is the shape CLAUDE.md
# calls a check that silently examines nothing.
#
# It runs in the `test` job, which already fetches and builds `engines/vendor/` -- `build.rs`
# FAILS rather than skipping when `--all-features` asks for engines that are not there, so a
# job without them cannot pass this by documenting less.
#
# WHAT IT DOES NOT CHECK, STATED BECAUSE THE FIRST VERSION IMPLIED IT DID
#
# **Rustdoc on PRIVATE items.** Without `--document-private-items`, rustdoc does not resolve an
# intra-doc link on a private item, so a broken one there passes. A code review measured at least
# 14 such links today, 10 inside engine-gated private modules (`pdfium/thread.rs`,
# `qpdf/redact_steps.rs`, `qpdf/handle.rs`, the two `ffi.rs`). Turning the flag on here would
# make this gate red on arrival, so it is a follow-up (#187): fix the links, then add the flag. Until
# then the gate covers the PUBLIC engine-gated surface and says so.
#
# WHAT IT EXAMINED, AGAINST WHAT IT SHOULD HAVE
#
# A clean `cargo doc` says nothing about how much it documented. So after it runs, this checks:
#   - every workspace library target has a doc page, the list and each target's real name coming
#     from `cargo metadata`, not typed here;
#   - every PUBLIC item the engine gate guards at the `burrow-engines` crate root has its page, the
#     list derived from `lib.rs`. A public gated item of a kind this cannot map to a page is
#     refused rather than counted as private, which is the direction that under-counts.
#
# The doc directories are CLEARED first. `cargo doc` regenerates only the crates it documents, so a
# page left by an earlier build would satisfy this for a crate the current build never touched --
# measured by a review with a one-crate build over a full earlier tree, which reported 6 of 6.
#
# BURROW_DOC_DIR, when set, skips the build and checks that tree instead, and BURROW_LIB_RS
# replaces the `lib.rs` the gated items are derived from. Both exist for
# `tools/test-check-rustdoc.sh`; nothing else should set them, and the output says when the build
# was skipped.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
metadata="$(cd "$root" && cargo metadata --no-deps --format-version 1)"
target_dir="$(python3 -c 'import json, sys; print(json.loads(sys.argv[1])["target_directory"])' "$metadata")"
crates="$(python3 -c '
import json, sys
meta = json.loads(sys.argv[1])
for package in meta["packages"]:
    for target in package["targets"]:
        if any(kind in ("lib", "rlib") for kind in target["kind"]):
            print(target["name"].replace("-", "_"))
            break
' "$metadata")"

if [ -z "${BURROW_DOC_DIR:-}" ]; then
    doc_dir="$target_dir/doc"
    # CLEARED, so every page checked below was written by THIS build.
    for crate in $crates; do
        rm -rf "${doc_dir:?}/$crate"
    done
    (cd "$root" && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features)
    built="built by this run"
else
    doc_dir="$BURROW_DOC_DIR"
    built="BUILD SKIPPED -- checking the tree at BURROW_DOC_DIR, for the self-test"
fi

python3 - "${BURROW_LIB_RS:-$root/core/burrow-engines/src/lib.rs}" "$doc_dir" "$crates" "$built" <<'PYEOF'
import pathlib, re, sys

lib_rs, doc_dir, crates, built = (
    pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3].split(), sys.argv[4]
)

def gated(attribute: str) -> bool:
    """Whether a `#[cfg(all(...))]` attribute is the engine gate, whatever order it names its parts."""
    return (
        attribute.startswith("#[cfg(all(")
        and re.search(r'feature\s*=\s*"native-engines"', attribute) is not None
        and re.search(r"\bburrow_native_engines\b", attribute) is not None
    )

# The page each kind of public item gets. Anything public and gated that is NOT one of these is
# refused below -- counting it as private would under-count, and quietly.
PAGES = {
    "mod": lambda name: pathlib.Path(name) / "index.html",
    "fn": lambda name: pathlib.Path(f"fn.{name}.html"),
    "struct": lambda name: pathlib.Path(f"struct.{name}.html"),
    "trait": lambda name: pathlib.Path(f"trait.{name}.html"),
    "enum": lambda name: pathlib.Path(f"enum.{name}.html"),
}

lines = lib_rs.read_text().splitlines()
public, private, unrecognised = [], [], []
at = 0
while at < len(lines):
    if not lines[at].strip().startswith("#[cfg(all("):
        at += 1
        continue
    # A rustfmt-WRAPPED attribute is joined before it is read.
    joined, end = lines[at].strip(), at
    while not joined.endswith(")]") and end + 1 < len(lines):
        end += 1
        joined += " " + lines[end].strip()
    at = end + 1
    if not gated(joined):
        continue
    item = at
    while item < len(lines) and lines[item].strip().startswith(("#[", "//")):
        item += 1
    text = lines[item].strip() if item < len(lines) else ""
    found = re.match(r"pub (?:unsafe |const |async )*(mod|fn|struct|trait|enum) (\w+)", text)
    if found:
        public.append(found.groups())
    elif text.startswith("pub ") or text.startswith("pub use"):
        unrecognised.append(text[:60])
    else:
        private.append(text[:40])

failures = []
if unrecognised:
    failures.append(
        "a public engine-gated item this cannot map to a doc page, so it cannot be checked: "
        + "; ".join(unrecognised)
    )
if not public:
    failures.append("no public engine-gated item was derived, so nothing here is checked")

missing_crates = [c for c in crates if not (doc_dir / c / "index.html").is_file()]
missing_items = [
    f"{kind} {name}" for kind, name in public
    if not (doc_dir / "burrow_engines" / PAGES[kind](name)).is_file()
]
if missing_crates:
    failures.append(f"crate(s) with no doc page: {', '.join(missing_crates)}")
if missing_items:
    failures.append(
        "engine-gated item(s) with no doc page -- documented without --all-features, or not at "
        f"all: {', '.join(missing_items)}"
    )

print(
    f"rustdoc ({built}): {len(crates) - len(missing_crates)} of {len(crates)} workspace crate(s) "
    f"documented; {len(public) - len(missing_items)} of {len(public)} public engine-gated "
    f"item(s) ({', '.join(name for _, name in public)}); {len(private)} gated private item(s) "
    "not checked, because private rustdoc is not built yet"
)
if failures:
    for failure in failures:
        print(f"  FAIL {failure}", file=sys.stderr)
    sys.exit(1)
print("OK — every crate and every public engine-gated item has its page.")
PYEOF
