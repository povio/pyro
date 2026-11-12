#!/usr/bin/env bash
# Step 1 of 3: build the instrumented binary (target/<host>/release-pgo/lightningpanda).
#
# `-Cprofile-generate` instruments every crate of the binary. `--cfg lp_pgo_training` compiles in
# what gets every process's profile written: the profile runtime writes from an exit hook, so
# crates/lightningpanda/src/main.rs turns SIGTERM/SIGINT into a normal exit, and
# crates/lp-cdp/src/pgo_training.rs writes before fork mode's fast exits and gives each forked
# process a file of its own. Extra arguments go to cargo (for example `--locked`) and must match
# use.sh's.
source "$(dirname "$0")/common.sh"
mkdir -p "$PROFRAW_DIR"
rm -f "$PROFRAW_DIR"/*.profraw
RUSTFLAGS="-Cprofile-generate=$PROFRAW_DIR --cfg lp_pgo_training $LINKER_FLAG" \
    cargo build --profile release-pgo -p lightningpanda "${CARGO_TARGET[@]}" "$@"
echo "instrumented: $PGO_BIN"
