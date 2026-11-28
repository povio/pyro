#!/bin/sh
# Self-test of scripts/macos/linker/ld with stub xcrun/codesign/nm (no link runs).
# Usage: scripts/macos/test-linker.sh [path/to/libJavaScriptCore.a]
# Without an argument it uses $PYRO_JSC_DIR/lib/libJavaScriptCore.a.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
ld="$here/linker/ld"
lib="${1:-${PYRO_JSC_DIR:-}/lib/libJavaScriptCore.a}"
[ -f "$lib" ] || { echo "no libJavaScriptCore.a at $lib" >&2; exit 2; }
libdir="$(dirname "$lib")"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
real_nm="$(command -v nm)"
printf '#!/bin/sh\n[ "$1" = ld ] && shift\nprintf "%%s\\n" "$@" > "$STUB_LOG"\n' > "$tmp/bin/xcrun"
printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/codesign"
chmod +x "$tmp/bin/xcrun" "$tmp/bin/codesign"
run() { STUB_LOG="$tmp/log" PATH="$tmp/bin:$PATH" TMPDIR="$tmp" "$ld" "$@"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

run -L "$libdir" -lJavaScriptCore -o "$tmp/x"
n="$(grep -c '^-u$' "$tmp/log")"
bad="$(awk 'p == "-u" && $0 !~ /^_/ { c++ } { p = $0 } END { print c + 0 }' "$tmp/log")"
[ "$n" -gt 700 ] && [ "$bad" = 0 ] || fail "-L -l: $n -u options, $bad without underscore"
echo "ok: -L dir -lJavaScriptCore passes $n -u symbols"
run -L"$libdir" -lJavaScriptCore -o "$tmp/x"
[ "$(grep -c '^-u$' "$tmp/log")" = "$n" ] || fail "-L<dir> form"
echo "ok: -L<dir> form"
run "$lib" -o "$tmp/x"
[ "$(grep -c '^-u$' "$tmp/log")" = "$n" ] || fail "path form"
echo "ok: explicit archive path"
printf -- '-L\n%s\n-lJavaScriptCore\n-o\n%s\n' "$libdir" "$tmp/x" > "$tmp/rsp"
run "@$tmp/rsp"
[ "$(grep -c '^-u$' "$tmp/log")" = "$n" ] || fail "response file"
echo "ok: response file"
PYRO_KEEP_JSC_INTERPRETER_ATOMS=0 run -L "$libdir" -lJavaScriptCore -o "$tmp/x"
[ "$(grep -c '^-u$' "$tmp/log" || true)" = 0 ] || fail "escape hatch"
echo "ok: PYRO_KEEP_JSC_INTERPRETER_ATOMS=0"
run -lfoo -L/nonexistent -o "$tmp/x" a.o
[ "$(cat "$tmp/log")" = "$(printf -- '-lfoo\n-L/nonexistent\n-o\n%s\na.o' "$tmp/x")" ] || fail "unrelated link changed"
echo "ok: link without the archive is unchanged"
run -dylib -L "$libdir" -lJavaScriptCore -o "$tmp/x.dylib"
[ "$(cat "$tmp/log")" = "$(printf -- '-dylib\n-L\n%s\n-lJavaScriptCore\n-o\n%s' "$libdir" "$tmp/x.dylib")" ] || fail "dylib changed"
echo "ok: -dylib link is unchanged"

# Post-link check: a stub nm that reports the archive's own symbols passes,
# the same list minus the two sentinels fails.
syms="$("$real_nm" -g -U -m "$lib" | awk '/:$/ { m = ($0 ~ /LowLevelInterpreter\.cpp\.o\)?:$/); next } m { print $1, "S", $NF }')"
printf '%s\n' "$syms" > "$tmp/good.nm"
grep -v -e _ipint_i64_add128_validate -e _ipint_simd_i8x16_relaxed_swizzle_validate "$tmp/good.nm" > "$tmp/bad.nm"
# a shifted copy: every address after the first sentinel moves by 0x280
awk 'BEGIN { s = 0 } $3 == "_ipint_argumINT_a0_validate" { $1 = sprintf("%016x", 0) } { print }' "$tmp/good.nm" > "$tmp/shift.nm"
printf '#!/bin/sh\ncase "$*" in *-g*) exec "%s" "$@" ;; *) cat "$NM_FILE" ;; esac\n' "$real_nm" > "$tmp/bin/nm"
chmod +x "$tmp/bin/nm"
chk() { NM_FILE="$1" PATH="$tmp/bin:$PATH" "$ld" --check-jsc-interpreter "$lib" "$tmp/x" 2>/dev/null; }
chk "$tmp/good.nm" || fail "check rejected the archive's own symbols"
echo "ok: check accepts intact symbols"
chk "$tmp/bad.nm" && fail "check accepted missing sentinels"
echo "ok: check rejects missing sentinels"
chk "$tmp/shift.nm" && fail "check accepted moved handlers"
echo "ok: check rejects moved handlers"
echo "all passed"
