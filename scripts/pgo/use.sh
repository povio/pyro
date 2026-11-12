#!/usr/bin/env bash
# Step 3 of 3: build the optimized binary (target/<host>/release-pgo/lightningpanda, replacing
# the instrumented one) from target/pgo/merged.profdata, or from LP_PGO_PROFILE=<file>, a profile
# kept from a training of the same sources.
#
# Cargo does not hash the profile's content, so the build reads it under a name that carries its
# hash: a new profile rebuilds, the same one is up to date. Extra arguments go to cargo and must
# match generate.sh's.
#
# Four "function control flow change detected (hash mismatch)" warnings for lp-cdp are expected:
# the functions that call the training hooks (`--cfg lp_pgo_training`) differ between the two
# builds and lose their counts (fork and exit paths, none of them hot).
source "$(dirname "$0")/common.sh"
profile="${LP_PGO_PROFILE:-$MERGED}"
[[ -f "$profile" ]] || { echo "no $profile: run scripts/pgo/train.sh first" >&2; exit 1; }
if command -v shasum >/dev/null; then
    sum="$(shasum -a 256 < "$profile" | cut -c1-12)"
else
    sum="$(sha256sum < "$profile" | cut -c1-12)"
fi
use="$PGO_DIR/use-$sum.profdata"
[[ -f "$use" ]] || cp "$profile" "$use"
if [[ -n "$JSC_PGO" ]]; then
    # The archive's directory name does not cover the profile: start it afresh (the build
    # directory stays, and rebuilds what the new profile path changes, which is everything).
    name="use${LTO:+-lto}"
    rm -rf "$PGO_DIR/jsc-$name"
    LP_JSC_DIR="$(pgo_jsc "$name" "-fprofile-use=$use -Wno-profile-instr-unprofiled -Wno-profile-instr-out-of-date -Wno-backend-plugin" "${LTO:+thin}")"
    export LP_JSC_DIR
fi
if [[ -n "$LTO" ]]; then
    export LP_LTO_LIBRARY="$LP_PGO_LLVM/lib/libLTO.dylib"
fi
RUSTFLAGS="-Cprofile-use=$use $LINKER_FLAG" \
    cargo build --profile release-pgo -p lightningpanda "${CARGO_TARGET[@]}" "$@"
echo "optimized: $PGO_BIN"
