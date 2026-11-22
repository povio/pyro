#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-source"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$W/candidate-target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib opfs::store::tests >"$W/native-store.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration crypto_js::file_stream_writes_observe_queued_bytes_and_abort_preserves_the_file -- --exact >"$W/native-stream.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration crypto_js::dedicated_workers_expose_streams_file_reader_crypto_keys_and_storage -- --exact >"$W/native-worker.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration crypto_js::dedicated_worker_file_system_interfaces_are_own_data_properties -- --exact >"$W/native-descriptors.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration crypto_js::navigator_storage_reports_usage_and_no_persistence -- --exact >"$W/native-manager.log" 2>&1
