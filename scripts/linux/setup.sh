#!/usr/bin/env bash
# Installs the Linux build dependencies on Ubuntu 24.04 (CI runners and
# scripts/linux/Dockerfile): the toolchain for scripts/jsc/build.sh (WebKit needs
# a newer clang than the distribution's 18) and the Rust crates' system libraries.
set -euo pipefail
llvm="${LLVM:-21}"
sudo=(); [[ "$(id -u)" == 0 ]] || sudo=(sudo)
"${sudo[@]}" apt-get update
"${sudo[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential cmake ninja-build ruby perl python3 libicu-dev patchelf pkg-config \
    git ca-certificates curl wget gnupg lsb-release software-properties-common \
    libfontconfig-dev libfreetype-dev
curl -sSf https://apt.llvm.org/llvm.sh | "${sudo[@]}" bash -s -- "$llvm"
for t in clang clang++ ld.lld; do "${sudo[@]}" ln -sf "/usr/bin/$t-$llvm" "/usr/local/bin/$t"; done
