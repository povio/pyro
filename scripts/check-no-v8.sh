#!/usr/bin/env bash
# Fails if any crate in the dependency graph looks like a V8 binding.
# The rewrite must run on JavaScriptCore only.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo "error: cargo not on PATH" >&2; exit 2; }
tree=$(cargo tree --workspace --edges normal,build,dev --prefix none --format '{p}' | sort -u)
if printf '%s\n' "$tree" | grep -Ei '^(v8|rusty_v8|deno_core|deno_|v8-|libv8)[ @]' ; then
    echo "error: V8-based crate in dependency graph" >&2
    exit 1
fi
if grep -rIl --include='*.rs' --include='*.toml' --include='*.m' -E '\bv8::|rusty_v8|libv8' crates 2>/dev/null; then
    echo "error: V8 reference in sources" >&2
    exit 1
fi
echo "ok: no V8 in dependency graph ($(printf '%s\n' "$tree" | wc -l | tr -d ' ') packages)"
