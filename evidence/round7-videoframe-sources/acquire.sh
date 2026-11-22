#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$sha"
echo "$sha" > built-revision.txt
node snapshot.cjs candidate "$W/pyro"
node geometry.cjs candidate "$W/pyro"
node conversion.cjs candidate "$W/pyro"
python3 run.py candidate
python3 report.py
python3 families.py
