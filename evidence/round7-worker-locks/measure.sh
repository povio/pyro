#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
printf '%s\n' "$1" > built-revision.txt
./candidate.sh "$1"
for p in probe queue-probe context-probe partition-probe settlement flavors pending-exit nested-surface idl-probe; do node "$p.cjs" candidate "$W/pyro"; done
for p in probe queue-probe context-probe partition-probe settlement pending-exit idl-probe; do
 cmp "candidate-$p.json" "chrome158-$p.json"
done
python3 run.py candidate
python3 report.py
python3 classify.py
python3 families.py
./native.sh > native.log 2>&1
