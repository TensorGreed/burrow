#!/usr/bin/env bash
# Build the native engine libraries for the host architecture.
#
# Order matters: zlib and libjpeg-turbo first, then qpdf against them. We vendor both
# rather than using the host's, so native and wasm builds link the SAME versions -- one
# fewer divergence source for the differential conformance harness (M1 item 12).
#
# Produces, under engines/vendor/native-<arch>/:
#   lib/libpdfium.so   (extracted from the prebuilt; NOT built here)
#   lib/libz.a, lib/libjpeg.a, lib/libqpdf.a
#   lib/fuzz/libqpdf.a (ASan+fuzzer instrumented, for M1 PR 2's fuzz targets)
#   include/           headers for all of the above
#
# Requires engines/fetch.sh to have run: every input is checksum-verified there, and
# nothing here touches the network.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
vendor="$here/vendor"
src="$vendor/src"
arch="$(uname -m)"
prefix="$vendor/native-$arch"
jobs="$(nproc)"

case "$arch" in
  aarch64) pdfium_pkg="pdfium-linux-arm64.tgz" ;;
  x86_64)  pdfium_pkg="pdfium-linux-x64.tgz" ;;
  *) echo "build-native: unsupported arch $arch (only aarch64 and x86_64 are pinned)" >&2; exit 1 ;;
esac

for f in "$pdfium_pkg" qpdf-12.4.1.tar.gz zlib-1.3.2.tar.gz libjpeg-turbo-3.2.0.tar.gz; do
  [ -f "$vendor/$f" ] || { echo "build-native: $f missing -- run engines/fetch.sh first" >&2; exit 1; }
done

# clang is required for the instrumented qpdf: -fsanitize=fuzzer-no-link is clang-only.
command -v clang >/dev/null || { echo "build-native: clang required (apt install clang)" >&2; exit 1; }
command -v cmake >/dev/null || { echo "build-native: cmake required" >&2; exit 1; }

rm -rf "$prefix"
mkdir -p "$prefix/lib/fuzz" "$prefix/include" "$src"

say() { printf '\n== %s\n' "$1"; }

# ---------------------------------------------------------------------------------
say "PDFium: extract the prebuilt shared library"
# Not built here. No static archive is published for any platform, so native linkage is
# dynamic -- see ADR 0004. The .so is checksum-verified by fetch.sh and re-verified by
# core/burrow-engines/build.rs before it is linked.
pd="$src/pdfium-$arch"
rm -rf "$pd"; mkdir -p "$pd"
tar xzf "$vendor/$pdfium_pkg" -C "$pd"
cp "$pd/lib/libpdfium.so" "$prefix/lib/"
cp -R "$pd/include" "$prefix/include/pdfium"
# Keep the licence set: it is per-artifact (08-licenses.sh derives it from build.ninja),
# and engines/licenses.toml is audited against these files.
cp -R "$pd/licenses" "$prefix/licenses-pdfium"
cp "$pd/LICENSE" "$prefix/licenses-pdfium/PACKAGING-LICENSE.txt"
echo "   libpdfium.so $(stat -c%s "$prefix/lib/libpdfium.so") bytes"

# ---------------------------------------------------------------------------------
say "zlib 1.3.2 (static)"
tar xzf "$vendor/zlib-1.3.2.tar.gz" -C "$src"
cmake -S "$src/zlib-1.3.2" -B "$src/build-zlib-$arch" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >/dev/null
cmake --build "$src/build-zlib-$arch" -j "$jobs" >/dev/null
cmake --install "$src/build-zlib-$arch" >/dev/null
# Some zlib versions install libz.a under a versioned name; normalise.
[ -f "$prefix/lib/libz.a" ] || cp "$src/build-zlib-$arch"/libz.a "$prefix/lib/" 2>/dev/null || true
echo "   libz.a $(stat -c%s "$prefix/lib/libz.a") bytes"

