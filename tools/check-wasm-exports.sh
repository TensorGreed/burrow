#!/usr/bin/env bash
# qpdf.wasm's exports must be exactly what core/burrow-engines/src/qpdf/ffi.rs declares,
# minus the functions engines/qpdf-not-exported.toml argues are deliberately absent.
#
# WHY THE EXPECTED SET IS DERIVED FROM ffi.rs
# ===========================================
#
# ffi.rs is the file ADR 0013 governs: every function in it was verified to route through
# qpdf's `trap_errors`, or carries an argued exemption. Deriving the expected export set from
# that file means the web module can export only what the ADR cleared -- and that adding an
# export requires adding a native declaration, where the trapped-function check applies.
#
# An earlier version compared the built module against the same shell variable used to build
# it, so adding a function put it on both sides and the check always passed. It could catch
# emcc silently dropping a symbol and never the failure that matters: a human exporting
# something nothing cleared. Verified at the time by exporting `_qpdf_is_linearized` -- the old
# check reported "20 exports, exactly the declared set".
#
# WHY IT LIVES HERE RATHER THAN IN build-wasm.sh
# ===============================================
#
# Because that is where it was, and CI runs `build-wasm.sh` only on a wasm cache MISS.
# `qpdf_remove_page` was declared natively for split in PR #55, never exported, and stayed on
# main undetected: no PR touched `pins.toml`, `fetch.sh` or `build-wasm.sh`, so the cache stayed
# warm and the check never ran. It surfaced only when rotate's branch edited a COMMENT in
# pins.toml, which changed the cache key.
#
# The second cache-gated check in this repository to hide a real defect. This one reads only
# committed files plus a built `qpdf.wasm` -- present on a warm cache too -- so the `web` job
# runs it on every run, and `build-wasm.sh` calls it as well so a local build still fails fast.
#
# Usage: tools/check-wasm-exports.sh [qpdf.wasm] [ffi.rs] [qpdf-not-exported.toml]
#
# The second and third arguments exist so `tools/test-check-wasm-exports.sh` can point this at
# fixtures and watch each rule fire. Ordinary arguments rather than a test-only backdoor: the
# defaults are the real files, and a caller that passes nothing checks the real thing.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
wasm="${1:-$repo/engines/vendor/wasm/lib/qpdf.wasm}"
ffi_rs="${2:-$repo/core/burrow-engines/src/qpdf/ffi.rs}"
not_exported="${3:-$repo/engines/qpdf-not-exported.toml}"

[ -f "$ffi_rs" ] || { echo "check-wasm-exports: cannot find $ffi_rs" >&2; exit 1; }
[ -f "$not_exported" ] || { echo "check-wasm-exports: cannot find $not_exported" >&2; exit 1; }
[ -f "$wasm" ] || {
  echo "check-wasm-exports: $wasm not found. Run engines/build-wasm.sh, or pass the path." >&2
  exit 1
}

# PROBE THE PARSE BEFORE TRUSTING IT. The `-n` guard below catches a sed that matches NOTHING;
# it cannot catch one that matches the wrong thing -- a looser expression that also picked up a
# commented-out declaration would quietly widen the set this check exists to narrow. So the
# expression runs against fixtures first, positive and negative.
#
# THE CHARACTER CLASS INCLUDES CAPITALS, AND IT DID NOT UNTIL M1 PR B2. Four qpdf C functions
# have one -- qpdf_set_deterministic_ID, qpdf_set_static_ID, qpdf_set_static_aes_IV,
# qpdf_set_suppress_original_object_IDs -- and with `[a-z_0-9]*` this parse could see none of
# them. It fails CLOSED here: the expected set is DERIVED from this parse, so an export the
# parse cannot see is one the module gets refused for having. Noisy beats silent, and it is
# still wrong.
parse_decls() { sed -n 's/^\s*pub(super) fn \(qpdf[A-Za-z_0-9]*\)\s*(.*/\1/p'; }

probe_fail=0
[ "$(printf '    pub(super) fn qpdf_read_memory(\n' | parse_decls)" = "qpdf_read_memory" ] \
  || { echo "check-wasm-exports: the ffi.rs parse does not match its own fixture" >&2; probe_fail=1; }
[ "$(printf '    pub(super) fn qpdf_set_deterministic_ID(\n' | parse_decls)" = "qpdf_set_deterministic_ID" ] \
  || { echo "check-wasm-exports: the ffi.rs parse cannot see a name with a capital letter" >&2; probe_fail=1; }
for bad in '    // pub(super) fn qpdf_is_linearized(' \
           '    /// `qpdf_is_linearized` is deliberately absent' \
           '    pub fn qpdf_is_linearized(' \
           '    pub(super) fn pdfium_load(' ; do
  [ -z "$(printf '%s\n' "$bad" | parse_decls)" ] \
    || { echo "check-wasm-exports: the parse matches a near-miss it must reject: $bad" >&2; probe_fail=1; }
done
[ "$probe_fail" = 0 ] || {
  echo "check-wasm-exports: refusing to derive an expected set from a parse that does not behave as declared" >&2
  exit 1
}

declared="$(parse_decls <"$ffi_rs" | sort -u)"
[ -n "$declared" ] || {
  echo "check-wasm-exports: parsed ZERO functions out of ffi.rs -- the check would be vacuous" >&2
  exit 1
}

