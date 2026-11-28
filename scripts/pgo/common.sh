# Shared by the scripts/pgo/*.sh steps (sourced, not run).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Raw profiles, their merge and the training logs; `target/` keeps them out of the tree.
PGO_DIR="${PYRO_PGO_DIR:-$ROOT/target/pgo}"
PROFRAW_DIR="$PGO_DIR/profraw"
MERGED="$PGO_DIR/merged.profdata"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
# Both builds name the host with --target, so RUSTFLAGS reach the binary's crates only, not build
# scripts and proc macros (which would otherwise run instrumented during the build). Cargo's crate
# hashes depend on --target: the two builds must agree, because a profile recorded without it
# matches no Rust function of a build with it, and nothing warns.
CARGO_TARGET=(--target "$HOST")
# The binary both builds produce (`--profile release-pgo`).
PGO_BIN="$ROOT/target/$HOST/release-pgo/pyro"
# RUSTFLAGS replaces .cargo/config.toml's flags, so the macOS linker wrapper that signs the binary
# with the JIT entitlement has to be repeated (README, "Build"). Linux has no such flag.
LINKER_FLAG=""
if [[ "$(uname -s)" == Darwin ]]; then
    LINKER_FLAG="-Clink-arg=-Bscripts/macos/linker"
fi
# llvm-profdata from rustup's llvm-tools component: it must match rustc's LLVM (the Command Line
# Tools' copy cannot read rustc's .profraw format).
if [[ "$HOST" == aarch64-unknown-linux-gnu ]]; then
    # lld, as .cargo/config.toml (this script's RUSTFLAGS replace it): GNU ld cannot reach the profile runtime from
    # an instrumented binary's .text.unlikely (relocation truncated to fit: R_AARCH64_CALL26).
    LINKER_FLAG="-Clink-arg=-fuse-ld=lld"
fi
PROFDATA="$(rustc --print sysroot)/lib/rustlib/$HOST/bin/llvm-profdata"
# sccache does not hash the profile file, and instrumented objects are not worth caching: build
# without the wrapper (an empty RUSTC_WRAPPER disables it).
export RUSTC_WRAPPER=
cd "$ROOT"

# PYRO_PGO_LLVM=<dir>: an LLVM toolchain (bin/clang, bin/clang++) of the major version rustc is built
# on. With it the JavaScriptCore archive is built twice more, instrumented for the training and
# then with its profile, so the C++ is optimized by the same run as the Rust (one profile runtime,
# one file per process, one merge). Without it the archive is linked as built.
JSC_PGO=""
if [[ -n "${PYRO_PGO_LLVM:-}" ]]; then
    want="$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')"
    have="$("$PYRO_PGO_LLVM/bin/clang" --version | sed -n 's/.*clang version \([0-9]*\).*/\1/p' | head -1)"
    if [[ "$want" != "$have" ]]; then
        echo "PYRO_PGO_LLVM: clang $have, but rustc is built on LLVM $want (their profile formats differ)" >&2
        exit 1
    fi
    JSC_PGO=1
fi

# PYRO_PGO_LTO=1 (with PYRO_PGO_LLVM; macOS arm64): the optimized build's JavaScriptCore archive is
# ThinLTO bitcode (scripts/jsc/build.sh, PYRO_JSC_LTO), which the link optimizes as a whole with the
# profile; ld loads PYRO_PGO_LLVM's libLTO for it (PYRO_LTO_LIBRARY, scripts/macos/linker/ld). The Rust
# code stays native: taking it into the same LTO (-Clinker-plugin-lto, so that the bindings inline
# the C API) measured no better. The instrumented build stays without LTO (the counters are placed
# before it, so the profile matches either way).
LTO=""
if [[ -n "${PYRO_PGO_LTO:-}" ]]; then
    [[ -n "$JSC_PGO" ]] || { echo "PYRO_PGO_LTO needs PYRO_PGO_LLVM" >&2; exit 1; }
    [[ "$HOST" == aarch64-apple-darwin ]] || { echo "PYRO_PGO_LTO: macOS arm64 only (not tried elsewhere)" >&2; exit 1; }
    LTO=1
fi

# Builds the JavaScriptCore archive with PYRO_PGO_LLVM's clang and extra flags (and PYRO_JSC_LTO=LTO,
# if given) into $PGO_DIR/jsc-NAME (build directory $PGO_DIR/webkit-build-NAME) and prints the
# directory for PYRO_JSC_DIR. The WebKit checkout is the one scripts/jsc/build.sh uses (PYRO_WEBKIT_SRC).
pgo_jsc() {
    local name="$1" flags="$2" lto="${3:-}" root
    if [[ "$(uname -s)" == Darwin ]]; then
        root="$HOME/Library/Caches/pyro"
    else
        root="${XDG_CACHE_HOME:-$HOME/.cache}/pyro"
    fi
    PYRO_JSC_CACHE="$PGO_DIR/jsc-$name" PYRO_JSC_BUILD="$PGO_DIR/webkit-build-$name" \
        PYRO_WEBKIT_SRC="${PYRO_WEBKIT_SRC:-$root/webkit-src}" \
        CC="$PYRO_PGO_LLVM/bin/clang" CXX="$PYRO_PGO_LLVM/bin/clang++" PYRO_JSC_CFLAGS="$flags" PYRO_JSC_LTO="$lto" \
        "$ROOT/scripts/jsc/build.sh" >&2
    echo "$PGO_DIR/jsc-$name/$("$ROOT/scripts/jsc/cache-key.sh")"
}
