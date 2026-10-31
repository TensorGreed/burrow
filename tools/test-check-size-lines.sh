#!/usr/bin/env bash
# Adversarial self-test for tools/check-size-lines.sh and tools/size-lines.mjs.
#
# THE CHECK'S FAILURE MODE IS A FILE THAT LOOKS COMPLETE. So every rule is held to a per-rule probe
# on every run: a plant that breaks exactly that rule must be refused NAMING the reason, and its
# near-miss twin (the same file with the rule's input legal but unusual) must pass. Then each rule is
# broken in a COPY of the checker, beside the original so `./first-load.mjs` resolves, and the copy
# must no longer refuse that plant: that is what shows the plant exercises the rule rather than
# tripping some other one. Every mutation asserts it applied first.
#
# Baseline: the real `apps/web/dist` is measured, written and checked, and the count printed must
# be "examined N of N" with N read from size-budget.json -- not a number this file chose.
#
# Usage: tools/test-check-size-lines.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-size-lines.sh"
engine="$here/size-lines.mjs"
mutant="$here/size-lines.selftest-mutant.mjs"
real="$repo/apps/web/dist"
workroot="$(mktemp -d)"

cleanup() {
  trap - EXIT INT TERM HUP
  rm -f "$mutant"
  rm -rf "$workroot"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# `refuses <name> <expect> <file>`: the checker refuses, naming the reason.
refuses() {
  local name="$1" expect="$2" file="$3" out status=0
  out="$("$checker" --file "$file" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then bad "$name: it did not refuse"; return; fi
  if ! grep -qF -- "$expect" <<<"$out"; then
    bad "$name: refused, but not for the stated reason (wanted: $expect)"
    sed 's/^/        /' <<<"$out" | tail -3
    return
  fi
  ok "$name"
}
# `accepts <name> <file>`: a near-miss twin passes.
accepts() {
  local name="$1" file="$2" out status=0
  out="$("$checker" --file "$file" 2>&1)" || status=$?
  if [ "$status" -ne 0 ]; then bad "$name: the near-miss was refused"; sed 's/^/        /' <<<"$out" | tail -3; return; fi
  ok "$name"
}

# The expected names, from the budget itself.
expected_n="$(node --input-type=module -e '
import { readFileSync } from "node:fs";
import { expectedLineKeys } from "'"$engine"'";
const b = JSON.parse(readFileSync("'"$repo"'/apps/web/size-budget.json", "utf8"));
console.log(expectedLineKeys(b).length);')"
if [ "${expected_n:-0}" -lt 1 ]; then echo "FAILED: could not read the expected line count" >&2; exit 1; fi

# A synthetic complete file, built from the budget's names, and `mutate <js body>` over a copy of it.
base="$workroot/base.json"
node --input-type=module -e '
import { readFileSync, writeFileSync } from "node:fs";
import { expectedLineKeys } from "'"$engine"'";
const b = JSON.parse(readFileSync("'"$repo"'/apps/web/size-budget.json", "utf8"));
const hex = (c) => c.repeat(64);
const base = Object.keys(b.artifacts);
const lines = Object.fromEntries(expectedLineKeys(b).map((k, i) => [k, { raw: 1000 + i, brotli: 500 + i, sha256: hex("a"), sha256_normalised: hex("b") }]));
writeFileSync("'"$base"'", JSON.stringify({ format: 1, arch: "x86_64", measured_route: "/merge-pdf/index.html", source: null, lines, total: { raw: base.reduce((n, k) => n + lines[k].raw, 0), brotli: base.reduce((n, k) => n + lines[k].brotli, 0) }, render_total: { raw: Object.values(lines).reduce((n, l) => n + l.raw, 0), brotli: Object.values(lines).reduce((n, l) => n + l.brotli, 0) } }, null, 2));'

n=0
mutate() { # mutate <name-for-file> <js statement over d>
  n=$((n + 1))
  local out="$workroot/plant$n.json"
  node -e '
const fs = require("fs");
const d = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
'"$2"'
fs.writeFileSync(process.argv[2], JSON.stringify(d));' "$base" "$out"
  echo "$out"
}

echo "per-rule probes (each: the plant is refused naming its reason, the twin passes) -- $expected_n base lines expected"

accepts "the synthetic baseline passes, so a refusal below means something" "$base"
keys="$(node -e 'const d=JSON.parse(require("fs").readFileSync(process.argv[1]));console.log(Object.keys(d.lines).join(" "))' "$base")"
lines_checked=0
for key in $keys; do
  refuses "line $key absent" "lines lacks $key" "$(mutate x "delete d.lines[\"$key\"];")"
  lines_checked=$((lines_checked + 1))
done
if [ "$lines_checked" -ne "$expected_n" ]; then bad "probed $lines_checked of $expected_n lines"; else ok "every expected line was probed by name ($lines_checked of $expected_n)"; fi

refuses "an extra line" "unexpected" "$(mutate x 'd.lines["engines/zzz.wasm"] = d.lines[Object.keys(d.lines)[0]];')"
refuses "an empty lines object" "lines lacks" "$(mutate x 'd.lines = {};')"
refuses "zero brotli" "malformed brotli" "$(mutate x 'd.lines[Object.keys(d.lines)[0]].brotli = 0;')"
refuses "a fractional raw" "malformed raw" "$(mutate x 'd.lines[Object.keys(d.lines)[0]].raw = 1.5;')"
refuses "a short sha256" "malformed sha256" "$(mutate x 'd.lines[Object.keys(d.lines)[0]].sha256 = "abc";')"
refuses "a missing normalised sha" "malformed sha256_normalised" "$(mutate x 'delete d.lines[Object.keys(d.lines)[0]].sha256_normalised;')"
refuses "a missing total" "total has a missing" "$(mutate x 'delete d.total;')"
refuses "a missing render_total" "render_total has a missing" "$(mutate x 'd.render_total.brotli = 0;')"
refuses "a total that is not its lines' sum" "total.brotli is" "$(mutate x 'd.total.brotli += 1;')"
refuses "a render total that is not its lines' sum" "render_total.raw is" "$(mutate x 'd.render_total.raw -= 1;')"
refuses "format 2" "format is 2" "$(mutate x 'd.format = 2;')"
refuses "no arch" "arch is missing" "$(mutate x 'delete d.arch;')"
refuses "no measured_route" "measured_route is missing" "$(mutate x 'd.measured_route = "";')"
refuses "no source key" "source is missing" "$(mutate x 'delete d.source;')"
printf 'not json' >"$workroot/notjson.json"
refuses "not JSON" "cannot read" "$workroot/notjson.json"
printf '[]' >"$workroot/array.json"
refuses "a JSON array" "not a JSON object" "$workroot/array.json"

# Near-miss twins: legal but unusual, which a rule matching too much would refuse.
accepts "source set (the CI shape)" "$(mutate x 'd.source = {sha:"0123abcd", run_id:"1", workflow:"CI", event:"push"};')"
accepts "arch aarch64 (a local file)" "$(mutate x 'd.arch = "aarch64";')"
accepts "lines in reverse order" "$(mutate x 'd.lines = Object.fromEntries(Object.entries(d.lines).reverse());')"
accepts "an extra field on a line" "$(mutate x 'd.lines[Object.keys(d.lines)[0]].files = ["a.js"];')"

echo
echo "mutants (a copy of the checker with one rule broken must stop refusing that rule's plant)"
mutant_gate() { # <name> <old> <new> <plant file> <expect>
  local name="$1" old="$2" new="$3" plant="$4" expect="$5"
  node -e '
const fs = require("fs");
const [src, dst, old, nw] = process.argv.slice(1);
const s = fs.readFileSync(src, "utf8");
if (!s.includes(old)) { console.error("MUTATION DID NOT APPLY: " + old); process.exit(3); }
const t = s.replace(old, nw);
if (t === s) { console.error("MUTATION CHANGED NOTHING"); process.exit(3); }
fs.writeFileSync(dst, t);' "$engine" "$mutant" "$old" "$new" || { bad "$name: the mutation did not apply"; return; }
  local out status=0
  out="$(node "$mutant" check "$plant" 2>&1)" || status=$?
  if grep -qF -- "$expect" <<<"$out"; then bad "$name: the mutant STILL refuses the plant, so the rule is not what refuses it"; return; fi
  ok "$name: mutant no longer says '$expect'"
}
firstkey="$(node -e 'const d=JSON.parse(require("fs").readFileSync(process.argv[1]));console.log(Object.keys(d.lines)[0])' "$base")"
mutant_gate "the absent-line rule" 'if (absent.length > 0)' 'if (false)' "$(mutate x "delete d.lines[\"$firstkey\"];")" "lines lacks"
mutant_gate "the extra-line rule" 'if (extra.length > 0)' 'if (false)' "$(mutate x 'd.lines["engines/zzz.wasm"] = d.lines[Object.keys(d.lines)[0]];')" "unexpected"
mutant_gate "the zero-brotli rule" 'Number.isSafeInteger(n) && n > 0' 'Number.isSafeInteger(n)' "$(mutate x 'd.lines[Object.keys(d.lines)[0]].brotli = 0;')" "malformed brotli"
mutant_gate "the sha256 rule" 'if (!isHex64(line?.sha256)) bad.push("sha256");' 'if (false) bad.push("sha256");' "$(mutate x 'd.lines[Object.keys(d.lines)[0]].sha256 = "abc";')" "malformed sha256"
mutant_gate "the sha256_normalised rule, on its own" 'if (!isHex64(line?.sha256_normalised)) bad.push("sha256_normalised");' 'if (false) bad.push("sha256_normalised");' "$(mutate x 'd.lines[Object.keys(d.lines)[0]].sha256_normalised = "abc";')" "malformed sha256_normalised"
mutant_gate "the arch rule" 'typeof doc.arch !== "string" || doc.arch === ""' 'false' "$(mutate x 'delete d.arch;')" "arch is missing"
mutant_gate "the measured_route rule" 'typeof doc.measured_route !== "string" || doc.measured_route === ""' 'false' "$(mutate x 'd.measured_route = "";')" "measured_route is missing"
mutant_gate "the not-an-object rule" 'doc === null || typeof doc !== "object" || Array.isArray(doc)' 'false' "$workroot/array.json" "not a JSON object"
mutant_gate "the lines-missing rule" 'if (lines === null) {' 'if (false) {' "$(mutate x 'delete d.lines;')" "lines is missing"
mutant_gate "the total-presence rule" '!isCount(doc[name]?.raw) || !isCount(doc[name]?.brotli)' 'false' "$(mutate x 'delete d.total;')" "total has a missing"
mutant_gate "the total-sum rule" 'doc[name][field] !== sum' 'false' "$(mutate x 'd.total.brotli += 1;')" "total.brotli is"
mutant_gate "the format rule" 'doc.format !== 1' 'false' "$(mutate x 'd.format = 2;')" "format is 2"
mutant_gate "the source rule" '!("source" in doc)' 'false' "$(mutate x 'delete d.source;')" "source is missing"

echo
echo "end to end, on the real production build"
if [ ! -d "$real" ]; then
  echo "  SKIP-REFUSED: no build at apps/web/dist; run \`pnpm build\` in apps/web. Refusing rather than passing vacuously." >&2
  exit 1
fi
real_out="$workroot/real/size-lines.json"
out="$("$checker" --out "$real_out" 2>&1)" && status=0 || status=$?
if [ "$status" -ne 0 ]; then bad "the real build's file was refused"; sed 's/^/        /' <<<"$out"; else ok "the real build's file passes"; fi
if grep -qF "examined $expected_n of $expected_n expected base line(s)" <<<"$out"; then ok "it reported 'examined $expected_n of $expected_n'"; else bad "its count line is not 'examined $expected_n of $expected_n'"; sed 's/^/        /' <<<"$out"; fi
node -e 'const d=JSON.parse(require("fs").readFileSync(process.argv[1]));if(d.arch!=="x86_64"&&d.arch!=="aarch64")process.exit(1)' "$real_out" && ok "its arch is recorded" || bad "no arch"
# The check is to refuse a directory that is not a build.
mkdir -p "$workroot/empty"
out="$(BURROW_SIZE_LINES_DIST="$workroot/empty" "$checker" --out "$workroot/e.json" 2>&1)" && status=0 || status=$?
if [ "$status" -ne 0 ]; then ok "an empty dist is refused"; else bad "an empty dist was not refused"; fi
out="$(BURROW_SIZE_LINES_DIST="$workroot/nonexistent" "$checker" --out "$workroot/e.json" 2>&1)" && status=0 || status=$?
if [ "$status" -ne 0 ] && grep -qF "no build at" <<<"$out"; then ok "a missing dist is refused, naming it"; else bad "a missing dist was not refused by name"; fi

echo
if [ "$fail" -ne 0 ]; then echo "FAILED — $fail case(s) failed, $pass passed" >&2; exit 1; fi
echo "OK — $pass case(s) all behaved as required"
