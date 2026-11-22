#!/bin/bash
set -eo pipefail
source "$HOME/Library/Caches/lightningpanda/pyro-env.sh"
cd /tmp/r7-idb-eval
if lsof -nP -iTCP:24820-24829 -sTCP:LISTEN; then exit 1; fi
bun runner/bench/wpt/run.mjs --wpt "$WPT_DIR" --server wptserve --testdriver --worker-globals --dirs IndexedDB --jobs 2 --tests urls.json --bin "$1=$PWD/$1" --out "$1.json" >"$1.log" 2>&1
