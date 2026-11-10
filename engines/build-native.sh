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
#   (each libqpdf.a also holds burrow's shim, burrow_qpdf_fixed_dangling_refs; see write_qpdf_shim)
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

# Versions come from pins.toml, never from literals here -- see engines/pins.sh.
# shellcheck source=engines/pins.sh
source "$here/pins.sh"
QPDF_VERSION="$(pins_get qpdf.version)" || { echo "$(basename "$0"): cannot read qpdf.version from pins.toml" >&2; exit 1; }
ZLIB_VERSION="$(pins_get zlib.version)" || { echo "$(basename "$0"): cannot read zlib.version from pins.toml" >&2; exit 1; }
JPEG_VERSION="$(pins_get libjpeg-turbo.version)" || { echo "$(basename "$0"): cannot read libjpeg-turbo.version from pins.toml" >&2; exit 1; }

# REPRODUCIBLE BUILD DATE (owner (95)(1), 2026-11-02). libjpeg-turbo's CMake stamps
# `string(TIMESTAMP ... "%Y%m%d")` into the library as "build YYYYMMDD", so rebuilding on another
# day changed 3 bytes of qpdf.wasm and moved its brotli size by hundreds of bytes. Pin the date
# to a value derived from the pinned tarball (pins.toml [libjpeg-turbo] build_date), never from
# the clock: -DBUILD for libjpeg-turbo, SOURCE_DATE_EPOCH for everything else that reads a date.
BUILD_DATE="$(pins_get libjpeg-turbo.build_date)" || { echo "$(basename "$0"): cannot read libjpeg-turbo.build_date from pins.toml" >&2; exit 1; }
JPEG_BUILD="$(date -u -d "$BUILD_DATE" +%Y%m%d)" || { echo "$(basename "$0"): bad build_date $BUILD_DATE" >&2; exit 1; }
SOURCE_DATE_EPOCH="$(date -u -d "$BUILD_DATE 00:00:00" +%s)" || { echo "$(basename "$0"): bad build_date $BUILD_DATE" >&2; exit 1; }
export SOURCE_DATE_EPOCH

case "$arch" in
  aarch64) pdfium_pkg="pdfium-linux-arm64.tgz" ;;
  x86_64)  pdfium_pkg="pdfium-linux-x64.tgz" ;;
  *) echo "build-native: unsupported arch $arch (only aarch64 and x86_64 are pinned)" >&2; exit 1 ;;
esac

for f in "$pdfium_pkg" qpdf-$QPDF_VERSION.tar.gz zlib-$ZLIB_VERSION.tar.gz libjpeg-turbo-$JPEG_VERSION.tar.gz; do
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
say "zlib $ZLIB_VERSION (static)"
rm -rf "$src/zlib-$ZLIB_VERSION"
tar xzf "$vendor/zlib-$ZLIB_VERSION.tar.gz" -C "$src"
cmake -S "$src/zlib-$ZLIB_VERSION" -B "$src/build-zlib-$arch" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF \
  -DCMAKE_INSTALL_PREFIX="$prefix" >/dev/null
cmake --build "$src/build-zlib-$arch" -j "$jobs" >/dev/null
cmake --install "$src/build-zlib-$arch" >/dev/null
# Some zlib versions install libz.a under a versioned name; normalise. Fail loudly if
# the fallback cannot find it -- otherwise the `stat` below dies with a bare
# "No such file or directory" and no clue why.
if [ ! -f "$prefix/lib/libz.a" ]; then
  cp "$src/build-zlib-$arch/libz.a" "$prefix/lib/" || {
    echo "build-native: zlib built but libz.a is not where expected" >&2
    exit 1
  }
fi
echo "   libz.a $(stat -c%s "$prefix/lib/libz.a") bytes"

