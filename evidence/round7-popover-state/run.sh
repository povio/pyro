#!/bin/sh
set -eu
N="$(cd "$(dirname "$0")" && pwd)"
R="$HOME/Library/Caches/lightningpanda"
cd "$R/round6-reference-158/census/runner"
exec bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$R/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$R/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 2 --tests "$N/selection.json" --bin "lp=$2" --out "$N/$1.json"
