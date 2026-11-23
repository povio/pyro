#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
node --test crates/pyro-webapi/tests/fixtures/audio_encoder*.cjs
cargo test --locked --offline --profile wptiter -p pyro-media --test audio_packets
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib audio_data::tests::
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration messaging_js::audio_data_
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration dom_js::encoded_chunks_
cargo test --locked --offline --profile wptiter -p pyro-media --test audio_encode
