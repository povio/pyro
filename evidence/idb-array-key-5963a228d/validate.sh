#!/bin/bash
set -euxo pipefail
d="$(cd "$(dirname "$0")" && pwd)"
cd "$d/src"
export CARGO_TARGET_DIR="$1" CARGO_BUILD_JOBS=1
profile="$2"
export CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/$(scripts/jsc/cache-key.sh)"
test -s "$PYRO_JSC_DIR/lib/libJavaScriptCore.a"
shasum -a 256 "$PYRO_JSC_DIR/lib/libJavaScriptCore.a" > "$d/jsc.sha256"
diff -qr "$PYRO_JSC_DIR/inputs/patches" crates/pyro-jsc-sys/webkit/patches
dir="$profile"
if [[ $dir = dev ]]; then dir=debug; fi
for arm in base candidate; do
 if [[ $arm = candidate ]]; then git restore crates/pyro-webapi/tests/indexeddb_js.rs; git checkout --detach 5963a228d; fi
 test -z "$(git status --porcelain)"
 touch crates/pyro-webapi/src/{idb.js,indexeddb.rs}
 cargo build --locked --profile "$profile" -p pyro > "$d/$arm-build.log" 2>&1
 cp -c "$CARGO_TARGET_DIR/$dir/pyro" "$d/$arm"
 python3 "$d/check.py" "$arm" "$d/$arm" > "$d/$arm.source"
 if [[ $arm = base ]]; then git apply "$d/regression.patch"; fi
 result=0
 cargo test --locked --profile "$profile" -p pyro-webapi --test integration -- indexeddb_js::array_key_conversion_rejects_proxies_without_traps --exact > "$d/$arm-native.log" 2>&1 || result=$?
 rg '^running 1 test$' "$d/$arm-native.log"
 if [[ $arm = base ]]; then test "$result" = 101; rg 'array_key_conversion_rejects_proxies_without_traps .* FAILED' "$d/$arm-native.log"; else test "$result" = 0; rg 'array_key_conversion_rejects_proxies_without_traps .* ok' "$d/$arm-native.log"; fi
done
