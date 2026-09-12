#!/usr/bin/env bash
# Build the wasm engine artifacts.
#
# PDFium is not built here: the prebuilt package IS the artifact (there is no wasm static
# archive to link -- see ADR 0006). We only unpack it and assert it still exports the API
# we depend on, so an upstream bump that quietly drops symbols fails here rather than in
# the browser.
#
# qpdf IS built here, from the same checksum-verified source as the native build, so both
# paths link the same qpdf version. Same for zlib and libjpeg-turbo, which are vendored
# rather than taken from Emscripten's ports.
#
# WASM *linking* into a worker is M1 PR 4. This script only produces and checks artifacts.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
vendor="$here/vendor"
src="$vendor/src"
prefix="$vendor/wasm"
jobs="$(nproc)"

# Versions come from pins.toml, never from literals here -- see engines/pins.sh.
# shellcheck source=engines/pins.sh
source "$here/pins.sh"
QPDF_VERSION="$(pins_get qpdf.version)" || { echo "$(basename "$0"): cannot read qpdf.version from pins.toml" >&2; exit 1; }
ZLIB_VERSION="$(pins_get zlib.version)" || { echo "$(basename "$0"): cannot read zlib.version from pins.toml" >&2; exit 1; }
JPEG_VERSION="$(pins_get libjpeg-turbo.version)" || { echo "$(basename "$0"): cannot read libjpeg-turbo.version from pins.toml" >&2; exit 1; }

for f in pdfium-wasm.tgz qpdf-$QPDF_VERSION.tar.gz zlib-$ZLIB_VERSION.tar.gz libjpeg-turbo-$JPEG_VERSION.tar.gz; do
  [ -f "$vendor/$f" ] || { echo "build-wasm: $f missing -- run engines/fetch.sh first" >&2; exit 1; }
done

# emsdk is a SHIPPED dependency (it contributes musl libc, compiler-rt and the JS glue),
# pinned in engines/pins.toml. Locate it without guessing silently.
EMSDK_DIR="${EMSDK:-$HOME/.local/share/emsdk}"
[ -f "$EMSDK_DIR/emsdk_env.sh" ] || {
  echo "build-wasm: emsdk not found at $EMSDK_DIR" >&2
  echo "  git clone --depth 1 https://github.com/emscripten-core/emsdk.git $EMSDK_DIR" >&2
  echo "  $EMSDK_DIR/emsdk install $(sed -n '/^\[emsdk\]/,/^\[/p' "$here/pins.toml" | sed -n 's/^version = "\(.*\)"/\1/p')" >&2
  echo "  $EMSDK_DIR/emsdk activate <that same version>" >&2
  exit 1
}
# shellcheck disable=SC1091
source "$EMSDK_DIR/emsdk_env.sh" >/dev/null 2>&1

want_emsdk="$(sed -n '/^\[emsdk\]/,/^\[/p' "$here/pins.toml" | sed -n 's/^version = "\(.*\)"/\1/p')"
have_emsdk="$(emcc --version 2>/dev/null | sed -n '1s/.*) \([0-9.]*\).*/\1/p')"
if [ -n "$want_emsdk" ] && [ "$want_emsdk" != "$have_emsdk" ]; then
  echo "build-wasm: emsdk version mismatch -- pins.toml wants $want_emsdk, found ${have_emsdk:-unknown}" >&2
  echo "  Emscripten contributes code to the shipped artifact, so this is not cosmetic." >&2
  exit 1
fi

rm -rf "$prefix"
mkdir -p "$prefix/lib" "$prefix/include" "$src"

say() { printf '\n== %s\n' "$1"; }

