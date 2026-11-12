#!/usr/bin/env bash
# Builds the static JavaScriptCore of a configured JSCOnly build directory
# (-DENABLE_STATIC_JSC=ON, see build.sh) and merges it into one archive:
#
#   scripts/jsc/archive.sh BUILD_DIR OUT.a
#
# A static JSCOnly build is four pieces: lib/libJavaScriptCore.a, lib/libWTF.a,
# lib/libbmalloc.a, and the objects of the JavaScriptCoreJIT subtarget (jit, dfg,
# ftl, bytecode, bytecompiler, llint: an object library that CMake hands to
# whatever links JavaScriptCore instead of putting it in the archive). One
# archive holds them all, so lp-jsc-sys names a single library and the order of
# the pieces on the link line cannot matter.
#
# build.sh calls this; run it by hand for a build directory of your own (a git
# worktree of the WebKit checkout, which build.sh cannot target).
#
# Environment: JOBS (parallel jobs, default: number of CPUs), AR (Linux, default ar),
# LIBTOOL (macOS, default libtool; build.sh sets llvm-libtool-darwin for an LTO archive).
set -euo pipefail

build="$(cd "${1:?usage: $0 BUILD_DIR OUT.a}" && pwd)"
out="${2:?usage: $0 BUILD_DIR OUT.a}"
if [[ "$(uname -s)" == Darwin ]]; then
    ncpu="$(sysctl -n hw.ncpu)"
else
    ncpu="$(nproc)"
fi
jobs="${JOBS:-$ncpu}"

# The subtarget's objects as this configuration defines them (not a directory
# listing: an object left by an earlier configuration would be linked twice).
jit_objects=()
while IFS= read -r object; do
    jit_objects+=("$object")
done < <(ninja -C "$build" -t targets all |
    sed -n 's|^\(Source/JavaScriptCore/CMakeFiles/JavaScriptCoreJIT\.dir/.*\.o\): .*$|\1|p')
[[ "${#jit_objects[@]}" -gt 0 ]] || { echo "no JavaScriptCoreJIT objects in $build/build.ninja: has WebKit dropped the subtarget (Source/JavaScriptCore/CMakeLists.txt)?" >&2; exit 1; }

ninja -C "$build" -j "$jobs" JavaScriptCore WTF bmalloc "${jit_objects[@]}"

for piece in libJavaScriptCore.a libWTF.a libbmalloc.a; do
    [[ -f "$build/lib/$piece" ]] || { echo "no $piece under $build/lib (configured with -DENABLE_STATIC_JSC=ON?)" >&2; exit 1; }
done

mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
rm -f "$out"
if [[ "$(uname -s)" == Darwin ]]; then
    (cd "$build" && "${LIBTOOL:-libtool}" -static -no_warning_for_no_symbols -o "$out" \
        lib/libJavaScriptCore.a "${jit_objects[@]}" lib/libWTF.a lib/libbmalloc.a)
else
    # An MRI script: `ar` has no option that adds another archive's members. It
    # writes inside the build directory, whose names are plain: the script's
    # syntax has no quoting, and a `+` in the output path would end the name.
    merged=lib/libJavaScriptCoreMerged.a
    rm -f "$build/$merged"
    {
        echo "create $merged"
        echo "addlib lib/libJavaScriptCore.a"
        for object in "${jit_objects[@]}"; do
            echo "addmod $object"
        done
        echo "addlib lib/libWTF.a"
        echo "addlib lib/libbmalloc.a"
        echo save
        echo end
    } | (cd "$build" && "${AR:-ar}" -M)
    mv "$build/$merged" "$out"
fi
[[ -s "$out" ]] || { echo "no archive written to $out" >&2; exit 1; }
echo "archived: $out (${#jit_objects[@]} JIT objects)"
