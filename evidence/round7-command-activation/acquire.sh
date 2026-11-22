#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
rev=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$rev"
echo "$rev" > built-revision.txt
for p in commands shadow reflection boundaries; do node "$p.cjs" candidate "$W/pyro"; done
python3 run.py candidate
python3 report.py
