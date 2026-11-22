#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-source"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0"
export CARGO_TARGET_DIR="$W/candidate-target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration custom_elements_js:: -- --skip self_inserting_connected_callbacks_end > "$W/native-web.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-dom --test integration adoption:: > "$W/native-dom.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-dom --test integration custom_reactions:: > "$W/native-reactions.log" 2>&1
