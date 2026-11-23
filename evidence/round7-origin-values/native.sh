#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-source"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$W/candidate-target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib origin::tests:: >"$W/native-origin.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration dom_js::origin_values_use_native_records_and_preserve_opaque_identity -- --exact >"$W/native-web.log" 2>&1
for test_name in messaging::same_origin_frames_post_cloned_messages_with_the_incumbent_source_and_origin messaging::popup_and_opener_see_each_other_as_message_source; do
 cargo test --locked --offline --profile wptiter -p pyro-browser --test integration "$test_name" -- --exact >>"$W/native-messaging.log" 2>&1
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration event_source_js::event_source_message_retains_its_response_origin -- --exact >"$W/native-eventsource.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-browser --test integration messaging::popup_message_origin_uses_the_sender_at_post_time -- --exact >>"$W/native-messaging.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-dom --lib blob_hyperlinks::tests:: >"$W/native-blob-links.log" 2>&1
