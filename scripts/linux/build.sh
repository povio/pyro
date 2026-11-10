#!/usr/bin/env bash
# Builds lightningpanda for Linux inside Docker (scripts/linux/Dockerfile).
#
#   scripts/linux/build.sh                 # patched JSC (cached) + release binary
#   scripts/linux/build.sh cargo test ...  # run any command in the container instead
#
# The JSC build, cargo registry and target dir live in named Docker volumes so
# reruns are incremental. PLATFORM picks the architecture (default: the host's,
# linux/arm64 on Apple silicon; linux/amd64 runs under emulation there and is slow).
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
platform="${PLATFORM:-}"
arch="${platform#linux/}"; arch="${arch:-native}"
image="lightningpanda-linux-build:$arch"

docker build ${platform:+--platform "$platform"} -t "$image" "$root/scripts/linux"

cmd=("$@")
if [[ ${#cmd[@]} -eq 0 ]]; then
    cmd=(bash -c 'scripts/jsc/build.sh && cargo build --release -p lightningpanda')
fi

docker run --rm ${platform:+--platform "$platform"} \
    -v "$root":/src \
    -v "lightningpanda-jsc-$arch":/root/.cache/lightningpanda \
    -v "lightningpanda-cargo-$arch":/opt/cargo/registry \
    -v "lightningpanda-target-$arch":/target \
    -e CARGO_TARGET_DIR=/target \
    -e JOBS \
    "$image" "${cmd[@]}"