# Surface the build logs when something fails.
#
# Every cmake and em++ invocation below redirects to a log file, which keeps the normal
# output readable -- and meant a failure in CI printed the section heading and then nothing
# at all. `set -e` exits, the log stays on a machine nobody can reach, and the only
# information is an exit code. Diagnosing that took a round trip through CI that should not
# have been necessary.
dump_logs_on_failure() {
  local status=$?
  [ "$status" -eq 0 ] && return 0
  echo "" >&2
  echo "build-wasm: FAILED (exit $status). Tail of every build log:" >&2
  for log in "$src"/*.log; do
    [ -f "$log" ] || continue
    echo "" >&2
    echo "---- $(basename "$log") ----" >&2
    tail -40 "$log" >&2
  done
  return "$status"
}
trap dump_logs_on_failure EXIT

# ---------------------------------------------------------------------------------
say "PDFium wasm: unpack the prebuilt and check its exports"
pd="$src/pdfium-wasm"
rm -rf "$pd"; mkdir -p "$pd"
tar xzf "$vendor/pdfium-wasm.tgz" -C "$pd"
cp "$pd/lib/pdfium.js" "$pd/lib/pdfium.wasm" "$prefix/lib/"
cp -R "$pd/include" "$prefix/include/pdfium"
cp -R "$pd/licenses" "$prefix/licenses-pdfium"
cp "$pd/LICENSE" "$prefix/licenses-pdfium/PACKAGING-LICENSE.txt"

# Regression guard: the module must still export the FPDF_* surface. There is no
# EXPORTED_FUNCTIONS setting in upstream's wasm patches, so this is not guaranteed by
# anything except their build happening to keep it -- worth asserting.
exports="$(node -e '
const fs = require("fs");
const m = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
console.log(WebAssembly.Module.exports(m).filter(e => /^FPDF/.test(e.name)).length);
' "$prefix/lib/pdfium.wasm")"
echo "   FPDF_* exports: $exports"
[ "$exports" -ge 400 ] || {
  echo "build-wasm: pdfium.wasm exports only $exports FPDF_* symbols (expected >= 400)." >&2
  echo "  The prebuilt module's API surface shrank. Do not proceed." >&2
  exit 1
}
for sym in FPDF_InitLibrary FPDF_LoadMemDocument FPDF_GetPageCount FPDF_GetLastError; do
  node -e '
  const fs = require("fs");
  const m = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
  const has = WebAssembly.Module.exports(m).some(e => e.name === process.argv[2]);
  process.exit(has ? 0 : 1);
  ' "$prefix/lib/pdfium.wasm" "$sym" || { echo "build-wasm: $sym is not exported" >&2; exit 1; }
done
echo "   required entry points present"

# The bundle's correctness depends on ONE line of upstream's glue.
#
# All worker code is concatenated into a single script (see tools/stage-web-engines.mjs and
# ADR 0014), and `prelude.js` assigns `self.Module` before pdfium.js runs so that
# `instantiateWasm` is in place. That survives only because pdfium.js writes
#
#   var Module = typeof Module != "undefined" ? Module : {}
#
# `var` hoisting creates the binding before any of the bundle executes, but `var x = v` only
# ASSIGNS when its initialiser runs -- by which point the prelude has set the global, and the
# conditional keeps it. If upstream ever simplified this to `var Module = {}`, our
# configuration would be silently discarded: `instantiateWasm` would be gone, pdfium would
# fall back to fetching its own .wasm relative to an opaque blob: URL, and the failure would
# surface in a browser rather than here.
grep -q 'var Module=typeof Module!="undefined"?Module:{}' "$prefix/lib/pdfium.js" || {
  echo "build-wasm: pdfium.js no longer preserves a pre-existing Module." >&2
  echo "  The worker bundle assigns self.Module before this glue runs, and relies on the" >&2
  echo "  conditional form to keep it. Re-check prelude.js against the new glue." >&2
  exit 1
}
echo "   the glue still honours a pre-existing Module"

# ---------------------------------------------------------------------------------
say "zlib + libjpeg-turbo for wasm"
# Vendored, not Emscripten ports: the port shipped IJG libjpeg 9f while describing itself
# as "BSD license", and using our own copies keeps native and wasm on the same versions.
rm -rf "$src/zlib-$ZLIB_VERSION"
tar xzf "$vendor/zlib-$ZLIB_VERSION.tar.gz" -C "$src"
rm -rf "$src/libjpeg-turbo-$JPEG_VERSION"
tar xzf "$vendor/libjpeg-turbo-$JPEG_VERSION.tar.gz" -C "$src"

emcmake cmake -S "$src/zlib-$ZLIB_VERSION" -B "$src/build-zlib-wasm" \
  -DCMAKE_BUILD_TYPE=Release -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >"$src/zlib-wasm.log" 2>&1
cmake --build "$src/build-zlib-wasm" -j "$jobs" >>"$src/zlib-wasm.log" 2>&1
cmake --install "$src/build-zlib-wasm" >>"$src/zlib-wasm.log" 2>&1
echo "   libz.a $(stat -c%s "$prefix/lib/libz.a") bytes"

emcmake cmake -S "$src/libjpeg-turbo-$JPEG_VERSION" -B "$src/build-jpeg-wasm" \
  -DCMAKE_BUILD_TYPE=Release -DENABLE_SHARED=OFF -DENABLE_STATIC=ON \
  -DWITH_TURBOJPEG=OFF -DWITH_SIMD=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >"$src/jpeg-wasm.log" 2>&1
cmake --build "$src/build-jpeg-wasm" -j "$jobs" >>"$src/jpeg-wasm.log" 2>&1
cmake --install "$src/build-jpeg-wasm" >>"$src/jpeg-wasm.log" 2>&1
echo "   libjpeg.a $(stat -c%s "$prefix/lib/libjpeg.a") bytes"

# ---------------------------------------------------------------------------------
say "qpdf $QPDF_VERSION for wasm (native crypto only, exceptions enabled)"
rm -rf "$src/qpdf-$QPDF_VERSION"
tar xzf "$vendor/qpdf-$QPDF_VERSION.tar.gz" -C "$src"
b="$src/build-qpdf-wasm"
rm -rf "$b"
# -fexceptions is MANDATORY: without it a qpdf throw aborts the whole module rather than
# becoming a typed error. Measured in spike 0001, Finding 3.
#
# PKG_CONFIG_EXECUTABLE is broken on purpose so qpdf cannot find the HOST zlib/libjpeg,
# which would be the wrong ABI entirely for wasm.
#
# THE FOUR PATHS ARE PASSED EXPLICITLY, AND THAT IS NOT BELT AND BRACES.
#
# With pkg-config broken, qpdf falls back to `find_path(zlib.h)` / `find_library(z)`
# (libqpdf/CMakeLists.txt:158-160, :178-179). Under the Emscripten toolchain those search
# the emsdk SYSROOT, not CMAKE_PREFIX_PATH -- so on a machine whose emsdk cache happens to
# have the zlib and libjpeg PORTS built, qpdf silently configures against
# `emsdk/upstream/emscripten/cache/sysroot/`, and on a fresh emsdk it finds nothing and the
# configure fails with "zlib not found".
#
# That is exactly what happened: the build worked on a developer machine and failed in CI,
# and the developer machine was the one that was wrong. Measured from its CMakeCache:
#
#   ZLIB_LIB_PATH  = .../emsdk/upstream/emscripten/cache/sysroot/lib/wasm32-emscripten/libz.a
#   LIBJPEG_H_PATH = .../emsdk/upstream/emscripten/cache/sysroot/include
#
# Using the ports is precisely what the comment above this block forbids, and it is not only
# a version question: engines/licenses.toml declares OUR libjpeg-turbo 3.2.0 for the wasm
# artifacts, while Emscripten's port is IJG libjpeg 9f described upstream as "BSD license".
# A licence manifest that names a component the artifact was not built against is worse than
# no manifest.
#
# Setting the four cache variables makes CMake skip the search entirely.
emcmake cmake -S "$src/qpdf-$QPDF_VERSION" -B "$b" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF -DBUILD_STATIC_LIBS=ON \
  -DBUILD_DOC=OFF -DINSTALL_MANUAL=OFF -DINSTALL_EXAMPLES=OFF -DINSTALL_PKGCONFIG=OFF \
  -DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON -DSKIP_OS_SECURE_RANDOM=ON \
  -DPKG_CONFIG_EXECUTABLE=/nonexistent-on-purpose \
  -DCMAKE_PREFIX_PATH="$prefix" \
  -DZLIB_H_PATH="$prefix/include" \
  -DZLIB_LIB_PATH="$prefix/lib/libz.a" \
  -DLIBJPEG_H_PATH="$prefix/include" \
  -DLIBJPEG_LIB_PATH="$prefix/lib/libjpeg.a" \
  -DCMAKE_C_FLAGS="-fexceptions -I$prefix/include" \
  -DCMAKE_CXX_FLAGS="-fexceptions -I$prefix/include" \
  >"$src/qpdf-wasm-configure.log" 2>&1

# Assert qpdf configured against OUR zlib and libjpeg, not Emscripten's ports or the host's.
# The paths are passed in above; this proves CMake honoured them rather than overriding them,
# and it is the check whose absence let a developer machine and CI disagree for a whole
# afternoon.
for var in ZLIB_H_PATH ZLIB_LIB_PATH LIBJPEG_H_PATH LIBJPEG_LIB_PATH; do
  resolved="$(sed -n "s/^$var:[A-Z]*=//p" "$b/CMakeCache.txt")"
  case "$resolved" in
    "$prefix"/*) ;;
    *)
      echo "build-wasm: qpdf configured $var to '$resolved'" >&2
      echo "  That is outside $prefix, so it is Emscripten's port or the host's copy --" >&2
      echo "  neither of which is the checksum-verified source engines/licenses.toml declares." >&2
      exit 1
      ;;
  esac
done
echo "   zlib and libjpeg resolved inside the vendored prefix"

grep -q "GNU TLS crypto enabled: OFF" "$src/qpdf-wasm-configure.log" || { echo "build-wasm: GnuTLS was NOT disabled" >&2; exit 1; }
grep -q "OpenSSL crypto enabled: OFF"  "$src/qpdf-wasm-configure.log" || { echo "build-wasm: OpenSSL was NOT disabled" >&2; exit 1; }
grep -q "Native crypto enabled: ON"    "$src/qpdf-wasm-configure.log" || { echo "build-wasm: native crypto not enabled" >&2; exit 1; }

cmake --build "$b" --target libqpdf -j "$jobs" >>"$src/qpdf-wasm-configure.log" 2>&1
cp "$b/libqpdf/libqpdf.a" "$prefix/lib/"
cp -R "$src/qpdf-$QPDF_VERSION/include/qpdf" "$prefix/include/"
echo "   libqpdf.a $(stat -c%s "$prefix/lib/libqpdf.a") bytes"

# ---------------------------------------------------------------------------------
say "qpdf $QPDF_VERSION for wasm: the shipping engine module"
# The shipping qpdf engine module.
#
# EXPORTED_FUNCTIONS IS AN ALLOWLIST, AND THAT IS THE POINT. It is the C API surface
# `core/burrow-engines/src/qpdf/ffi.rs` declares, plus the allocator (_malloc, _free) and
# _qpdf_get_qpdf_version for the probe below -- and nothing else. A function that is not
# exported cannot be called from JS at all, which is the same "strongest available form"
# argument that keeps qpdf's message accessors undeclared on the native side.
#
# Whether those declarations are themselves safe -- routed through qpdf's `trap_errors`, or
# justified as non-parsing -- is checked from qpdf's own source by
# tools/check-qpdf-trapped.py (M1 PR 4c). This allowlist and that check are the two ends of
# the same rule: this one bounds what the module exposes, that one bounds what we may ask for.
#
# Notably absent: _qpdf_is_encrypted and _qpdf_is_linearized. Neither is trapped, and an
# object number above INT_MAX makes the latter throw std::range_error straight out of the C
# API. On native that aborts the process; in a browser it surfaces as a JS exception at the
# bridge, which is quieter but no safer.
#
# .cpp with an explicit extern "C": libqpdf is C++, so the link must go through em++, and
# em++ compiles a .c input as C++ -- which would name-mangle the exports.
qpdf_exports='"_malloc","_free",
  "_qpdf_init","_qpdf_cleanup","_qpdf_silence_errors","_qpdf_set_suppress_warnings",
  "_qpdf_set_logger","_qpdf_set_attempt_recovery","_qpdf_read_memory",
  "_qpdf_has_error","_qpdf_get_error","_qpdf_get_error_code","_qpdf_get_num_pages",
  "_qpdf_global_set_uint32","_qpdf_get_qpdf_version",
  "_qpdflogger_create","_qpdflogger_set_info","_qpdflogger_set_warn","_qpdflogger_set_error"'

# A translation unit that references the C API, so wasm-ld keeps the archive members. The
# -u flags emcc derives from EXPORTED_FUNCTIONS do the real work; this makes the intent
# legible and gives the version probe below something to call.
cat > "$src/qpdf_engine.cpp" <<'CPP'
#include <qpdf/qpdf-c.h>
#include <qpdf/qpdflogger-c.h>
CPP

# Flags that are load-bearing, each for a reason that is expensive to rediscover:
#
#   --no-entry            there is no main(); without it wasm-ld errors.
#   -fexceptions          MUST match how libqpdf.a was built. -fwasm-exceptions is a
#                         different model and will not link against it (spike 0001's
#                         __resumeException failure is the same class).
#   -sWASM_BIGINT=1       so qpdf_read_memory's `unsigned long long size` takes one BigInt
#                         rather than Emscripten's legalized (lo, hi) i32 pair. A manual
#                         64-bit split in JS would be arithmetic in the binding layer.
#   -sMAXIMUM_MEMORY=2GB  not only a ceiling: it keeps every heap address below 2^31, so an
#                         i32 pointer never reaches JS as a negative number.
#   -sENVIRONMENT=web,worker   drops the Node branch's require("fs") and sync-XHR paths.
#                         ADR 0006 requirement 3. `node` is deliberately NOT in the list.
#   -sFILESYSTEM=0        there are no files. This is a privacy-first library.
em++ -O2 -fexceptions --no-entry -I "$prefix/include" \
  "$src/qpdf_engine.cpp" "$prefix/lib/libqpdf.a" \
  "$prefix/lib/libz.a" "$prefix/lib/libjpeg.a" \
  -sMODULARIZE=1 -sEXPORT_NAME=createQpdfModule -sENVIRONMENT=web,worker \
  -sWASM_BIGINT=1 -sALLOW_MEMORY_GROWTH=1 -sINITIAL_MEMORY=16MB -sMAXIMUM_MEMORY=2GB \
  -sFILESYSTEM=0 -sINVOKE_RUN=0 -sEXIT_RUNTIME=0 \
  -sEXPORTED_FUNCTIONS="[$qpdf_exports]" \
  -sEXPORTED_RUNTIME_METHODS='["HEAPU8","HEAPU32","stackSave","stackAlloc","stackRestore"]' \
  -o "$prefix/lib/qpdf.js" 2>>"$src/qpdf-wasm-configure.log"

echo "   qpdf.js   $(stat -c%s "$prefix/lib/qpdf.js") bytes"
echo "   qpdf.wasm $(stat -c%s "$prefix/lib/qpdf.wasm") bytes"

# ---------------------------------------------------------------------------------
say "qpdf wasm: the export surface must match the native FFI declarations"
# THE EXPECTED SET IS DERIVED FROM core/burrow-engines/src/qpdf/ffi.rs, NOT FROM THE
# EXPORTED_FUNCTIONS ABOVE.
#
# A first version compared the built module against the same `$qpdf_exports` variable used
# to build it, so adding a function put it on both sides and the check always passed. It
# could only ever catch emcc silently dropping a symbol -- never the failure that matters,
# which is a human exporting something ADR 0013 never cleared. Verified by exporting
# _qpdf_is_linearized: the old check reported "20 exports, exactly the declared set".
#
# So the two tables are cross-checked instead. ffi.rs is the file ADR 0013 governs -- every
# function in it was verified to route through qpdf's `trap_errors` by reading qpdf-c.cc --
# and the web module may export exactly those, plus the allocator and the version string.
# Adding _qpdf_is_linearized here now fails unless it is also declared natively, where the
# ADR applies. (M1 PR 4c closes the other end: generating the trapped set from qpdf's own
# source and checking ffi.rs against it.)
ffi_rs="$here/../core/burrow-engines/src/qpdf/ffi.rs"
[ -f "$ffi_rs" ] || { echo "build-wasm: cannot find $ffi_rs to derive the allowlist from" >&2; exit 1; }
declared="$(sed -n 's/^\s*pub(super) fn \(qpdf[a-z_0-9]*\)\s*(.*/\1/p' "$ffi_rs" | sort -u)"
[ -n "$declared" ] || { echo "build-wasm: parsed ZERO functions out of ffi.rs -- the check would be vacuous" >&2; exit 1; }
echo "   ffi.rs declares $(printf '%s\n' "$declared" | wc -l) qpdf functions"

# malloc/free are the allocator the bridge needs; qpdf_get_qpdf_version is the probe below.
# Neither parses a PDF, so neither is an ADR 0013 concern.
allowed="$(printf '%s\nmalloc\nfree\nqpdf_get_qpdf_version\n' "$declared" | sort -u)"

node -e '
const fs = require("fs");
const m = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
const allowed = new Set(fs.readFileSync(process.argv[2], "utf8").split("\n").filter(Boolean));
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
const extra = got.filter((n) => !allowed.has(n));
const missing = [...allowed].filter((n) => !got.includes(n)).sort();
if (extra.length) {
  console.error("  EXPORTED BUT NOT DECLARED NATIVELY (so not cleared by ADR 0013): " + extra.join(", "));
}
if (missing.length) {
  console.error("  DECLARED NATIVELY BUT NOT EXPORTED (the web path cannot call it): " + missing.join(", "));
}
if (extra.length || missing.length) process.exit(1);
console.log("   " + got.length + " exports, matching ffi.rs exactly");
' "$prefix/lib/qpdf.wasm" <(printf '%s\n' "$allowed") || {
  echo "build-wasm: qpdf.wasm's exports and the native FFI declarations disagree." >&2
  echo "  These must be the same set: the native and web paths call the same C API, and" >&2
  echo "  ADR 0013 cleared exactly that set for crossing back into Rust without unwinding." >&2
  exit 1
}

