#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
node --test crates/pyro-webapi/tests/fixtures/video_encoder*.cjs crates/pyro-webapi/tests/fixtures/video_decoder_orientation.cjs
cargo test --locked --offline --profile wptiter -p pyro-media --features h264 --test video_encode
for t in packet_avc_and_annex_b_keep_sample_tags_and_restart_after_flush decodes_h264_mp4_with_b_frames packet_av1_keeps_sample_tags_and_restarts_after_flush; do
  cargo test --locked --offline --profile wptiter -p pyro-media --features h264 --test decode "$t" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration canvas_js::canvas_video_frame_snapshot_is_independent_and_charged_until_last_close -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration dom_js::encoded_chunks_
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib video_encoder::tests::
