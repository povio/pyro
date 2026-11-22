#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$sha"
echo "$sha" > built-revision.txt
node profiles.cjs candidate "$W/pyro"
node reconfigure.cjs candidate "$W/pyro"
node color.cjs candidate "$W/pyro"
node pattern.cjs candidate "$W/pyro"
node decode-pattern.cjs candidate-on-candidate "$W/pyro" candidate-pattern.json
node decode-pattern.cjs signed-on-candidate "$W/pyro" chrome158-pattern.json
python3 run.py candidate
python3 report.py
python3 families.py
