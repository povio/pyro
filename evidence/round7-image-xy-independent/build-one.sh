#!/bin/sh
set -eu
D=$(cd "$(dirname "$0")" && pwd)
R="$HOME/Library/Caches/lightningpanda"
label=$1
revision=$(git rev-parse "$2^{commit}")
target=$3
git switch --detach "$revision"
test -z "$(git status --porcelain --untracked-files=no)"
test "$(git rev-parse HEAD)" = "$revision"
date -u > "$D/$label-build-start"
PYRO_BUILD_JOBS="${BUILD_JOBS:-1}" CARGO_BUILD_JOBS="${BUILD_JOBS:-1}" "$R/round6-noncss-measure/build-exact" "$target" > "$D/$label-build.log" 2>&1
test "$(git rev-parse HEAD)" = "$revision"
cp -c "$target/wptiter/pyro" "$D/pyro-$label"
chmod 555 "$D/pyro-$label"
date -u > "$D/$label-build-end"
{ echo "$revision"; git rev-parse HEAD^{tree}; echo "$target"; scripts/jsc/cache-key.sh; rustc -Vv; shasum -a 256 "$D/pyro-$label" "$R/jsc/$(scripts/jsc/cache-key.sh)/lib/libJavaScriptCore.a"; } > "$D/$label-provenance.txt"
