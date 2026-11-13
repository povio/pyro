#!/usr/bin/env bash
# Installs the Linux build dependencies on Ubuntu 24.04 (CI runners and
# scripts/linux/Dockerfile): the toolchain for scripts/jsc/build.sh (WebKit needs
# a newer clang than the distribution's 18) and the Rust crates' system libraries.
set -euo pipefail
llvm="${LLVM:-21}"
sudo=(); [[ "$(id -u)" == 0 ]] || sudo=(sudo)
# A runner whose resolver does not answer for Ubuntu's mirrors (GitHub's arm64 runners, for hours on
# 2026-11-11: `Temporary failure resolving 'ports.ubuntu.com'`) gets their addresses from a public one.
for host in ports.ubuntu.com azure.archive.ubuntu.com archive.ubuntu.com security.ubuntu.com; do
    getent ahostsv4 "$host" >/dev/null 2>&1 && continue
    command -v dig >/dev/null || continue
    address="$(dig +short +time=5 +tries=2 @1.1.1.1 "$host" A | grep -E '^[0-9.]+$' | head -n 1 || true)"
    if [[ -n "$address" ]]; then
        echo "setup.sh: the resolver has no answer for $host; using $address" >&2
        echo "$address $host" | "${sudo[@]}" tee -a /etc/hosts >/dev/null
    fi
done
"${sudo[@]}" apt-get update
"${sudo[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential cmake ninja-build nasm ruby perl python3 libicu-dev patchelf pkg-config \
    git ca-certificates curl wget gnupg lsb-release software-properties-common \
    libfontconfig-dev libfreetype-dev
curl -sSf https://apt.llvm.org/llvm.sh | "${sudo[@]}" bash -s -- "$llvm"
for t in clang clang++ ld.lld; do "${sudo[@]}" ln -sf "/usr/bin/$t-$llvm" "/usr/local/bin/$t"; done
