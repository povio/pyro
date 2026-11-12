# Shared by the scripts/pgo/*.sh steps (sourced, not run).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Raw profiles, their merge and the training logs; `target/` keeps them out of the tree.
PGO_DIR="${LP_PGO_DIR:-$ROOT/target/pgo}"
PROFRAW_DIR="$PGO_DIR/profraw"
MERGED="$PGO_DIR/merged.profdata"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
# Both builds name the host with --target, so RUSTFLAGS reach the binary's crates only, not build
# scripts and proc macros (which would otherwise run instrumented during the build). Cargo's crate
# hashes depend on --target: the two builds must agree, because a profile recorded without it
# matches no Rust function of a build with it, and nothing warns.
CARGO_TARGET=(--target "$HOST")
# The binary both builds produce (`--profile release-pgo`).
PGO_BIN="$ROOT/target/$HOST/release-pgo/lightningpanda"
# RUSTFLAGS replaces .cargo/config.toml's flags, so the macOS linker wrapper that signs the binary
# with the JIT entitlement has to be repeated (README, "Build"). Linux has no such flag.
LINKER_FLAG=""
if [[ "$(uname -s)" == Darwin ]]; then
    LINKER_FLAG="-Clink-arg=-Bscripts/macos/linker"
fi
# llvm-profdata from rustup's llvm-tools component: it must match rustc's LLVM (the Command Line
# Tools' copy cannot read rustc's .profraw format).
PROFDATA="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-profdata"
# sccache does not hash the profile file, and instrumented objects are not worth caching: build
# without the wrapper (an empty RUSTC_WRAPPER disables it).
export RUSTC_WRAPPER=
cd "$ROOT"
