#!/usr/bin/env bash
# Makes a fork-capable browser self-contained: copies the patched
# JavaScriptCore it was built against to DIR/lib/libJavaScriptCore.dylib,
# points DIR/lightningpanda at that copy, and re-signs what it changed.
#
#   scripts/release/bundle-jsc.sh DIR JSC_DYLIB
#
# DIR holds `lightningpanda` (and optionally `lp-snapshot-compare`), as built
# with LP_JSC_DIR; the build links the library by its absolute cache path.
# Used for the fork release archive and for the npm package's arm64 browser.
set -euo pipefail

dir="$(cd "${1:?usage: $0 DIR JSC_DYLIB}" && pwd)"
dylib="${2:?usage: $0 DIR JSC_DYLIB}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
entitlements="$root/scripts/macos/jit.entitlements"

linked="$(otool -L "$dir/lightningpanda" | awk '/libJavaScriptCore\.dylib/ {print $1; exit}')"
[[ -n "$linked" ]] || { echo "error: $dir/lightningpanda does not link libJavaScriptCore.dylib" >&2; exit 1; }

mkdir -p "$dir/lib"
cp "$dylib" "$dir/lib/libJavaScriptCore.dylib"
chmod u+w "$dir/lib/libJavaScriptCore.dylib"
install_name_tool -id @rpath/libJavaScriptCore.dylib "$dir/lib/libJavaScriptCore.dylib" 2>/dev/null
if [[ "$linked" != @loader_path/lib/libJavaScriptCore.dylib ]]; then
    install_name_tool -change "$linked" @loader_path/lib/libJavaScriptCore.dylib "$dir/lightningpanda" 2>/dev/null
fi
codesign -f -s - "$dir/lib/libJavaScriptCore.dylib" 2>/dev/null
codesign -f -s - --entitlements "$entitlements" "$dir/lightningpanda" 2>/dev/null
if [[ -f "$dir/lp-snapshot-compare" ]]; then
    codesign -f -s - --entitlements "$entitlements" "$dir/lp-snapshot-compare" 2>/dev/null
fi
