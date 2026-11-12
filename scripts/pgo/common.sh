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

# LP_PGO_LLVM=<dir>: an LLVM toolchain (bin/clang, bin/clang++) of the major version rustc is built
# on. With it the JavaScriptCore archive is built twice more, instrumented for the training and
# then with its profile, so the C++ is optimized by the same run as the Rust (one profile runtime,
# one file per process, one merge). Without it the archive is linked as built.
JSC_PGO=""
if [[ -n "${LP_PGO_LLVM:-}" ]]; then
    want="$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')"
    have="$("$LP_PGO_LLVM/bin/clang" --version | sed -n 's/.*clang version \([0-9]*\).*/\1/p' | head -1)"
    if [[ "$want" != "$have" ]]; then
        echo "LP_PGO_LLVM: clang $have, but rustc is built on LLVM $want (their profile formats differ)" >&2
        exit 1
    fi
    JSC_PGO=1
fi

# Builds the JavaScriptCore archive with LP_PGO_LLVM's clang and extra flags into
# $PGO_DIR/jsc-NAME (build directory $PGO_DIR/webkit-build-NAME) and prints the directory for
# LP_JSC_DIR. The WebKit checkout is the one scripts/jsc/build.sh uses (LP_WEBKIT_SRC).
pgo_jsc() {
    local name="$1" flags="$2" root
    if [[ "$(uname -s)" == Darwin ]]; then
        root="$HOME/Library/Caches/lightningpanda"
    else
        root="${XDG_CACHE_HOME:-$HOME/.cache}/lightningpanda"
    fi
    LP_JSC_CACHE="$PGO_DIR/jsc-$name" LP_JSC_BUILD="$PGO_DIR/webkit-build-$name" \
        LP_WEBKIT_SRC="${LP_WEBKIT_SRC:-$root/webkit-src}" \
        CC="$LP_PGO_LLVM/bin/clang" CXX="$LP_PGO_LLVM/bin/clang++" LP_JSC_CFLAGS="$flags" \
        "$ROOT/scripts/jsc/build.sh" >&2
    echo "$PGO_DIR/jsc-$name/$("$ROOT/scripts/jsc/cache-key.sh")"
}
