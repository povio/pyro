#!/bin/bash
set -eu
d=$(cd "$(dirname "$0")" && pwd)
cd "$d/src"
export CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR="$1"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/$(scripts/jsc/cache-key.sh)"
test -f "$PYRO_JSC_DIR/lib/libJavaScriptCore.a"
shasum -a 256 "$PYRO_JSC_DIR/lib/libJavaScriptCore.a" > "$d/jsc.sha256"
for arm in parent candidate; do
 if [ "$arm" = candidate ]; then git restore crates/pyro-webapi/tests/indexeddb_js.rs; git checkout -q --detach 400b8aaa869e90593c7e8bc645ef03c6f11a169b; fi
 git rev-parse HEAD 'HEAD^{tree}' > "$d/$arm-source.txt"; git diff --name-only >> "$d/$arm-source.txt"
 # Cargo's previous target may have been populated from another checkout.
 touch crates/pyro-js/src/engine.rs crates/pyro-js/src/lib.rs crates/pyro-webapi/src/indexeddb.rs crates/pyro-webapi/tests/indexeddb_js.rs
 if [ "$arm" = parent ] && [ -n "${2:-}" ]; then cp -c "$2" "$d/$arm"; else
 cargo build --locked -j1 -p pyro > "$d/$arm-build.log" 2>&1
 cp -c "$CARGO_TARGET_DIR/debug/pyro" "$d/$arm"; fi
 python3 "$d/verify-source.py" "$arm" "$d/$arm" "$d" >> "$d/embedded.log"
 shasum -a 256 "$d/$arm" > "$d/$arm.sha256"
 if [ "$arm" = parent ]; then git apply "$d/native-only.patch"; fi
 rc=0; cargo test --locked -j1 -p pyro-webapi --test integration -- indexeddb_js::ordinary_transaction_creation_ends_between_listener_invocations --exact --test-threads=1 > "$d/$arm-creation.log" 2>&1 || rc=$?
 echo "$rc" > "$d/$arm-creation.exit"; if [ "$arm" = candidate ]; then test "$rc" = 0; fi
 for f in indexes_cursors_and_directions records_survive_navigation_and_appear_in_databases; do cargo test --locked -j1 -p pyro-webapi --test integration -- "indexeddb_js::$f" --exact --test-threads=1 > "$d/$arm-$f.log" 2>&1; grep -q '1 passed' "$d/$arm-$f.log"; done
 bash "$d/wpt.sh" "$arm"
done
bash "$d/wpt.sh" repeat
