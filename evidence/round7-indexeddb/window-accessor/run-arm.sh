#!/bin/bash
set -eu
arm=$1
cd "$(dirname "$0")"
d=$PWD
cd git
export CARGO_BUILD_JOBS=1
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/$(scripts/jsc/cache-key.sh)"
diff -qr "$PYRO_JSC_DIR/inputs/patches" crates/pyro-jsc-sys/webkit/patches
git rev-parse HEAD > "$d/$arm-revision"
git diff > "$d/$arm-source.diff"
/usr/bin/time -p cargo build --locked --profile dev -p pyro > "$d/$arm-build.log" 2>&1
cp -c "$CARGO_TARGET_DIR/debug/pyro" "$d/$arm"
rc=0
/usr/bin/time -p cargo test --locked --profile dev -p pyro-webapi --test integration -- indexeddb_js:: --test-threads=1 > "$d/$arm-native.log" 2>&1 || rc=$?
echo "$rc" > "$d/$arm-native.exit"
bash "$d/wpt.sh" "$arm"
node "$d/identity.cjs" "$arm" "$d/$arm"
