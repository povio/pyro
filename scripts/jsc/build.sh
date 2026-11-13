#!/usr/bin/env bash
# Builds the patched JavaScriptCore that pyro-jsc-sys links into the binary instead
# of the system framework (see crates/pyro-jsc-sys/build.rs and
# docs/ARCHITECTURE.md): one static archive, lib/libJavaScriptCore.a, holding
# JavaScriptCore, WTF and bmalloc (scripts/jsc/archive.sh). ICU and the C++
# runtime stay the system's shared libraries.
#
# Inputs: crates/pyro-jsc-sys/webkit/REVISION (WebKit commit) and
# crates/pyro-jsc-sys/webkit/patches/*.patch. The result goes to
# $PYRO_JSC_CACHE/<revision>-<patch hash>/ with a copy of those inputs, which is how
# build.rs finds a build that matches the checkout. Re-running after a patch
# change rebuilds incrementally in the shared build directory.
#
# Needs cmake, ninja, the Xcode Command Line Tools (macOS) or clang + ICU dev
# headers (Linux, see scripts/linux/Dockerfile) and ~30 GB of free RAM at -j10.
#
# Environment:
#   PYRO_JSC_CACHE   artifact root (default ~/Library/Caches/pyro/jsc on
#                  macOS, ${XDG_CACHE_HOME:-~/.cache}/pyro/jsc on Linux)
#   PYRO_WEBKIT_SRC  WebKit checkout, created if missing (default $PYRO_JSC_CACHE/../webkit-src)
#   PYRO_JSC_BUILD   ninja build dir (default $PYRO_JSC_CACHE/../webkit-build)
#   JOBS           parallel jobs (default: number of CPUs)
#   CC, CXX        compilers (Linux: default clang; macOS: default the Command Line
#                  Tools', which compile against the SDK's libc++ headers, as a
#                  compiler set here does too)
#   PYRO_JSC_CFLAGS  extra C and C++ flags (scripts/pgo: profile instrumentation and
#                  use). The output directory's name does not cover the compiler or
#                  flags: a build with others needs its own PYRO_JSC_CACHE and PYRO_JSC_BUILD.
#   PYRO_JSC_LTO     thin (macOS): an archive of ThinLTO bitcode, which the binary's link
#                  optimizes (scripts/pgo, PYRO_PGO_LTO; ld then needs the same LLVM's
#                  libLTO). CC must be an LLVM release's clang: its llvm-ar,
#                  llvm-ranlib, llvm-libtool-darwin and libLTO.dylib build the archive.
#
# Static since 2026-11: the 48 MB dylib cost dyld 1.2 ms of every launch on the
# M1 Max (process start to main 7.2-7.7 ms, 6.0-6.1 ms with the archive linked
# in) and was a second file to ship, sign and find at run time.
#
# No LTO by default. A -DLTO_MODE=thin build (about twice the build time) was
# measured against this one on the M1 Max, serve CPU, binaries alternated:
# Playwright suite -1%/+5%, 1,200-row table loops -3..+6%, PDF templates -4%,
# ToS SPA load within noise: no reproducible gain. Together with a profile it
# pays (scripts/pgo, PYRO_PGO_LTO; README "Profile-guided optimization"). An LTO
# build applies webkit/lto/llint-without-lto.patch, which compiles
# llint/LowLevelInterpreter.cpp without LTO: the LTO backend moves the
# offlineasm IPInt handlers and IPInt::initialize()'s alignment RELEASE_ASSERT
# kills the process at startup.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
inputs="$root/crates/pyro-jsc-sys/webkit"
rev="$(tr -d '[:space:]' < "$inputs/REVISION")"
os="$(uname -s)"
libname=libJavaScriptCore.a
if [[ "$os" == Darwin ]]; then
    default_cache="$HOME/Library/Caches/pyro/jsc"
    ncpu="$(sysctl -n hw.ncpu)"
else
    default_cache="${XDG_CACHE_HOME:-$HOME/.cache}/pyro/jsc"
    ncpu="$(nproc)"
fi
cache="${PYRO_JSC_CACHE:-$default_cache}"
src="${PYRO_WEBKIT_SRC:-$(dirname "$cache")/webkit-src}"
build="${PYRO_JSC_BUILD:-$(dirname "$cache")/webkit-build}"
jobs="${JOBS:-$ncpu}"

sha256() { if command -v shasum >/dev/null; then shasum -a 256; else sha256sum; fi; }
patch_hash="$(cat "$inputs"/patches/*.patch | sha256 | cut -c1-12)"
out="$cache/${rev:0:12}-$patch_hash"
if [[ -f "$out/lib/$libname" ]]; then
    echo "up to date: $out"
    exit 0
fi

for tool in cmake ninja; do
    command -v "$tool" >/dev/null || { echo "missing $tool (brew install cmake ninja / apt install cmake ninja-build)" >&2; exit 1; }
done

