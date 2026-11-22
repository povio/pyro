#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
./candidate.sh "$1"
python3 run.py candidate
for probe in probe parse-probe cross-document font-probe relative-probe; do node "$probe.cjs" candidate "$W/pyro"; done
python3 report.py
python3 classify.py
python3 families.py
./native.sh > native.log 2>&1
