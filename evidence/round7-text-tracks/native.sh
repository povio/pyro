#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
node --test crates/pyro-webapi/tests/fixtures/webvtt*.cjs crates/pyro-webapi/tests/fixtures/text_track*.cjs
for t in media::idl_state_is_settable_in_strict_code media::removing_the_current_source_keeps_the_pointer_in_place media_http::a_live_stream_does_not_hold_the_load_event; do
  cargo test --locked --offline --profile wptiter -p pyro-browser --test integration "$t" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-browser --test integration media::track_fetches_use_the_media_parent_and_keep_completed_cues_on_reattachment -- --exact