# ---------------------------------------------------------------------------------
say "libjpeg-turbo $JPEG_VERSION (static)"
# libjpeg-TURBO, not IJG libjpeg 9f: maintained, SIMD, same codebase PDFium bundles.
rm -rf "$src/libjpeg-turbo-$JPEG_VERSION"
tar xzf "$vendor/libjpeg-turbo-$JPEG_VERSION.tar.gz" -C "$src"
cmake -S "$src/libjpeg-turbo-$JPEG_VERSION" -B "$src/build-jpeg-$arch" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DENABLE_SHARED=OFF -DENABLE_STATIC=ON -DWITH_TURBOJPEG=OFF \
  -DBUILD="$JPEG_BUILD" \
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
  # find_path/find_library fallback, and the four paths below point that fallback at our
  # prefix EXPLICITLY rather than hoping CMAKE_PREFIX_PATH wins the search order.
  #
  # This comment used to claim "which we point at our prefix" while setting only
  # CMAKE_PREFIX_PATH and an -I flag, and nothing checked the result. The wasm build made
  # the same assumption and was measurably wrong: it silently configured against
  # Emscripten's zlib and libjpeg PORTS on a machine whose emsdk cache had them, and failed
  # outright in CI where it did not. Asserted below, on both paths.
  #
  # USE_IMPLICIT_CRYPTO=OFF + REQUIRE_CRYPTO_NATIVE=ON is LOAD-BEARING: the upstream
  # default is ON and would link GnuTLS, which is LGPL-2.1-or-later. Asserted below.
  cmake -S "$src/qpdf-$QPDF_VERSION" -B "$b" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
    -DBUILD_SHARED_LIBS=OFF -DBUILD_STATIC_LIBS=ON \
    -DBUILD_DOC=OFF -DINSTALL_MANUAL=OFF -DINSTALL_EXAMPLES=OFF -DINSTALL_PKGCONFIG=OFF \
    -DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON \
    -DPKG_CONFIG_EXECUTABLE=/nonexistent-on-purpose \
    -DCMAKE_PREFIX_PATH="$prefix" \
    -DZLIB_H_PATH="$prefix/include" \
    -DZLIB_LIB_PATH="$prefix/lib/libz.a" \
    -DLIBJPEG_H_PATH="$prefix/include" \
    -DLIBJPEG_LIB_PATH="$prefix/lib/libjpeg.a" \
    -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_C_FLAGS="-I$prefix/include $cflags" \
    -DCMAKE_CXX_FLAGS="-I$prefix/include $cxxflags" \
    > "$src/qpdf-$variant-$arch-configure.log" 2>&1 || {
      echo "build-native: qpdf ($variant) configure failed; see $src/qpdf-$variant-$arch-configure.log" >&2
      tail -20 "$src/qpdf-$variant-$arch-configure.log" >&2 || true
      exit 1
    }

  # Fail closed on WHICH zlib and libjpeg were used, not just on the crypto flags. A build
  # against the host's copies is a different artifact from the one engines/licenses.toml
  # declares, and nothing downstream would notice.
  for var in ZLIB_H_PATH ZLIB_LIB_PATH LIBJPEG_H_PATH LIBJPEG_LIB_PATH; do
    local resolved
    resolved="$(sed -n "s/^$var:[A-Z]*=//p" "$b/CMakeCache.txt")"
    case "$resolved" in
      "$prefix"/*) ;;
      *)
        echo "build-native: qpdf ($variant) configured $var to '$resolved'" >&2
        echo "  That is outside $prefix, so it is the host's copy rather than the" >&2
        echo "  checksum-verified source engines/licenses.toml declares." >&2
        exit 1
        ;;
    esac
  done

  # Fail closed on the crypto question rather than trusting the flags took effect.
  local log="$src/qpdf-$variant-$arch-configure.log"
  grep -q "GNU TLS crypto enabled: OFF" "$log" || { echo "build-native: GnuTLS was NOT disabled" >&2; exit 1; }
  grep -q "OpenSSL crypto enabled: OFF"  "$log" || { echo "build-native: OpenSSL was NOT disabled" >&2; exit 1; }
  grep -q "Native crypto enabled: ON"    "$log" || { echo "build-native: native crypto not enabled" >&2; exit 1; }

  cmake --build "$b" --target libqpdf -j "$jobs" >/dev/null

  # BURROW'S SHIM, compiled with this variant's own flags and qpdf's (the C++ standard, the
  # define and the visibility its CMake sets for libqpdf), against this build's generated
  # `qpdf-config.h` and the source tree's private headers -- the same layout of
  # `QPDF::Members` libqpdf was compiled with. Added to libqpdf.a, so every link that has
  # qpdf has the shim and BUILD_MANIFEST.sha256 covers it with no new entry.
  write_qpdf_shim "$b/burrow_qpdf_shim.cpp"
  # shellcheck disable=SC2086 # the variant's flags are a word list on purpose
  clang++ $cxxflags -O3 -DNDEBUG -std=c++20 -fvisibility=hidden -fPIC -DQPDF_DISABLE_QTC=1 \
    -Wall -Wextra -Wconversion -Wsign-conversion -Wold-style-cast -Werror \
    -I "$prefix/include" -I "$src/qpdf-$QPDF_VERSION/include" \
    -I "$src/qpdf-$QPDF_VERSION/libqpdf" -I "$b/libqpdf" \
    -c "$b/burrow_qpdf_shim.cpp" -o "$b/burrow_qpdf_shim.o"
  local ar_tool
  ar_tool="$(sed -n 's/^CMAKE_AR:FILEPATH=//p' "$b/CMakeCache.txt")"
  "${ar_tool:-ar}" rcs "$b/libqpdf/libqpdf.a" "$b/burrow_qpdf_shim.o"
  # FAIL CLOSED ON THE SHIM BEING IN THE ARCHIVE, defined, not merely referenced.
  nm --defined-only "$b/libqpdf/libqpdf.a" 2>/dev/null \
    | grep -q ' T burrow_qpdf_fixed_dangling_refs$' || {
      echo "build-native: qpdf ($variant): burrow_qpdf_fixed_dangling_refs is not defined in libqpdf.a" >&2
      exit 1
    }
  echo "   shim ($variant): burrow_qpdf_fixed_dangling_refs defined in libqpdf.a"
  cp "$b/libqpdf/libqpdf.a" "$outdir/"
}

# BURROW'S QPDF SHIM (ADR 0031 §1, *Resolution at open*; owner decision (89)). burrow's own
# code, MIT OR Apache-2.0, not qpdf's: it edits nothing in qpdf's source and reads one private
# member. It is inline here and in engines/build-wasm.sh, so the CI cache keys and the wasm
# build stamp that hash these scripts change whenever it does. Keep the two copies identical.
write_qpdf_shim() { # path
  cat > "$1" <<'CPP'
// burrow_qpdf_fixed_dangling_refs: 1 if qpdf's whole-file resolution has run to the end since
// the last cross-reference reconstruction (QPDF::Members::fixed_dangling_refs, set at the end of
// QPDF::fixDanglingReferences and cleared by reconstruct_xref), 0 if not, -1 for a null handle.
// It reads one bool: it parses nothing, resolves nothing, allocates nothing and cannot throw.
#include <qpdf/DLL.h>
#include <qpdf/qpdf-c.h>

#include "qpdf/QPDF_private.hh"
#include "qpdf/qpdf-c_impl.hh"

namespace
{
    // THE STANDARD'S ROUTE TO A PRIVATE MEMBER WITHOUT EDITING THE CLASS: access is not checked
    // in the arguments of an explicit instantiation ([temp.spec.general]/6). Each name is still
    // looked up, so a qpdf that renames or removes either member, or `QPDF::m`, fails to
    // compile here rather than reading the wrong thing. The member-pointer types are deduced.
    template <typename Tag, auto Member>
    struct Steal
    {
        friend constexpr auto
        get(Tag)
        {
            return Member;
        }
    };
    struct MembersTag
    {
        friend constexpr auto get(MembersTag);
    };
    struct FlagTag
    {
        friend constexpr auto get(FlagTag);
    };
    template struct Steal<MembersTag, &QPDF::m>;
    template struct Steal<FlagTag, &QPDF::Members::fixed_dangling_refs>;
} // namespace

extern "C" QPDF_DLL int
burrow_qpdf_fixed_dangling_refs(qpdf_data qpdf) noexcept
{
    if (qpdf == nullptr || !qpdf->qpdf) {
        return -1;
    }
    auto const& members = (*qpdf->qpdf).*get(MembersTag{});
    if (!members) {
        return -1;
    }
    return (*members).*get(FlagTag{}) ? 1 : 0;
}
CPP
}

say "qpdf $QPDF_VERSION (static, native crypto only)"
rm -rf "$src/qpdf-$QPDF_VERSION"
tar xzf "$vendor/qpdf-$QPDF_VERSION.tar.gz" -C "$src"
build_qpdf plain "" "" "$prefix/lib"
cp -R "$src/qpdf-$QPDF_VERSION/include/qpdf" "$prefix/include/"
echo "   libqpdf.a $(stat -c%s "$prefix/lib/libqpdf.a") bytes"

say "qpdf $QPDF_VERSION (fuzzer + ASan instrumented)"
# For M1 PR 2's fuzz targets. Built here because we build qpdf from source, so we can
# instrument it -- which the prebuilt PDFium cannot be. Unused until PR 2.
build_qpdf fuzz \
  "-fsanitize=fuzzer-no-link,address -fno-omit-frame-pointer" \
  "-fsanitize=fuzzer-no-link,address -fno-omit-frame-pointer" \
  "$prefix/lib/fuzz"
echo "   fuzz/libqpdf.a $(stat -c%s "$prefix/lib/fuzz/libqpdf.a") bytes"

# Record what we produced. The static archives have no upstream hash to pin (we build
# them, and the builds are not bit-reproducible), so core/burrow-engines/build.rs
# verifies them against this instead. That catches a tree changed after the build --
# including one restored from a CI cache that no longer matches it. It does not establish
# upstream provenance: that comes from fetch.sh verifying the source tarballs.
say "recording build manifest"
# The instrumented archive is covered too. It was not until M1 PR 3, which is a real gap:
# fuzz/libqpdf.a is linked into fuzz binaries, and a CI cache restoring a tree nothing had
# checksummed is exactly the case this manifest exists to catch. Recorded with its `fuzz/`
# prefix so build.rs can tell the two apart.
( cd "$prefix/lib" && sha256sum libpdfium.so libqpdf.a libz.a libjpeg.a fuzz/libqpdf.a > BUILD_MANIFEST.sha256 )
sed 's/^/   /' "$prefix/lib/BUILD_MANIFEST.sha256"

say "done: $prefix"
find "$prefix/lib" -maxdepth 2 \( -name '*.a' -o -name '*.so' \) | sort | while read -r f; do
  printf '   %-28s %10s bytes\n' "${f#"$prefix/lib/"}" "$(stat -c%s "$f")"
done
