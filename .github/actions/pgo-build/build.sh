#!/usr/bin/env bash
set -euo pipefail
cd "${GITHUB_WORKSPACE:?}"

export CARGO_TARGET_DIR="$PWD/target"
export PYRO_PGO_DIR="$CARGO_TARGET_DIR/pgo"
export PYRO_PGO_LLVM="$PWD/.ci/llvm-pgo"
export PYRO_WEBKIT_SRC="$PYRO_PGO_DIR/webkit-src"
host_profile="$CARGO_TARGET_DIR/release-pgo"
target_profile="$CARGO_TARGET_DIR/aarch64-apple-darwin/release-pgo"

clear_profile_intermediates() {
    rm -rf -- "$1"/{.fingerprint,build,deps,examples,incremental}
}

cleanup() {
    local result=0
    clear_profile_intermediates "$host_profile" || result=1
    clear_profile_intermediates "$target_profile" || result=1
    rm -rf -- "$PYRO_WEBKIT_SRC" "$PYRO_PGO_DIR"/{webkit-build-*,profraw,jsc-use*,jsc-generate/*.tmp} \
        "$PYRO_PGO_DIR"/{merged,part-*,use-*}.profdata || result=1
    return "$result"
}

if [[ "${1:-}" == --cleanup ]]; then
    cleanup
    exit
fi

cleanup_on_exit() {
    local status="$1"
    if ! cleanup; then
        echo '::warning::PGO scratch cleanup failed' >&2
        if (( status == 0 )); then status=1; fi
    fi
    exit "$status"
}

trap 'cleanup_on_exit "$?"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

df -h .
scripts/pgo/generate.sh "$@"
rm -rf -- "$PYRO_PGO_DIR/webkit-build-generate"
scripts/pgo/train.sh
clear_profile_intermediates "$target_profile"
rm -f -- "$target_profile/pyro"
rm -rf -- "$PYRO_PGO_DIR/profraw" "$PYRO_PGO_DIR"/part-*.profdata
df -h .
scripts/pgo/use.sh "$@"
