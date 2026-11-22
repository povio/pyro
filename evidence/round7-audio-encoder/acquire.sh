#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$sha"
echo "$sha" > built-revision.txt
node capabilities.cjs candidate "$W/pyro"
node segments.cjs candidate "$W/pyro"
node stereo.cjs candidate "$W/pyro"
node rates.cjs candidate "$W/pyro"
node tone.cjs candidate "$W/pyro"
node decode-tone.cjs candidate-on-candidate "$W/pyro" candidate-tone.json
node decode-tone.cjs signed-on-candidate "$W/pyro" chrome158-tone.json
python3 run.py candidate
python3 report.py
python3 families.py