# NEITHER module may have an async or threaded runtime: the bridge calls every Emscripten
# export synchronously from Rust, and Asyncify or JSPI would make each one return a Promise,
# which `DocumentEngine` (a sync trait) cannot accommodate.
#
# BOTH artifacts are checked, not just ours. An earlier version ran these over qpdf.js only
# -- the one file that cannot change without us changing it -- and skipped pdfium.js, the
# third-party prebuilt that is re-fetched on every bump and is the only one that could
# acquire any of this without anyone noticing.
for artifact in qpdf.js pdfium.js; do
  for forbidden in Asyncify _emscripten_proxy pthread_create SharedArrayBuffer; do
    ! grep -q "$forbidden" "$prefix/lib/$artifact" || {
      echo "build-wasm: $artifact contains '$forbidden' -- the synchronous bridge assumption is broken" >&2
      exit 1
    }
  done
  # And neither may reach the network. The worker supplies the wasm bytes itself.
  #
  # pdfium.js DOES still contain `fetch(...)`, `XMLHttpRequest` and `require("fs")` -- it is
  # an unmodified prebuilt, and we never applied -sENVIRONMENT=web,worker to it. None of
  # that runs, because `instantiateWasm` pre-empts the fetch and a worker is not Node. What
  # is checked here is narrower and still worth checking: no ABSOLUTE URL, so there is no
  # host baked into the artifact for any of that machinery to reach.
  ! grep -qE "https?://[a-zA-Z0-9]" "$prefix/lib/$artifact" || {
    echo "build-wasm: $artifact contains an absolute URL -- non-negotiable #1" >&2
    exit 1
  }
