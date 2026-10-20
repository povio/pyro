#!/bin/bash
# Paired synthetic benchmark of three JsString::new variants (see README.md).
# Usage: run.sh BIN_DIR OUT_DIR   (BIN_DIR/{base,len,exact}/large_string_cycles)
set -euo pipefail
bin=$1; out=$2; mkdir -p "$out/raw"
# All six orders of the three variants, one per round: each variant runs
# first, second and third equally often in every configuration.
orders=("base len exact" "exact len base" "len exact base" "base exact len" "exact base len" "len base exact")
configs=("realm 30 4096 ascii" "realm 30 4096 cjk" "convert 30 4096 ascii" "convert 30 4096 cjk"
         "convert 30 12288 cjk" "realm 30 1024 ascii" "realm 30 1024 cjk" "convert 30 1024 ascii" "convert 30 1024 cjk")
: > "$out/ORDER.tsv"
for c in "${configs[@]}"; do
  tag=${c// /-}
  for r in 1 2 3 4 5 6; do
    pos=0
    for v in ${orders[$((r-1))]}; do
      pos=$((pos+1))
      f="$out/raw/$tag.r$r.$v.txt"
      # shellcheck disable=SC2086
      "$bin/$v/large_string_cycles" $c > "$f" 2>&1
      printf '%s\t%s\t%s\t%s\t%s\n' "$tag" "$r" "$pos" "$v" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/ORDER.tsv"
    done
  done
done
