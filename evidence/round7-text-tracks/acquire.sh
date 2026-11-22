#!/bin/sh
set -eu
W=$(cd "$(dirname "$0")" && pwd)
cd "$W"
rev=$(git -C source rev-parse HEAD)
test -z "$(git -C source status --porcelain)"
./candidate.sh "$rev"
echo "$rev" > built-revision.txt
python3 run.py candidate > acquisition-wpt.log 2>&1 &
wpt_pid=$!
failed=0
for name in captions replacement region realms playback reattach equivalent; do
  node "$name.cjs" candidate "$W/pyro" > "candidate-$name.log" 2>&1 || failed=1
done
wait "$wpt_pid"
python3 report.py
python3 cohorts.py
exit "$failed"
