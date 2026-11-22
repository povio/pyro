#!/bin/bash
set -eu
D="$(cd "$(dirname "$0")" && pwd)"
R="$HOME/Library/Caches/lightningpanda"
cd "$R/round6-reference-158/census/runner"
for arm in base candidate repeat; do
  bin="$arm"
  if [ "$arm" = repeat ]; then bin=candidate; fi
  /usr/bin/time -p bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$R/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$R/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 1 --tests "$D/selection.json" --bin "lp=$D/$bin" --out "$D/$arm.json" > "$D/$arm-wpt.log" 2>&1
done
