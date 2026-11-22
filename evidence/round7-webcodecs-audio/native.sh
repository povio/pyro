#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
node --test crates/pyro-webapi/tests/fixtures/video_decoder_orientation.cjs
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib audio_data::
for t in decodes_wav decodes_mp3 decodes_aac_m4a decodes_ogg_vorbis_stereo decodes_ogg_opus decodes_webm_opus decodes_flac resamples_to_the_context_rate packet_av1_keeps_sample_tags_and_restarts_after_flush packet_avc_and_annex_b_keep_sample_tags_and_restart_after_flush; do
  cargo test --locked --offline --profile wptiter -p pyro-media --test decode "$t" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration messaging_js::audio_data_
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration messaging_js::encoded_chunk_and_image_data_keep_graph_aliases_in_a_message -- --exact
cargo test --locked --offline --profile wptiter -p pyro-webapi --test integration dom_js::encoded_chunks_
cargo test --locked --offline --profile wptiter -p pyro-media --test audio_packets
