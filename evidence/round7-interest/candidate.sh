#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W/build-base"
test -x "$W/base-pyro"
git diff --quiet
git switch --detach "$1"
PYRO_BUILD_JOBS=1 CARGO_BUILD_JOBS=1 "$HOME/Library/Caches/lightningpanda/round6-noncss-measure/build-exact" "$W/target" > "$W/candidate-build.log" 2>&1
cp "$W/target/wptiter/pyro" "$W/pyro"
shasum -a 256 "$W/pyro" > "$W/pyro.sha256"
