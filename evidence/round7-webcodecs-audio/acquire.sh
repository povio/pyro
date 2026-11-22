#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
sha=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$sha"
echo "$sha" > built-revision.txt
for probe in ordinary transport close capability pcm; do
 node "$probe.cjs" candidate "$W/pyro"
done
python3 run.py candidate
python3 video-queue/run.py candidate
python3 report.py
python3 video-queue/report.py
python3 families.py
