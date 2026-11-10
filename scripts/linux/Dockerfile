# Linux build environment for lightningpanda: patched JSCOnly (scripts/jsc/build.sh)
# and the Rust workspace. Used by scripts/linux/build.sh; CI runs the same
# scripts/linux/setup.sh on its Ubuntu runners.
FROM ubuntu:24.04
COPY setup.sh /tmp/setup.sh
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
    && /tmp/setup.sh && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
    && chmod -R a+w /opt/rustup /opt/cargo
ENV CC=clang CXX=clang++
WORKDIR /src