# Sparse, blobless checkout: JavaScriptCore, WTF, bmalloc and the build tooling.
if [[ ! -d "$src/.git" ]]; then
    mkdir -p "$src"
    git -C "$src" init -q
    git -C "$src" remote add origin https://github.com/WebKit/WebKit.git
    git -C "$src" sparse-checkout set --no-cone '/*' '!/LayoutTests/' '!/JSTests/' '!/PerformanceTests/' \
        '!/Websites/' '!/ManualTests/' '!/Source/WebCore/' '!/Source/WebKit/' '!/Source/WebKitLegacy/' \
        '!/Source/ThirdParty/ANGLE/' '!/Source/ThirdParty/libwebrtc/' '!/Tools/' '/Tools/Scripts/' \
        '/Tools/jsc/' '/Tools/CMakeLists.txt'
fi
if ! git -C "$src" cat-file -e "$rev^{commit}" 2>/dev/null; then
    git -C "$src" fetch --depth 1 --filter=blob:none origin "$rev"
fi
git -C "$src" checkout -q -f "$rev"
git -C "$src" clean -fdq Source
for p in "$inputs"/patches/*.patch; do
    git -C "$src" apply --whitespace=nowarn "$p"
done
# LTO only (webkit/lto/, outside the cache key: no-ops without LTO_MODE).
if [[ -n "${PYRO_JSC_LTO:-}" ]]; then
    for p in "$inputs"/lto/*.patch; do
        git -C "$src" apply --whitespace=nowarn "$p"
    done
fi

# Resolve the real interpreter: version-manager shims (asdf) fail in the build
# directory, which has no .tool-versions.
ruby_bin="$(ruby -e 'print RbConfig.ruby' 2>/dev/null || echo /usr/bin/ruby)"

platform_flags=()
cflags="${PYRO_JSC_CFLAGS:-}"
cxxflags="$cflags"
if [[ "$os" == Darwin ]]; then
    platform_flags+=(-DCMAKE_OSX_DEPLOYMENT_TARGET=14.0)
    if [[ -n "${CC:-}${CXX:-}" ]]; then
        # Another clang (an LLVM release for scripts/pgo) would take its own libc++
        # headers, newer than the system's libc++ the binary links.
        sdk="$(xcrun --show-sdk-path)"
        platform_flags+=(-DCMAKE_C_COMPILER="${CC:-clang}" -DCMAKE_CXX_COMPILER="${CXX:-clang++}"
                         -DCMAKE_OSX_SYSROOT="$sdk")
        cxxflags="-nostdinc++ -isystem $sdk/usr/include/c++/v1 $cxxflags"
    fi
else
    platform_flags+=(-DCMAKE_C_COMPILER="${CC:-clang}" -DCMAKE_CXX_COMPILER="${CXX:-clang++}")
fi
if [[ -n "${PYRO_JSC_LTO:-}" ]]; then
    [[ "$os" == Darwin ]] || { echo "PYRO_JSC_LTO: macOS only (not tried on Linux)" >&2; exit 1; }
    llvm_bin="$(dirname "${CC:?PYRO_JSC_LTO needs CC, an LLVM release clang}")"
    # Apple's ld with that LLVM's libLTO for the executables the build runs (the
    # LLInt offset extractors): ld64.lld cannot read the macOS 27 SDK's .tbd files.
    platform_flags+=(-DLTO_MODE="$PYRO_JSC_LTO" -DUSE_LD_LLD=OFF
                     -DCMAKE_AR="$llvm_bin/llvm-ar" -DCMAKE_RANLIB="$llvm_bin/llvm-ranlib"
                     -DCMAKE_EXE_LINKER_FLAGS="-Wl,-lto_library,$llvm_bin/../lib/libLTO.dylib")
    # Xcode's libtool cannot index a newer LLVM's bitcode (archive.sh).
    export LIBTOOL="$llvm_bin/llvm-libtool-darwin"
fi
# Only when set: otherwise CMake reads CFLAGS and CXXFLAGS as before.
if [[ -n "$cflags" ]]; then platform_flags+=(-DCMAKE_C_FLAGS="$cflags"); fi
if [[ -n "$cxxflags" ]]; then platform_flags+=(-DCMAKE_CXX_FLAGS="$cxxflags"); fi

cmake -G Ninja -S "$src" -B "$build" \
    -DRuby_EXECUTABLE="$ruby_bin" \
    -DPORT=JSCOnly \
    -DCMAKE_BUILD_TYPE=Release \
    "${platform_flags[@]}" \
    -DENABLE_STATIC_JSC=ON \
    -DDEVELOPER_MODE=OFF \
    -DENABLE_TOOLS=OFF

tmp="$out.tmp"
rm -rf "$tmp"
mkdir -p "$tmp/lib" "$tmp/inputs/patches"
cp "$inputs/REVISION" "$tmp/inputs/"
cp "$inputs"/patches/*.patch "$tmp/inputs/patches/"
# Builds, and merges JavaScriptCore, its JIT subtarget's objects, WTF and bmalloc.
JOBS="$jobs" "$root/scripts/jsc/archive.sh" "$build" "$tmp/lib/$libname"
mv "$tmp" "$out"
echo "built: $out"
