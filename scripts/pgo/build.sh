#!/usr/bin/env bash
# Profile-guided release build of `lightningpanda`, end to end:
#
#   scripts/pgo/build.sh [cargo options, e.g. --locked]
#
# 1. generate.sh: instrumented build (-Cprofile-generate, --cfg lp_pgo_training)
# 2. train.sh:    bench/scripts/cdp-perf.mjs, every scenario once, `serve` and `serve --fork`;
#                 profiles merged into target/pgo/merged.profdata
# 3. use.sh:      optimized build (-Cprofile-use)
#
# Result: target/<host>/release-pgo/lightningpanda (`rustc -vV` names the host): the binary of
# `cargo build --release` (profile `release-pgo` inherits `release`), optimized by the profile.
# Plain `cargo build` is unaffected. Ship it in place of the `--release` binary (strip, sign and
# bundle as the release jobs do); the lp-render comparison worker does not need it. What it
# buys, measured: README, "Profile-guided optimization".
#
# From a clean checkout this needs what a release build needs (README "Build": the JavaScriptCore
# archive from scripts/jsc/build.sh, or LP_JSC_DIR) plus
#   rustup component add llvm-tools     llvm-profdata matching rustc's LLVM
#   bun                                 runs the training (bench/node_modules is installed)
# Time (M1 Max, other builds running): 11 min, two release builds and 2 min of training.
#
# Knobs: LP_PGO_DIR (default target/pgo); LP_PGO_EXTRA, LP_PGO_EXTRA_WEIGHT, LP_PGO_PORT (an
# extra training workload, train.sh); LP_PGO_PROFILE (use.sh: build from a kept profile).
set -euo pipefail
here="$(dirname "$0")"
"$here/generate.sh" "$@"
"$here/train.sh"
"$here/use.sh" "$@"
