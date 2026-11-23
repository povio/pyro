#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-dom --test integration forms_values::
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration svg_animated_enumeration_js::dom_idl_arguments_abstract_range_and_fullscreen -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration forms_js::form_reset -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration custom_elements_js::form_associated_callbacks_follow_owner_disabled_state_and_reset -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration dom_js::controlled_changed_values_reset_selection_to_the_utf16_end -- --exact
