#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
./candidate.sh "$1"
for p in classic-lazy classic idl echo lifecycle abort flow worker conversion teardown; do
 node "$p.cjs" candidate "$W/pyro"
done
python3 run.py candidate
python3 report.py
python3 classify.py
python3 families.py
./native.sh > native.log 2>&1