done
echo "   neither module has an async runtime, threads, or an absolute URL"

# ---------------------------------------------------------------------------------
say "qpdf wasm: the artifact that ships is the one that answers"
# Instantiated through Module.instantiateWasm, supplying the bytes ourselves -- which is
# exactly how the worker does it under `connect-src` scoped to the engine URLs, and the
# reason this probe cannot simply let the glue fetch: -sENVIRONMENT=web,worker has (rightly)
# removed the Node file-reading path, so a `locateFile` filesystem path no longer resolves.
# Verifying the hook here means a PDFium or emsdk bump that breaks it fails the build rather
# than the browser.
cp "$prefix/lib/qpdf.js" "$src/qpdf-engine.cjs"
version="$(node -e '
const fs = require("fs");
const createQpdfModule = require(process.argv[1]);
const bytes = fs.readFileSync(process.argv[2]);
createQpdfModule({
  instantiateWasm(imports, done) {
    WebAssembly.instantiate(bytes, imports).then((r) => done(r.instance, r.module));
    return {};
  },
}).then((m) => {
  // Read straight out of the heap rather than through cwrap and UTF8ToString. Those were
  // exported solely for this probe, and shipping a generic "read a NUL-terminated string
  // out of the qpdf heap" helper sits badly beside the care taken to keep the qpdf message
  // accessors unreachable. (No apostrophes in here: the whole script is inside a shell
  // single-quoted string, and one closes it.)
  const ptr = m._qpdf_get_qpdf_version();
  let end = ptr;
  while (m.HEAPU8[end] !== 0) end += 1;
  console.log(new TextDecoder().decode(m.HEAPU8.subarray(ptr, end)));
});
' "$src/qpdf-engine.cjs" "$prefix/lib/qpdf.wasm" 2>&1 | tail -1)"
echo "   qpdf reports: ${version:-<no answer>}"
[ "$version" = "$QPDF_VERSION" ] || { echo "build-wasm: wasm qpdf reported '${version}', expected $QPDF_VERSION" >&2; exit 1; }

say "done: $prefix"
