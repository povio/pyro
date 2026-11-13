#!/usr/bin/env bash
# Makes a Linux browser self-contained: copies the ICU libraries it links (their
# sonames are versioned per distribution) to DIR/lib and points
# DIR/pyro (and DIR/pyro-snapshot-compare) at them through an $ORIGIN
# rpath. The patched JavaScriptCore itself is inside the browser: a static
# archive since 2026-11 (scripts/jsc/build.sh), where this script used to copy
# libJavaScriptCore.so. macOS needs no counterpart: its ICU is the system's.
#
#   scripts/release/bundle-icu-linux.sh DIR
#
# DIR holds `pyro` (and optionally `pyro-snapshot-compare`), as built
# with PYRO_JSC_DIR.
set -euo pipefail

dir="$(cd "${1:?usage: $0 DIR}" && pwd)"

if patchelf --print-needed "$dir/pyro" | grep -q 'libJavaScriptCore'; then
    echo "error: $dir/pyro links a shared JavaScriptCore; build it against lib/libJavaScriptCore.a (scripts/jsc/build.sh)" >&2
    exit 1
fi

mkdir -p "$dir/lib"
# ICU, resolved where the build machine's loader finds it (libicui18n needs
# libicuuc, which needs libicudata: ldd lists all three).
copied=0
while read -r name path; do
    cp -L "$path" "$dir/lib/$name"
    chmod u+w "$dir/lib/$name"
    patchelf --set-rpath '$ORIGIN' "$dir/lib/$name"
    copied=$((copied + 1))
done < <(ldd "$dir/pyro" | awk '$1 ~ /^libicu/ && $3 ~ /^\// {print $1, $3}')
[[ "$copied" -gt 0 ]] || { echo "error: $dir/pyro links no ICU library" >&2; exit 1; }
# ICU's license (Unicode-3.0) travels with its libraries.
for c in /usr/share/doc/libicu[0-9]*/copyright; do
    [[ -f "$c" ]] && cp "$c" "$dir/lib/ICU-COPYRIGHT.txt" && break
done

for bin in pyro pyro-snapshot-compare; do
    [[ -f "$dir/$bin" ]] || continue
    # The linker drops unused libraries (--as-needed); the comparator may not use ICU.
    patchelf --print-needed "$dir/$bin" | grep -q '^libicu' || continue
    patchelf --set-rpath '$ORIGIN/lib' "$dir/$bin"
done
