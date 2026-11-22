#!/bin/bash
set -eu
cd "$(dirname "$0")"
export CARGO_TARGET_DIR=/private/tmp/idb-controls.ShrpdZ/target-ec01f96fee1ebcfe0ed32c7cd65c2f56cd28ce50
bash run-arm.sh parent
git -C git restore crates/pyro-webapi/tests/indexeddb_js.rs
git -C git checkout --detach bdbb41a00f8b9df720027f27927c9cdeda388b0a
bash run-arm.sh candidate
bash wpt.sh repeat chrome
node identity.cjs chrome ignored
python3 compare.py
