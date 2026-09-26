#!/usr/bin/env bash
# Adversarial self-test for tools/build-stamp.py (#149).
#
# Two halves.
#
# THE PROBE GATE. Each rule in the stamper is broken in a COPY placed beside the original --
# beside it, so the copy resolves the repository the same way and fails for the rule's reason
# rather than for a wrong path -- and the copy must refuse, NAMING THE PROBE for that rule.
# Every mutation is asserted to have applied before the copy runs.
#
# THE VERDICTS. A stamp is planted somewhere temporary with `--stamp` and then doctored the way
# a stale one differs from a current one: a changed input file, an input the build never saw,
# an export added since, another format, another artifact, nothing at all. Each must refuse,
# naming what differs. The inputs are always this repository's own, so nothing here edits the
# tree -- the doctoring happens to the stamp, which is the other side of the same comparison.
#
# Only `engines-wasm` is exercised end to end: its inputs are committed files, so this runs in
# CI's `deny` job, which fetches and installs nothing. `pkg`'s record needs rustc and
# wasm-pack; its one verdict here is the refusal to stamp a build that is not its own.
#
# Usage: tools/test-build-stamp.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tool="$here/build-stamp.py"
copy="$here/build-stamp.selftest-copy.py"
work="$(mktemp -d)"
trap 'rm -rf "$work" "$copy"' EXIT

pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; [ -n "${2:-}" ] && sed 's/^/        /' <<<"$2" | tail -6; fail=$((fail + 1)); }

stamper() { python3 -B "$tool" "$@"; }

# --- the baseline: the real tool passes its own probes --------------------------------------
if out="$(stamper --probe 2>&1)"; then
  ok "the real stamper passes its probes ($(grep -o '[0-9]* rule probe' <<<"$out"))"
else
  bad "the real stamper fails its own probes, so no case below means anything" "$out"
  exit 1
fi

# --- the probe gate -------------------------------------------------------------------------
# `mutant <name> <old> <new> <the probe that must be named>`
mutant() {
  local name="$1" old="$2" new="$3" expect="$4"
  python3 - "$tool" "$copy" "$old" "$new" <<'PYEOF'
import pathlib, sys
src, dst, old, new = sys.argv[1:]
text = pathlib.Path(src).read_text()
if old not in text:
    sys.exit(f"the mutation target is not in build-stamp.py: {old!r}")
pathlib.Path(dst).write_text(text.replace(old, new, 1))
PYEOF
  if cmp -s "$tool" "$copy"; then
    bad "$name: the mutation did not apply"
    return
  fi
  local out status=0
  out="$(python3 -B "$copy" --probe 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    bad "$name: the broken copy passed its probes" "$out"
  elif ! grep -qF -- "$expect" <<<"$out"; then
    bad "$name: refused, but did not name the probe '$expect'" "$out"
  else
    ok "$name"
  fi
}

mutant "a changed input that is not reported is refused" \
  'changed = [k for k in recorded.keys() & current.keys() if recorded[k] != current[k]]' \
  'changed = [k for k in recorded.keys() & current.keys() if False]' \
  "a changed file is named"
mutant "an input new since the build that is not reported is refused" \
  'listing("new since the build", files(current.keys() - recorded.keys()))' \
  'listing("new since the build", [])' \
  "a new file is named"
mutant "an input gone since the build that is not reported is refused" \
  'listing("gone since the build", files(recorded.keys() - current.keys()))' \
  'listing("gone since the build", [])' \
  "a vanished file is named"
mutant "a comparison that reports everything is refused" \
  'lines: list[str] = []' \
  'lines: list[str] = ["everything differs"]' \
  "an identical record differs in nothing"
mutant "an export delta that does not name the symbol is refused" \
  'delta = [f"+{s}" for s in sorted(now - was)]' \
  'delta = [f"+" for s in sorted(now - was)]' \
  "an export added since the build is named by symbol"
mutant "an export parse that reads outside the block is refused" \
  'return sorted(set(re.findall(r'"'"'"(_\w+)"'"'"', block.group(1))))' \
  'return sorted(set(re.findall(r'"'"'"(_\w+)"'"'"', build_script)))' \
  "the export list is read from its block"
mutant "an emsdk parse that takes any table's version is refused" \
  'version = tomllib.loads(pins_text).get("emsdk", {}).get("version")' \
  'version = next((t.get("version") for t in tomllib.loads(pins_text).values() if isinstance(t, dict)), None)' \
  "a version under another table is not the emsdk pin"
