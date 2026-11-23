#!/bin/sh
set -eu
D=$(cd "$(dirname "$0")" && pwd)
R="$HOME/Library/Caches/lightningpanda"
cd "$R/round6-reference-158/census/runner"
export WPT_CHROME_STATE_DIR="$D"
exec bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$R/wptci/wpt-full" --server wptserve --testdriver --dirs css --jobs 1 --tests "$D/urls.json" --bin "lp=$2" --out "$D/$1.json"
