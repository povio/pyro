#!/bin/sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cache=$(dirname "$(dirname "$here")")
cd "$cache/round6-reference-158/census/runner"
for kind in focus guard; do
  bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$cache/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$cache/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 --tests "$here/$kind.json" --bin "lp=$2" --out "$here/$1-$kind.json" > "$here/$1-$kind.log" 2>&1
done