mutant "a cache-key parse that reads commented lines is refused" \
  '        if line.lstrip().startswith("#"):
            continue
        found = re.search(rf"\bkey:' \
  '        found = re.search(rf"\bkey:' \
  "a commented or native key is not the wasm key"
mutant "a closure that stops at the crate itself is refused" \
  'stack.extend(path_dependencies(directory) - seen)' \
  'stack.extend(set())' \
  "the binding's closure reaches burrow-types"
mutant "a tree listing that is not scoped to the crate is refused" \
  '"--exclude-standard", "--", *directories]' \
  '"--exclude-standard", "--", *directories, "tools"]' \
  "a crate's tree lists nothing outside it"

# And the rule that is not a probe: the CI cache key must hash exactly what the stamp records.
python3 - "$tool" "$copy" <<'PYEOF'
import pathlib, sys
src, dst = sys.argv[1:]
text = pathlib.Path(src).read_text()
old = '"engines/pins.sh",\n'
assert old in text, "the engines-wasm input list no longer names engines/pins.sh"
pathlib.Path(dst).write_text(text.replace(old, "", 1))
PYEOF
out="" status=0
out="$(python3 -B "$copy" --probe 2>&1)" || status=$?
if cmp -s "$tool" "$copy"; then
  bad "an input dropped from the stamp but kept in the cache key: the mutation did not apply"
elif [ "$status" -ne 0 ] && grep -qF "extra ['engines/pins.sh']" <<<"$out"; then
  ok "a cache key that hashes more than the stamp records is refused, naming the file"
else
  bad "a cache key that hashes more than the stamp records was not refused by name" "$out"
fi
rm -f "$copy"

# --- the verdicts -----------------------------------------------------------------------------
record="$work/record.json"
stamp="$work/stamp"
stamper begin engines-wasm >"$record"
if out="$(stamper commit engines-wasm "$record" --stamp "$stamp" 2>&1)" && [ -f "$stamp" ]; then
  ok "a record taken from a still tree commits"
else
  bad "a record taken from a still tree did not commit" "$out"
fi

# `verdict <name> <python edit of the stamp's JSON, or "" for none> <expected text>`
verdict() {
  local name="$1" edit="$2" expect="$3"
  local doctored="$work/doctored"
  cp "$stamp" "$doctored"
  if [ -n "$edit" ]; then
    python3 - "$doctored" "$edit" <<'PYEOF'
import json, pathlib, sys
path, edit = pathlib.Path(sys.argv[1]), sys.argv[2]
stamp = json.loads(path.read_text())
before = json.dumps(stamp, sort_keys=True)
exec(edit, {"stamp": stamp, "inputs": stamp["inputs"]})
if json.dumps(stamp, sort_keys=True) == before:
    sys.exit("the edit changed nothing")
path.write_text(json.dumps(stamp))
PYEOF
  fi
  local out status=0
  out="$(stamper check engines-wasm --stamp "$doctored" 2>&1)" || status=$?
  if [ -z "$edit" ]; then
    if [ "$status" -eq 0 ] && grep -qE "current -- [0-9]+ input\(s\) examined, [0-9]+ recorded" <<<"$out"; then
      ok "$name"
    else
      bad "$name" "$out"
    fi
    return
  fi
  if [ "$status" -ne 0 ] && grep -qF -- "$expect" <<<"$out"; then
    ok "$name"
  else
    bad "$name: expected a refusal naming '$expect' (exit $status)" "$out"
  fi
}

verdict "a current stamp passes and says how much it examined" "" ""
verdict "a changed input file is refused by name" \
  'inputs["file:engines/pins.toml"] = "0" * 64' "changed since the build (1): engines/pins.toml"
verdict "an input the build never saw is refused by name" \
  'del inputs["file:engines/pins.sh"]' "new since the build (1): engines/pins.sh"
verdict "an input the tree no longer has is refused by name" \
  'inputs["file:engines/retired.sh"] = "0" * 64' "gone since the build (1): engines/retired.sh"
verdict "an export added since the build is refused by symbol -- #201's case" \
  'inputs["value:qpdf exports"] = inputs["value:qpdf exports"].replace("_qpdf_oh_set_array_item", "").strip()' \
  "+_qpdf_oh_set_array_item"
verdict "a different emsdk pin is refused with both versions" \
  'inputs["value:emsdk (pinned)"] = "0.0.1"' "emsdk (pinned): built with '0.0.1'"
verdict "a stamp of another format is refused" 'stamp["format"] = 0' "is format 0"
verdict "a stamp for another artifact is refused" 'stamp["artifact"] = "pkg"' "the stamp there is for 'pkg'"

out="" status=0
out="$(stamper check engines-wasm --stamp "$work/nothing-here" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "no stamp at" <<<"$out" && grep -qF "engines/build-wasm.sh" <<<"$out"; then
  ok "an absent stamp is refused, with the command that rebuilds it"
else
  bad "an absent stamp was not refused with its fix" "$out"
fi

printf 'not json' >"$work/garbage"
out="" status=0
out="$(stamper check engines-wasm --stamp "$work/garbage" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "unreadable" <<<"$out"; then
  ok "an unreadable stamp is refused"
else
  bad "an unreadable stamp was not refused" "$out"
fi

# A tree that moved during the build: the record from `begin` no longer matches at `commit`.
python3 - "$record" <<'PYEOF'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
record = json.loads(path.read_text())
record["file:engines/fetch.sh"] = "0" * 64
path.write_text(json.dumps(record))
PYEOF
out="" status=0
rm -f "$work/moved"
out="$(stamper commit engines-wasm "$record" --stamp "$work/moved" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "changed while it was building" <<<"$out" \
  && grep -qF "engines/fetch.sh" <<<"$out" && [ ! -e "$work/moved" ]; then
  ok "a build whose inputs moved under it gets no stamp, and the input is named"
else
  bad "a build whose inputs moved under it was stamped, or the refusal did not name the input" "$out"
fi

# `wrap` stamps only the build its artifact is defined by. Run with anything else it must
# refuse BEFORE running it, and write nothing.
out="" status=0
out="$(stamper wrap pkg --stamp "$work/wrapped" -- wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg-render --release 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -qF "wrap runs exactly" <<<"$out" && [ ! -e "$work/wrapped" ]; then
  ok "wrap refuses to stamp pkg with the render build's command, and writes nothing"
else
  bad "wrap stamped, or ran, a build that is not the artifact's own" "$out"
fi

echo
echo "test-build-stamp: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