# The argued absences. Parsed with the same shape as the file: one `name = "..."` per entry.
absent="$(sed -n 's/^name = "\(qpdf[A-Za-z_0-9]*\)".*/\1/p' "$not_exported" | sort -u)"
entries="$(grep -c '^\[\[function\]\]' "$not_exported" || true)"
names="$(printf '%s\n' "$absent" | grep -c . || true)"
[ "$entries" = "$names" ] || {
  echo "check-wasm-exports: $not_exported has $entries entries but $names parsed names." >&2
  echo "  An entry whose name this cannot read is an absence nobody is checking." >&2
  exit 1
}
# EVERY ENTRY CARRIES AN ARGUMENT, not just a name. An absence with no reason is an exemption
# with no owner, and the file says so at length.
missing_reason="$(awk '/^\[\[function\]\]/{name="";reason=0}
  /^name = /{name=$0}
  /^reason = /{reason=1}
  /^$/{if (name != "" && reason == 0) print name; name=""}
  END{if (name != "" && reason == 0) print name}' "$not_exported")"
[ -z "$missing_reason" ] || {
  echo "check-wasm-exports: an entry in $not_exported has no reason: $missing_reason" >&2
  exit 1
}

# malloc/free are the allocator the bridge needs; qpdf_get_qpdf_version is the version probe.
# Neither parses a PDF, so neither is an ADR 0013 concern.
expected="$(printf '%s\nmalloc\nfree\nqpdf_get_qpdf_version\n' "$declared" \
  | grep -vxF -f <(printf '%s\n' "$absent") | sort -u)"

node -e '
const fs = require("fs");
const m = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
const expected = new Set(fs.readFileSync(process.argv[2], "utf8").split("\n").filter(Boolean));
const absent = new Set(fs.readFileSync(process.argv[3], "utf8").split("\n").filter(Boolean));
// Every export is examined. A prefix filter was used here first, which meant 12 exports --
// the Emscripten and C++ ABI runtime symbols -- were never looked at at all, in a check
// whose whole purpose is to notice an unexpected export. They are benign, so they are
// allowlisted by name rather than skipped by pattern.
const RUNTIME = new Set([
  "__cxa_can_catch", "__cxa_decrement_exception_refcount", "__cxa_get_exception_ptr",
  "__cxa_increment_exception_refcount", "__indirect_function_table", "__wasm_call_ctors",
  "_emscripten_stack_alloc", "_emscripten_stack_restore", "_emscripten_tempret_set",
  "emscripten_stack_get_current", "emscripten_stack_init", "emscripten_stack_get_free",
  "emscripten_stack_get_base", "emscripten_stack_get_end", "memory", "setThrew",
  "__errno_location", "__get_temp_ret", "__set_temp_ret", "stackSave", "stackRestore",
  "stackAlloc",
]);
const got = WebAssembly.Module.exports(m)
  .map((e) => e.name)
  .filter((n) => !RUNTIME.has(n))
  .sort();

const extra = got.filter((n) => !expected.has(n) && !absent.has(n));
const missing = [...expected].filter((n) => !got.includes(n)).sort();
// AND THE THIRD DIRECTION, which is what the argued list makes possible to get wrong: a
// function recorded as deliberately absent that is in fact exported. That is a warning about
// a hazard which is not there, and it teaches people to stop reading the file.
const wrongly_present = got.filter((n) => absent.has(n)).sort();

if (extra.length) {
  console.error("  EXPORTED BUT NOT DECLARED NATIVELY (so not cleared by ADR 0013): " + extra.join(", "));
}
if (missing.length) {
  console.error("  DECLARED NATIVELY BUT NOT EXPORTED, and not argued in engines/qpdf-not-exported.toml: " + missing.join(", "));
}
if (wrongly_present.length) {
  console.error("  RECORDED AS DELIBERATELY NOT EXPORTED, BUT IS EXPORTED: " + wrongly_present.join(", "));
  console.error("  Remove the entry from engines/qpdf-not-exported.toml -- the web can call it.");
}
if (extra.length || missing.length || wrongly_present.length) process.exit(1);
console.log("   " + got.length + " exports, matching ffi.rs exactly");
console.log("   " + absent.size + " declared natively and deliberately not exported (argued in engines/qpdf-not-exported.toml):");
for (const name of [...absent].sort()) console.log("     " + name);
' "$wasm" <(printf '%s\n' "$expected") <(printf '%s\n' "$absent") || {
  echo "check-wasm-exports: qpdf.wasm's exports and the native FFI declarations disagree." >&2
  echo "  These must be the same set, minus the argued absences: the native and web paths call" >&2
  echo "  the same C API, and ADR 0013 cleared exactly that set for crossing back into Rust" >&2
  echo "  without unwinding. A declaration the web cannot use yet belongs in" >&2
  echo "  engines/qpdf-not-exported.toml with the operation it is for and why." >&2
  exit 1
}

# AND THE STALE-ENTRY DIRECTION. An absence recorded for a function nobody declares natively is
# dead weight that reads as coverage -- the same failure qpdf-untrapped-accepted.toml guards.
stale="$(printf '%s\n' "$absent" | grep -vxF -f <(printf '%s\n' "$declared") || true)"
[ -z "$stale" ] || {
  echo "check-wasm-exports: $not_exported names functions ffi.rs does not declare: $stale" >&2
  echo "  Remove the entries; there is nothing to be absent." >&2
  exit 1
}

echo "   ffi.rs declares $(printf '%s\n' "$declared" | wc -l | tr -d ' ') qpdf functions (parse verified against 2 fixtures and 4 near-misses)"
