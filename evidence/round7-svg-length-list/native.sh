#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-dom --lib svg_point_list::
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration svg_animated_enumeration_js::length_attributes_reflect_as_svg_lengths -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration svg_animated_enumeration_js::svg_lengths_of_one_character_new_interfaces_detached_lengths_and_css_units -- --exact
