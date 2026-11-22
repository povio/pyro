#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
for name in command_path_ends_below_the_host_that_hides_both_source_and_target command_target_reflection_observes_content_attribute_mutations toggle_and_command_event_interfaces command_and_toggle_source_follow_the_observing_listener popover_events_are_sync_beforetoggle_and_coalesced_toggle popovertarget_buttons command_activation_precedes_popover_applicability_and_preserves_dialog_invoker command_invokers details_toggle_events_and_exclusive_accordion dialog_close_requests_and_closed_by; do
 cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration "popover_dialog_js::$name" -- --exact
done
