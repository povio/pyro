#!/bin/sh
# Linker wrapper for macOS targets (see .cargo/config.toml).
#
# Links with the default `cc`, then re-signs linked executables ad hoc with
# scripts/macos/jit.entitlements (com.apple.security.cs.allow-jit only), so
# the system JavaScriptCore can use its JIT in `lightningpanda` and in test
# binaries. Without the entitlement JSC silently runs interpreter-only.
# The signature stays ad hoc, as the linker's own is; nothing else changes.
# Dynamic libraries and bundles are left with the linker's signature.
set -eu
cc "$@"
out=""
prev=""
for arg in "$@"; do
    case "$arg" in -dylib|-bundle|-dynamiclib|-shared) exit 0 ;; esac
    if [ "$prev" = "-o" ]; then out="$arg"; fi
    prev="$arg"
done
[ -n "$out" ] || exit 0
entitlements="$(cd "$(dirname "$0")" && pwd)/jit.entitlements"
# codesign reports "replacing existing signature" on stderr; rustc would
# surface that as a linker warning, so its output is shown only on failure.
if ! log="$(codesign --force --sign - --entitlements "$entitlements" "$out" 2>&1)"; then
    printf '%s\n' "$log" >&2
    exit 1
fi
