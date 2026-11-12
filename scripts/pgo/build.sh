#!/usr/bin/env bash
# Profile-guided release build of `lightningpanda`, end to end:
#
#   scripts/pgo/build.sh [cargo options, e.g. --locked]
#   LP_PGO_LLVM=<LLVM release> scripts/pgo/build.sh                # JavaScriptCore profiled too
#
# 1. generate.sh: instrumented build (-Cprofile-generate, --cfg lp_pgo_training); with
#                 LP_PGO_LLVM against an instrumented JavaScriptCore archive (target/pgo/jsc-generate)
# 2. train.sh:    bench/scripts/cdp-perf.mjs, every scenario once, `serve` and `serve --fork`;
#                 profiles merged into target/pgo/merged.profdata
# 3. use.sh:      optimized build (-Cprofile-use); with LP_PGO_LLVM against the archive rebuilt
#                 with the same profile (target/pgo/jsc-use)
#
# Result: target/<host>/release-pgo/lightningpanda (`rustc -vV` names the host): the binary of
# `cargo build --release` (profile `release-pgo` inherits `release`), optimized by the profile.
# Plain `cargo build` is unaffected. Ship it in place of the `--release` binary (strip, sign and
# bundle as the release jobs do); the lp-render comparison worker does not need it. What each
# step buys, measured: README, "Profile-guided optimization".
#
# From a clean checkout this needs what a release build needs (README "Build": the JavaScriptCore
# archive from scripts/jsc/build.sh, or LP_JSC_DIR) plus
#   rustup component add llvm-tools     llvm-profdata matching rustc's LLVM
#   bun                                 runs the training (bench/node_modules is installed)
# and for LP_PGO_LLVM an LLVM release of the major version rustc is built on (`rustc -vV`, "LLVM
# version"; macOS: github.com/llvm/llvm-project/releases, LLVM-<version>-macOS-ARM64.tar.xz;
# Linux: apt.llvm.org's /usr/lib/llvm-<major>), cmake and ninja (scripts/jsc/build.sh; its
# WebKit checkout, LP_WEBKIT_SRC, is shared with it).
# Time (M1 Max, other builds running): Rust only 11 min (two release builds and 2 min of
# training); with LP_PGO_LLVM 23 min (two more JavaScriptCore builds; the instrumented one is kept
# in target/pgo and rebuilt only when WebKit's revision or patches change).
#
# Knobs: LP_PGO_DIR (default target/pgo); LP_PGO_EXTRA, LP_PGO_EXTRA_WEIGHT, LP_PGO_PORT (an
# extra training workload, train.sh); LP_PGO_PROFILE (use.sh: build from a kept profile);
# LP_PGO_LLVM (common.sh); JOBS (JavaScriptCore build jobs).
set -euo pipefail
here="$(dirname "$0")"
"$here/generate.sh" "$@"
"$here/train.sh"
"$here/use.sh" "$@"
