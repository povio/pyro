#!/usr/bin/env bash
# Linux counterpart of bundle-jsc.sh: makes a browser self-contained by copying
# the patched JavaScriptCore it was built against, and the ICU libraries that
# library needs (their sonames are versioned per distribution), to DIR/lib, and
# pointing DIR/lightningpanda (and DIR/lp-snapshot-compare) at them through
# $ORIGIN rpaths.
#
#   scripts/release/bundle-jsc-linux.sh DIR JSC_SO
#
# DIR holds `lightningpanda` (and optionally `lp-snapshot-compare`), as built
# with LP_JSC_DIR; the build links the library by its absolute SONAME.
set -euo pipefail

dir="$(cd "${1:?usage: $0 DIR JSC_SO}" && pwd)"
so="${2:?usage: $0 DIR JSC_SO}"

mkdir -p "$dir/lib"
cp "$so" "$dir/lib/libJavaScriptCore.so"
chmod u+w "$dir/lib/libJavaScriptCore.so"
# ICU, resolved where the build machine's loader finds it.
while read -r name path; do
    cp -L "$path" "$dir/lib/$name"
    chmod u+w "$dir/lib/$name"
    patchelf --set-rpath '$ORIGIN' "$dir/lib/$name"
done < <(ldd "$so" | awk '$1 ~ /^libicu/ && $3 ~ /^\// {print $1, $3}')
# ICU's license (Unicode-3.0) travels with its libraries.
for c in /usr/share/doc/libicu[0-9]*/copyright; do
    [[ -f "$c" ]] && cp "$c" "$dir/lib/ICU-COPYRIGHT.txt" && break
done
patchelf --set-soname libJavaScriptCore.so --set-rpath '$ORIGIN' "$dir/lib/libJavaScriptCore.so"

for bin in lightningpanda lp-snapshot-compare; do
    [[ -f "$dir/$bin" ]] || continue
    linked="$(patchelf --print-needed "$dir/$bin" | grep 'libJavaScriptCore\.so' || true)"
    if [[ -z "$linked" ]]; then
        # The linker drops unused libraries (--as-needed); the comparator may not use JSC.
        [[ "$bin" != lightningpanda ]] || { echo "error: $dir/$bin does not link libJavaScriptCore.so" >&2; exit 1; }
        continue
    fi
    [[ "$linked" == libJavaScriptCore.so ]] || patchelf --replace-needed "$linked" libJavaScriptCore.so "$dir/$bin"
    patchelf --set-rpath '$ORIGIN/lib' "$dir/$bin"
done
