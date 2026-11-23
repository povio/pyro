#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$sha"
echo "$sha" > built-revision.txt
node pixels.cjs candidate "$W/pyro"
node lifecycle.cjs candidate "$W/pyro"
node metadata.cjs candidate "$W/pyro"
node orientation.cjs candidate "$W/pyro"
python3 run.py candidate
python3 report.py
python3 families.py
