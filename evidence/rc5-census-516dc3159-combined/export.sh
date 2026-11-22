#!/bin/sh
set -eu
D=$(cd "$(dirname "$0")" && pwd)
C=$(dirname "$D")
W="$C/census-release-516dc3159"
owner=$1
out=$2
shift 2
python3 "$D/exporter/aggregate.py" --plans "$W/plans-v2" --prior "$C/census-1f1be0324" --reference-db "$C/census-c3372ad21/exporter/cache/reference.sqlite" --loss-audit "$C/loss-audit-1f1be0324/losses.json" --run "$W" --owner "$owner" --cache "$D/cache" --out "$out" "$@"
