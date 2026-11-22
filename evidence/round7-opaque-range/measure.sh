#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
./candidate.sh "$1"
python3 run.py candidate
for p in probe prototype webidl boundaries lifecycle defaults raw-text reset-probe; do
 node "$p.cjs" candidate "$W/pyro"
done
python3 report.py
python3 classify.py
python3 families.py
./native-red.sh > native-red.log 2>&1
rg -q "dom_idl_arguments_abstract_range_and_fullscreen.*panicked" native-red.log
