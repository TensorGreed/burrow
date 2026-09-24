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
# WHAT IT EXAMINED, AGAINST WHAT IT SHOULD HAVE
#
# A clean `cargo doc` says nothing about how much it documented. So after it runs, this checks:
#   - every workspace crate with a library target has a doc page, the crate list coming from
#     `cargo metadata`, not typed here;
#   - every PUBLIC item the engine gate guards at the crate root has its page, the item list
#     derived from `core/burrow-engines/src/lib.rs` -- and the derivation itself is gated: the
#     gated attributes must equal the public items plus the private ones, so a pattern that
#     silently matched fewer cannot read as a complete sweep.
#
# BURROW_DOC_DIR, when set, skips the build and checks that tree instead. It exists for
# `tools/test-check-rustdoc.sh`, which plants synthetic trees; nothing else should set it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
doc_dir="${BURROW_DOC_DIR:-$root/target/doc}"

if [ -z "${BURROW_DOC_DIR:-}" ]; then
    (cd "$root" && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features)
fi

crates=$(cd "$root" && cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
meta = json.load(sys.stdin)
for package in meta["packages"]:
    if any(kind in ("lib", "rlib") for target in package["targets"] for kind in target["kind"]):
        print(package["name"].replace("-", "_"))
')

python3 - "$root" "$doc_dir" "$crates" <<'PYEOF'
import pathlib, re, sys

root, doc_dir, crates = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3].split()

# THE GATE, as the source writes it. A multi-line `#[cfg(all(` is joined before matching, so a
# rustfmt-wrapped attribute is counted like a one-line one.
GATE = re.compile(r'#\[cfg\(all\([^\]]*feature\s*=\s*"native-engines"[^\]]*burrow_native_engines')

lib = (root / "core/burrow-engines/src/lib.rs").read_text()
lines = lib.splitlines()
attributes, public, private = 0, [], []
at = 0
while at < len(lines):
    line = lines[at]
    if line.strip().startswith("#[cfg(all("):
        joined, end = line, at
        while "]" not in joined and end + 1 < len(lines):
            end += 1
            joined += " " + lines[end].strip()
        if GATE.search(joined):
            attributes += 1
            item = end + 1
            while item < len(lines) and (lines[item].strip().startswith("#[") or lines[item].strip().startswith("//")):
                item += 1
            text = lines[item].strip() if item < len(lines) else ""
            found = re.match(r"pub (mod|fn) (\w+)", text)
            if found:
                kind, name = found.groups()
                public.append((kind, name))
            else:
                private.append(text[:40])
        at = end + 1
        continue
    at += 1

failures = []
if attributes == 0 or attributes != len(public) + len(private):
    failures.append(
        f"the engine-gate derivation is broken: {attributes} gated attribute(s), "
        f"{len(public)} public and {len(private)} private item(s) after them"
    )
if not public:
    failures.append("no public engine-gated item was derived, so nothing here is checked")

missing_crates = [c for c in crates if not (doc_dir / c / "index.html").is_file()]
page = {"mod": lambda n: pathlib.Path(n) / "index.html", "fn": lambda n: pathlib.Path(f"fn.{n}.html")}
missing_items = [
    f"{kind} {name}" for kind, name in public
    if not (doc_dir / "burrow_engines" / page[kind](name)).is_file()
]
if missing_crates:
    failures.append(f"crate(s) with no doc page: {', '.join(missing_crates)}")
if missing_items:
    failures.append(
        "engine-gated item(s) with no doc page -- documented without --all-features, or not at "
        f"all: {', '.join(missing_items)}"
    )

print(
    f"rustdoc: {len(crates) - len(missing_crates)} of {len(crates)} workspace crate(s) "
    f"documented; {len(public) - len(missing_items)} of {len(public)} public engine-gated "
    f"item(s) ({', '.join(name for _, name in public)}), from {attributes} gated attribute(s) "
    f"of which {len(private)} guard private items"
)
if failures:
    for failure in failures:
        print(f"  FAIL {failure}", file=sys.stderr)
    sys.exit(1)
print("OK — every crate and every public engine-gated item has its page.")
PYEOF
