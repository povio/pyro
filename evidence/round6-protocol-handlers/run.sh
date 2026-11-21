#!/bin/sh
set -eu
N="$HOME/Library/Caches/lightningpanda/round6-noncss-measure/protocol-handlers"
C="$HOME/Library/Caches/lightningpanda"
cd "$C/round6-reference-158/census/runner"
exec bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$C/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$C/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 --tests "$N/selection.json" --bin "lp=$2" --out "$N/$1.json"