# ---------------------------------------------------------------------------------
say "libjpeg-turbo 3.2.0 (static)"
# libjpeg-TURBO, not IJG libjpeg 9f: maintained, SIMD, same codebase PDFium bundles.
tar xzf "$vendor/libjpeg-turbo-3.2.0.tar.gz" -C "$src"
cmake -S "$src/libjpeg-turbo-3.2.0" -B "$src/build-jpeg-$arch" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DENABLE_SHARED=OFF -DENABLE_STATIC=ON -DWITH_TURBOJPEG=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >/dev/null
cmake --build "$src/build-jpeg-$arch" -j "$jobs" >/dev/null
cmake --install "$src/build-jpeg-$arch" >/dev/null
echo "   libjpeg.a $(stat -c%s "$prefix/lib/libjpeg.a") bytes"

# ---------------------------------------------------------------------------------
build_qpdf() { # variant extra_c_flags extra_cxx_flags outdir
  local variant="$1" cflags="$2" cxxflags="$3" outdir="$4"
  local b="$src/build-qpdf-$variant-$arch"
  rm -rf "$b"
  # PKG_CONFIG_EXECUTABLE is broken on purpose: qpdf's pkg-config path would find the
  # HOST zlib/libjpeg, defeating the point of vendoring. Failing it forces the
  # find_path/find_library fallback, which we point at our prefix.
  #
  # USE_IMPLICIT_CRYPTO=OFF + REQUIRE_CRYPTO_NATIVE=ON is LOAD-BEARING: the upstream
  # default is ON and would link GnuTLS, which is LGPL-2.1-or-later. Asserted below.
  cmake -S "$src/qpdf-12.4.1" -B "$b" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_STATIC_LIBS=ON \
    -DBUILD_DOC=OFF -DINSTALL_MANUAL=OFF -DINSTALL_EXAMPLES=OFF -DINSTALL_PKGCONFIG=OFF \
    -DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON \
    -DPKG_CONFIG_EXECUTABLE=/nonexistent-on-purpose \
    -DCMAKE_PREFIX_PATH="$prefix" \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_C_FLAGS="-I$prefix/include $cflags" \
    -DCMAKE_CXX_FLAGS="-I$prefix/include $cxxflags" \
    > "$src/qpdf-$variant-$arch-configure.log" 2>&1 || {
      echo "build-native: qpdf ($variant) configure failed; see $src/qpdf-$variant-$arch-configure.log" >&2
      tail -20 "$src/qpdf-$variant-$arch-configure.log" >&2 || true
      exit 1
    }

  # Fail closed on the crypto question rather than trusting the flags took effect.
  local log="$src/qpdf-$variant-$arch-configure.log"
  grep -q "GNU TLS crypto enabled: OFF" "$log" || { echo "build-native: GnuTLS was NOT disabled" >&2; exit 1; }
  grep -q "OpenSSL crypto enabled: OFF"  "$log" || { echo "build-native: OpenSSL was NOT disabled" >&2; exit 1; }
  grep -q "Native crypto enabled: ON"    "$log" || { echo "build-native: native crypto not enabled" >&2; exit 1; }

  cmake --build "$b" --target libqpdf -j "$jobs" >/dev/null
  cp "$b/libqpdf/libqpdf.a" "$outdir/"
}

say "qpdf 12.4.1 (static, native crypto only)"
tar xzf "$vendor/qpdf-12.4.1.tar.gz" -C "$src"
build_qpdf plain "" "" "$prefix/lib"
cp -R "$src/qpdf-12.4.1/include/qpdf" "$prefix/include/"
echo "   libqpdf.a $(stat -c%s "$prefix/lib/libqpdf.a") bytes"

say "qpdf 12.4.1 (fuzzer + ASan instrumented)"
# For M1 PR 2's fuzz targets. Built here because we build qpdf from source, so we can
# instrument it -- which the prebuilt PDFium cannot be. Unused until PR 2.
build_qpdf fuzz \
  "-fsanitize=fuzzer-no-link,address -fno-omit-frame-pointer" \
  "-fsanitize=fuzzer-no-link,address -fno-omit-frame-pointer" \
  "$prefix/lib/fuzz"
echo "   fuzz/libqpdf.a $(stat -c%s "$prefix/lib/fuzz/libqpdf.a") bytes"

say "done: $prefix"
find "$prefix/lib" -maxdepth 2 -name '*.a' -o -maxdepth 2 -name '*.so' | sort | while read -r f; do
  printf '   %-28s %10s bytes\n' "${f#$prefix/lib/}" "$(stat -c%s "$f")"
done
