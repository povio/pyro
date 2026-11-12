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
RUSTFLAGS="-Cprofile-use=$use $LINKER_FLAG" \
    cargo build --profile release-pgo -p lightningpanda "${CARGO_TARGET[@]}" "$@"
echo "optimized: $PGO_BIN"
