#!/bin/bash
# usage: run.sh <lightningpanda binary> <output directory> [generator names]
# Probes each fixture set with the binary and compares it with Chrome 156's summary. Exits 1 if a generator fails.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
bin="$1"
out="$2"
shift 2
mkdir -p "$out"
cd "$here"
if [ $# -eq 0 ]; then
  set -- $(for g in generators/fx-*.mjs; do basename "$g" .mjs | sed 's/^fx-//'; done)
fi
status=0
for g in "$@"; do
  if ! node probe.mjs "generators/fx-$g.mjs" "$bin" "$out/$g.json" 2>&1 | grep -v 'no word list'; then :; fi
  if [ ! -f "$out/$g.json" ]; then
    echo "$g: probe failed"
    status=1
    continue
  fi
  echo "== $g"
  python3 compare.py "chrome-156/$g.json" "$out/$g.json"
done
exit $status
