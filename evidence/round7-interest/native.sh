#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$W/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-dom --test integration interest_invoker_reflection
cargo test --locked --offline --profile wptiter -p pyro-dom --test integration dispatch_targets::
