#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-source"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0"
export CARGO_TARGET_DIR="$W/candidate-target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib window_features::tests >"$W/native-parser.log" 2>&1
for case_name in named_noopener_creates_a_fresh_context_with_the_requested_name popup_keeps_its_opener_and_a_copy_of_the_session_storage stand_in_storage_before_the_page_exists_is_the_popups_area broadcast_channel_reaches_same_origin_documents_of_the_browser_context; do
  cargo test --locked --offline --profile wptiter -p pyro-browser --test integration "popups::$case_name" -- --exact >>"$W/native-browser.log" 2>&1
done
