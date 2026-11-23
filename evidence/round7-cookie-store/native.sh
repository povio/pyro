#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-source"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$W/candidate-target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-net --lib cookies::tests:: >"$W/native-jar.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-net --lib client::cookie_tests:: >"$W/native-network.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib cookie_store::subscriptions::tests:: >"$W/native-subscriptions.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration events_misc_js::cookie_changes_deliver_trusted_frozen_lists_after_the_mutation -- --exact >"$W/native-events.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration events_misc_js::cookie_change_listener_registered_after_mutation_in_the_same_turn_receives_the_event -- --exact >>"$W/native-events.log" 2>&1
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration events_misc_js::cookie_changes_to_one_name_in_a_turn_dispatch_separate_ordered_events -- --exact >>"$W/native-events.log" 2>&1
for test_name in service_worker::cookie_subscriptions_survive_update_and_allow_cleanup_after_unregister child_frames::cookie_partition_and_site_for_cookies_follow_the_whole_frame_chain service_worker::shared_workers_are_shared_between_documents_by_url_and_name service_worker::service_worker_keeps_working_after_the_registering_page_navigates service_worker::service_worker_answers_navigations_and_subresources; do
 cargo test --locked --offline --profile wptiter -p pyro-browser --test integration "$test_name" -- --exact >>"$W/native-browser.log" 2>&1
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration indexeddb_js::window_indexeddb_is_an_own_readonly_accessor -- --exact >"$W/native-existing-accessor.log" 2>&1
