#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
BUILD="$HOME/Library/Caches/lightningpanda/round7-interest"
cd "$BUILD/build-base"
export PYRO_JSC_DIR="$HOME/Library/Caches/lightningpanda/jsc/0e0dd7038a7a-6f4649e395e0" CARGO_TARGET_DIR="$BUILD/target" CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true CARGO_BUILD_JOBS=1
for t in byte_decoding regression_split_bom_beats_transport prescan_follows_the_html_algorithm an_undeclared_encoding_falls_back_to_the_inherited_one json_document_keeps_utf8_text_across_chunks_and_charset_hints; do
 cargo test --locked --offline --profile wptiter -p pyro-dom --test integration "loader::$t" -- --exact
done
cargo test --locked --offline --profile wptiter -p pyro-browser --test integration static_plaintext::
cargo test --locked --offline --profile wptiter -p pyro-browser --test integration child_frames::document_encoding_reaches_urls_and_forms_submitted_into_named_frames -- --exact
