#!/usr/bin/env bash
# Step 1 of 3: build the instrumented binary (target/<host>/release-pgo/pyro).
#
# `-Cprofile-generate` instruments every crate of the binary. `--cfg pyro_pgo_training` compiles in
# what gets every process's profile written: the profile runtime writes from an exit hook, so
# crates/pyro/src/main.rs turns SIGTERM/SIGINT into a normal exit, and
# crates/pyro-cdp/src/pgo_training.rs writes before fork mode's fast exits and gives each forked
# process a file of its own. Extra arguments go to cargo (for example `--locked`) and must match
# use.sh's.
#
# With PYRO_PGO_LLVM (common.sh) it first builds an instrumented JavaScriptCore archive
# (-fprofile-generate, target/pgo/jsc-generate) and links the binary against it.
source "$(dirname "$0")/common.sh"
mkdir -p "$PROFRAW_DIR"
rm -f "$PROFRAW_DIR"/*.profraw
if [[ -n "$JSC_PGO" ]]; then
    PYRO_JSC_DIR="$(pgo_jsc generate -fprofile-generate)"
    export PYRO_JSC_DIR
fi
RUSTFLAGS="-Cprofile-generate=$PROFRAW_DIR --cfg pyro_pgo_training $LINKER_FLAG" \
    cargo build --profile release-pgo -p pyro "${CARGO_TARGET[@]}" "$@"
echo "instrumented: $PGO_BIN"
