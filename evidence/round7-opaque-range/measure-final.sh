#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
printf '%s\n' "$1" > built-revision.txt
./candidate.sh "$1"
for p in probe prototype webidl boundaries lifecycle defaults raw-text reset-probe bigint; do
 node "$p.cjs" candidate "$W/pyro"
done
for p in probe prototype webidl raw-text reset-probe bigint defaults; do
 cmp "candidate-$p.json" "chrome158-$p.json"
done
python3 run.py candidate
python3 report.py
python3 classify.py
python3 families.py
./native.sh > native.log 2>&1
