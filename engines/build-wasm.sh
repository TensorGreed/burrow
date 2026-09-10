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

for f in pdfium-wasm.tgz qpdf-12.4.1.tar.gz zlib-1.3.2.tar.gz libjpeg-turbo-3.2.0.tar.gz; do
  [ -f "$vendor/$f" ] || { echo "build-wasm: $f missing -- run engines/fetch.sh first" >&2; exit 1; }
done

# emsdk is a SHIPPED dependency (it contributes musl libc, compiler-rt and the JS glue),
# pinned in engines/pins.toml. Locate it without guessing silently.
EMSDK_DIR="${EMSDK:-$HOME/.local/share/emsdk}"
[ -f "$EMSDK_DIR/emsdk_env.sh" ] || {
  echo "build-wasm: emsdk not found at $EMSDK_DIR" >&2
  echo "  git clone --depth 1 https://github.com/emscripten-core/emsdk.git $EMSDK_DIR" >&2
  echo "  $EMSDK_DIR/emsdk install latest && $EMSDK_DIR/emsdk activate latest" >&2
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

# ---------------------------------------------------------------------------------
say "zlib + libjpeg-turbo for wasm"
# Vendored, not Emscripten ports: the port shipped IJG libjpeg 9f while describing itself
# as "BSD license", and using our own copies keeps native and wasm on the same versions.
rm -rf "$src/zlib-1.3.2"
tar xzf "$vendor/zlib-1.3.2.tar.gz" -C "$src"
rm -rf "$src/libjpeg-turbo-3.2.0"
tar xzf "$vendor/libjpeg-turbo-3.2.0.tar.gz" -C "$src"

emcmake cmake -S "$src/zlib-1.3.2" -B "$src/build-zlib-wasm" \
  -DCMAKE_BUILD_TYPE=Release -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >"$src/zlib-wasm.log" 2>&1
cmake --build "$src/build-zlib-wasm" -j "$jobs" >>"$src/zlib-wasm.log" 2>&1
cmake --install "$src/build-zlib-wasm" >>"$src/zlib-wasm.log" 2>&1
echo "   libz.a $(stat -c%s "$prefix/lib/libz.a") bytes"

emcmake cmake -S "$src/libjpeg-turbo-3.2.0" -B "$src/build-jpeg-wasm" \
  -DCMAKE_BUILD_TYPE=Release -DENABLE_SHARED=OFF -DENABLE_STATIC=ON \
  -DWITH_TURBOJPEG=OFF -DWITH_SIMD=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >"$src/jpeg-wasm.log" 2>&1
cmake --build "$src/build-jpeg-wasm" -j "$jobs" >>"$src/jpeg-wasm.log" 2>&1
cmake --install "$src/build-jpeg-wasm" >>"$src/jpeg-wasm.log" 2>&1
echo "   libjpeg.a $(stat -c%s "$prefix/lib/libjpeg.a") bytes"

# ---------------------------------------------------------------------------------
say "qpdf 12.4.1 for wasm (native crypto only, exceptions enabled)"
rm -rf "$src/qpdf-12.4.1"
tar xzf "$vendor/qpdf-12.4.1.tar.gz" -C "$src"
b="$src/build-qpdf-wasm"
rm -rf "$b"
# -fexceptions is MANDATORY: without it a qpdf throw aborts the whole module rather than
# becoming a typed error. Measured in spike 0001, Finding 3.
#
# PKG_CONFIG_EXECUTABLE is broken on purpose so qpdf cannot find the HOST zlib/libjpeg,
# which would be the wrong ABI entirely for wasm.
emcmake cmake -S "$src/qpdf-12.4.1" -B "$b" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF -DBUILD_STATIC_LIBS=ON \
  -DBUILD_DOC=OFF -DINSTALL_MANUAL=OFF -DINSTALL_EXAMPLES=OFF -DINSTALL_PKGCONFIG=OFF \
  -DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON -DSKIP_OS_SECURE_RANDOM=ON \
  -DPKG_CONFIG_EXECUTABLE=/nonexistent-on-purpose \
  -DCMAKE_PREFIX_PATH="$prefix" \
  -DCMAKE_C_FLAGS="-fexceptions -I$prefix/include" \
  -DCMAKE_CXX_FLAGS="-fexceptions -I$prefix/include" \
  >"$src/qpdf-wasm-configure.log" 2>&1

grep -q "GNU TLS crypto enabled: OFF" "$src/qpdf-wasm-configure.log" || { echo "build-wasm: GnuTLS was NOT disabled" >&2; exit 1; }
grep -q "OpenSSL crypto enabled: OFF"  "$src/qpdf-wasm-configure.log" || { echo "build-wasm: OpenSSL was NOT disabled" >&2; exit 1; }
grep -q "Native crypto enabled: ON"    "$src/qpdf-wasm-configure.log" || { echo "build-wasm: native crypto not enabled" >&2; exit 1; }

cmake --build "$b" --target libqpdf -j "$jobs" >>"$src/qpdf-wasm-configure.log" 2>&1
cp "$b/libqpdf/libqpdf.a" "$prefix/lib/"
cp -R "$src/qpdf-12.4.1/include/qpdf" "$prefix/include/"
echo "   libqpdf.a $(stat -c%s "$prefix/lib/libqpdf.a") bytes"

# ---------------------------------------------------------------------------------
say "qpdf wasm: version probe module"
# The smallest possible module that proves libqpdf.a links under Emscripten and answers a
# call. The real binding is PR 4.
# .cpp with an explicit extern "C": libqpdf is C++ so the link must go through em++,
# and em++ compiles a .c input as C++, which would name-mangle the export and make
# wasm-ld reject `--export=burrow_qpdf_version`.
cat > "$src/qpdf_version_probe.cpp" <<'CPP'
#include <qpdf/qpdf-c.h>
extern "C" char const* burrow_qpdf_version(void) { return qpdf_get_qpdf_version(); }
CPP
em++ -O2 -fexceptions -I "$prefix/include" \
  "$src/qpdf_version_probe.cpp" "$prefix/lib/libqpdf.a" \
  "$prefix/lib/libz.a" "$prefix/lib/libjpeg.a" \
  -sMODULARIZE=1 -sEXPORT_NAME=createQpdfProbe -sENVIRONMENT=web,worker,node \
  -sEXPORTED_FUNCTIONS='["_burrow_qpdf_version"]' \
  -sEXPORTED_RUNTIME_METHODS='["cwrap","UTF8ToString"]' \
  -o "$prefix/lib/qpdf-probe.js" 2>>"$src/qpdf-wasm-configure.log"

cp "$prefix/lib/qpdf-probe.js" "$src/qpdf-probe.cjs"
version="$(node -e '
const createQpdfProbe = require(process.argv[1]);
createQpdfProbe({ locateFile: (f) => process.argv[2] + "/" + f }).then((m) => {
  console.log(m.cwrap("burrow_qpdf_version", "string", [])());
});
' "$src/qpdf-probe.cjs" "$prefix/lib" 2>/dev/null)"
echo "   qpdf reports: ${version:-<no answer>}"
[ "$version" = "12.4.1" ] || { echo "build-wasm: wasm qpdf reported '${version}', expected 12.4.1" >&2; exit 1; }

say "done: $prefix"
