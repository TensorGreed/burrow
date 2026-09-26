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
# Everything here runs in CI's `deny` job, which fetches and installs nothing. `engines-wasm` is
# exercised end to end because its inputs are committed files. `pkg` is exercised through
# `begin` (its record needs no tools to be computed) and through `wrap` with a fake `wasm-pack`
# first on PATH -- `wrap` checks the command's text, not the binary that answers it.
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

# --- the baseline: the real tool passes its own probes, ALL of them ------------------------
# THE COUNT IS PINNED. "19 rule probe(s)" with no expectation beside it reads the same after a
# probe is deleted; a probe added or removed changes this number on purpose, in the same commit.
expected_probes=19
if out="$(stamper --probe 2>&1)" && grep -qF "$expected_probes rule probe(s) verified" <<<"$out"; then
  ok "the real stamper passes all $expected_probes of its probes"
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
mutant "a changed value that is not reported is refused" \
  'lines.append(f"{name}: built with {recorded[key]!r}, now {current[key]!r}")' \
  'pass' \
  "a changed value is named"
mutant "an export parse that reads a missing block as empty is refused" \
  '    if block is None:
        return None' \
  '    if block is None:
        return []' \
  "no export block reads as none, not as empty"
mutant "an emsdk parse that reads nothing is refused" \
  'return version if isinstance(version, str) and version else None' \
  'return None' \
  "the emsdk pin is read from [emsdk]"
mutant "a cache-key parse that reads nothing is refused" \
  "return sorted(re.findall(r\"'([^']+)'\", found.group(1)))" \
  'return []' \
  "the wasm cache key's files are read"
mutant "a closure that reaches a crate it does not depend on is refused" \
  'tables = [manifest.get("dependencies", {}), manifest.get("build-dependencies", {})]' \
  'tables = [manifest.get("dependencies", {}), manifest.get("build-dependencies", {}), {"burrow-ffi": {"path": "../../bindings/burrow-ffi"}}]' \
  "the binding's closure does not reach the uniffi binding"
mutant "a tree listing that lists nothing is refused" \
  'return sorted({p for p in listed.decode().split("\0") if p and (repo / p).is_file()})' \
  'return []' \
  "a crate's tree lists its own manifest"
mutant "an output rewrite that is not reported is refused" \
  '("rewritten since the stamp", sorted(k for k in recorded.keys() & current.keys() if recorded[k] != current[k])),' \
  '("rewritten since the stamp", []),' \
  "a rewritten output file is named"
mutant "an output file added since the stamp that is not reported is refused" \
  '("added since the stamp", sorted(current.keys() - recorded.keys())),' \
  '("added since the stamp", []),' \
  "an output file added since the stamp is named"
mutant "an output comparison that reports everything is refused" \
  '    lines: list[str] = []
    for label, names in (' \
  '    lines: list[str] = ["everything"]
    for label, names in (' \
  "an identical output differs in nothing"
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
# THE REVIEW'S BLOCKING FINDING: wasm-pack does not clear its --out-dir, so a build that skips
# `wrap` leaves the previous stamp beside new bytes. Doctored on the recorded side, which works
# with or without a real vendor tree: a file the stamp saw that the output no longer has.
verdict "an output the stamp does not describe is refused, naming the file" \
  'stamp["output"]["lib/planted.wasm"] = "0" * 64' "removed since the stamp (1): lib/planted.wasm"
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

# WHAT `pkg` RECORDS. The probes test the helpers; this tests that `compute` uses them -- a record
# that stopped hashing the crate tree would read "current" across every Rust change, which is
# #128 again. `begin` needs no rustc or wasm-pack; their values just say they are unavailable.
out="" status=0
out="$(stamper begin pkg 2>&1)" || status=$?
missing=""
for key in file:bindings/burrow-wasm/src/lib.rs file:core/burrow-types/Cargo.toml \
  file:core/burrow-engines/src/lib.rs file:Cargo.lock value:rustc value:wasm-pack value:command; do
  grep -qF "\"$key\"" <<<"$out" || missing="$missing $key"
done
if [ "$status" -eq 0 ] && [ -z "$missing" ]; then
  ok "pkg's record covers the binding, the crates beneath it, the lockfile and the tool versions"
else
  bad "pkg's record is missing:${missing:- (begin failed, status $status)}" "$out"
fi

# `wrap`, WITH A FAKE wasm-pack FIRST ON PATH. `wrap` checks the command's text, not which binary
# answers it, so this exercises the real control flow in a job that has no wasm-pack at all.
fake="$work/bin"
mkdir -p "$fake"
cat >"$fake/wasm-pack" <<'FAKE'
#!/usr/bin/env bash
[ "${1:-}" = "-V" ] && { echo "wasm-pack 0.0.0-fake"; exit 0; }
exit "${FAKE_WASM_PACK_STATUS:-0}"
FAKE
chmod +x "$fake/wasm-pack"
real_pkg=(wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg --release)

printf 'old stamp\n' >"$work/wrapped-fail"
out="" status=0
out="$(PATH="$fake:$PATH" FAKE_WASM_PACK_STATUS=3 stamper wrap pkg --stamp "$work/wrapped-fail" -- "${real_pkg[@]}" 2>&1)" || status=$?
if [ "$status" -eq 3 ] && [ ! -e "$work/wrapped-fail" ] && grep -qF "no stamp written" <<<"$out"; then
  ok "a failed wrapped build removes the old stamp and writes none"
else
  bad "a failed wrapped build left a stamp, or did not pass its status through (status $status)" "$out"
fi

out="" status=0
out="$(PATH="$fake:$PATH" stamper wrap pkg --stamp "$work/wrapped-ok" -- "${real_pkg[@]}" 2>&1)" || status=$?
if [ "$status" -eq 0 ] && [ -f "$work/wrapped-ok" ] \
  && python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); assert s["inputs"]["value:wasm-pack"]=="wasm-pack 0.0.0-fake" and "output" in s' "$work/wrapped-ok"; then
  ok "a successful wrapped build writes a stamp recording the wasm-pack that ran"
else
  bad "a successful wrapped build wrote no stamp, or the wrong one (status $status)" "$out"
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
# AND THE CASES ARE COUNTED, for the reason the probes are: a case deleted, or one whose branch
# stopped being reached, would otherwise leave "N passed, 0 failed" reading as success.
expected_cases=38
if [ "$pass" -ne "$expected_cases" ]; then
  echo "test-build-stamp: expected $expected_cases passing cases, got $pass" >&2
  exit 1
fi
[ "$fail" -eq 0 ]
