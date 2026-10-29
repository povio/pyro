#!/usr/bin/env bash
# Builds the patched JavaScriptCore dylib that lp-jsc-sys links instead of the
# system framework (see crates/lp-jsc-sys/build.rs and docs/ARCHITECTURE.md).
#
# Inputs: crates/lp-jsc-sys/webkit/REVISION (WebKit commit) and
# crates/lp-jsc-sys/webkit/patches/*.patch. The result goes to
# $LP_JSC_CACHE/<revision>-<patch hash>/ with a copy of those inputs, which is how
# build.rs finds a build that matches the checkout. Re-running after a patch
# change rebuilds incrementally in the shared build directory.
#
# Needs cmake, ninja, the Xcode Command Line Tools and ~30 GB of free RAM at -j10.
#
# Environment:
#   LP_JSC_CACHE   artifact root (default ~/Library/Caches/lightningpanda/jsc)
#   LP_WEBKIT_SRC  WebKit checkout, created if missing (default $LP_JSC_CACHE/../webkit-src)
#   LP_JSC_BUILD   ninja build dir (default $LP_JSC_CACHE/../webkit-build)
#   JOBS           parallel jobs (default: number of CPUs)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
inputs="$root/crates/lp-jsc-sys/webkit"
rev="$(tr -d '[:space:]' < "$inputs/REVISION")"
cache="${LP_JSC_CACHE:-$HOME/Library/Caches/lightningpanda/jsc}"
src="${LP_WEBKIT_SRC:-$(dirname "$cache")/webkit-src}"
build="${LP_JSC_BUILD:-$(dirname "$cache")/webkit-build}"
jobs="${JOBS:-$(sysctl -n hw.ncpu)}"

patch_hash="$(cat "$inputs"/patches/*.patch | shasum -a 256 | cut -c1-12)"
out="$cache/${rev:0:12}-$patch_hash"
if [[ -f "$out/lib/libJavaScriptCore.dylib" ]]; then
    echo "up to date: $out"
    exit 0
fi

for tool in cmake ninja; do
    command -v "$tool" >/dev/null || { echo "missing $tool (brew install cmake ninja)" >&2; exit 1; }
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

cmake -G Ninja -S "$src" -B "$build" \
    -DPORT=JSCOnly \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0 \
    -DENABLE_STATIC_JSC=OFF \
    -DDEVELOPER_MODE=OFF \
    -DENABLE_TOOLS=OFF
ninja -C "$build" -j "$jobs" JavaScriptCore

lib="$(find "$build/lib" -maxdepth 1 -name 'libJavaScriptCore*.dylib' -type f | head -1)"
[[ -n "$lib" ]] || { echo "no libJavaScriptCore dylib under $build/lib" >&2; exit 1; }

tmp="$out.tmp"
rm -rf "$tmp"
mkdir -p "$tmp/lib" "$tmp/inputs/patches"
cp "$lib" "$tmp/lib/libJavaScriptCore.dylib"
cp "$inputs/REVISION" "$tmp/inputs/"
cp "$inputs"/patches/*.patch "$tmp/inputs/patches/"
mv "$tmp" "$out"
# An absolute install name lets every binary that links it load it without an rpath.
install_name_tool -id "$out/lib/libJavaScriptCore.dylib" "$out/lib/libJavaScriptCore.dylib"
codesign -f -s - "$out/lib/libJavaScriptCore.dylib"
echo "built: $out"
