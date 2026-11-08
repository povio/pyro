# Shared by the scripts/pgo/*.sh steps (sourced, not run).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Raw profiles from the training run and their merge; `target/` keeps them out of the tree.
PGO_DIR="${LP_PGO_DIR:-$ROOT/target/pgo}"
PROFRAW_DIR="$PGO_DIR/profraw"
MERGED="$PGO_DIR/merged.profdata"
# The binary both builds produce (`--profile release-pgo`).
PGO_BIN="$ROOT/target/release-pgo/lightningpanda"
# RUSTFLAGS replaces .cargo/config.toml's flags, so the linker wrapper that signs
# the binary with the JIT entitlement has to be repeated (README, "Build").
LINKER_FLAG="-Clink-arg=-Bscripts/macos/linker"
# sccache does not hash the profile file, and instrumented objects are not worth
# caching: build without the wrapper (an empty RUSTC_WRAPPER disables it).
export RUSTC_WRAPPER=
cd "$ROOT"
