#!/usr/bin/env bash
# Step 1 of 3: build the instrumented binary (target/release-pgo/pyro).
#
# `-Cprofile-generate` instruments every crate; `--cfg pyro_pgo_training` compiles in the
# SIGTERM/SIGINT handler (crates/pyro/src/main.rs) that lets train.sh stop
# `serve` with the profile written. Build scripts and proc macros run instrumented too
# and leave .profraw files behind; those are removed so only training data is merged.
# Extra arguments go to cargo (for example `--features perf-trace`).
# Takes about as long as a clean release build (M1 Max: 6.5 min).
source "$(dirname "$0")/common.sh"
mkdir -p "$PROFRAW_DIR"
RUSTFLAGS="-Cprofile-generate=$PROFRAW_DIR --cfg pyro_pgo_training $LINKER_FLAG" \
    cargo build --profile release-pgo -p pyro "$@"
rm -f "$PROFRAW_DIR"/*.profraw
echo "instrumented: $PGO_BIN (profiles will be written to $PROFRAW_DIR)"
