#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-net --test integration websocket::stream_
for test in websocket::websocket_echoes_text_and_binary_and_closes_cleanly websocket::websocket_activity_is_observed_in_order websocket::websocket_sends_cookies_and_credentials_and_reports_set_cookie websocket_h2::h2_websocket_echoes_on_the_pooled_connection_and_closes_cleanly websocket_deflate::deflate_is_offered_and_messages_round_trip_with_context_takeover; do
 cargo test --locked --offline --profile wptiter -p pyro-net --test integration "$test" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib websocket::tests::
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration websocket_js::
