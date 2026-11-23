#!/bin/bash
set -eu
D="$(cd "$(dirname "$0")" && pwd)"
R="$HOME/Library/Caches/lightningpanda"
cd "$R/round6-reference-158/census/runner"
export WPT_CHROME_STATE_DIR="$D/chrome-state"
for arm in "$@"; do
  bin="$arm"
  if [ "$arm" = repeat ]; then bin=candidate; fi
  exe="$D/$bin"
  if [ "$arm" = chrome ]; then exe="$R/round6-reference-158/census/launch-158"; fi
  /usr/bin/time -p bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$R/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$R/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 1 --tests "$D/selection.json" --bin "$arm=$exe" --out "$D/$arm.json" > "$D/$arm-wpt.log" 2>&1
done
