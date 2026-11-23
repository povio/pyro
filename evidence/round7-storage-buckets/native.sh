#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
for module in storage_buckets::catalog::tests:: cache_storage::tests:: indexeddb::tests:: opfs::store::tests::; do
 cargo test --locked --offline --profile wptiter -p pyro-webapi --lib "$module"
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration indexeddb_js::
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration storage_js::
