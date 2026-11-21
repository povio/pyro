#!/bin/sh
set -eu
D="$HOME/Library/Caches/lightningpanda/round6-noncss-measure"
N="$D/template-navigator-combined"
C="$D/.."
cd "$C/round6-reference-158/census/runner"
for cohort in focus guard navigator; do
 jobs=2
 if [ "$cohort" = navigator ]; then jobs=4; fi
 bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$C/wptci/wpt-full" --server wptserve --testdriver --http2 --generated-globals "$C/round6-inventory/MANIFEST-pinned.json" --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs "$jobs" --tests "$N/$cohort.urls.json" --bin "stack=$N/candidate" --out "$N/$cohort.json" > "$N/$cohort.log" 2>&1
done
