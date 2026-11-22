#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
cargo test --locked --offline --profile wptiter -p pyro-canvas --test budget retained_bitmaps_count_toward_the_budget -- --exact
node --test crates/pyro-webapi/tests/fixtures/video_decoder_orientation.cjs
for t in image_data_round_trips_and_draw_image canvas_encodes_png_jpeg_and_blobs offscreen_canvas_and_image_bitmaps kept_image_bytes_are_counted_as_they_come_and_go image_bitmaps_clone_and_transfer_and_offscreen_canvases_transfer; do
  cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration "canvas_js::$t" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-canvas --test image_decoder
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration canvas_js::image_decoder_frame_charge_lasts_until_last_clone_closes -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration canvas_js::video_frame_draw_orientation_follows_destination_canvas_style -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration canvas_js::a_canvas_whose_control_went_offscreen_shows_its_frames -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration canvas_js::canvas_video_frame_snapshot_is_independent_and_charged_until_last_close -- --exact
