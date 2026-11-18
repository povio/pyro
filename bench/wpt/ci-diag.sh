#!/bin/bash
# Machine diagnostics around a CI shard on the self-hosted Macs (lp-m1): power mode, thermal state, memory pressure,
# swap and load, as a snapshot before and after plus a sample every 5 s in between, so a slow shard can be told
# apart from a throttled (fanless worker-2-air), swapping (8 GB) or busy machine.
#
#   bench/wpt/ci-diag.sh start   snapshot, then sample in the background ($RUNNER_TEMP/ci-diag.log)
#   bench/wpt/ci-diag.sh stop    stop sampling, snapshot, and a one-line summary of the samples
set -u
dir="${RUNNER_TEMP:-/tmp}"
log="$dir/ci-diag.log"
pidf="$dir/ci-diag.pid"

snapshot() {
  echo "== $1 $(date -u +%H:%M:%S) $(hostname -s), $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) CPUs, $(( $(sysctl -n hw.memsize) / 1073741824 )) GB"
  echo "load: $(sysctl -n vm.loadavg)"
  echo "power: $(pmset -g 2>/dev/null | grep -iE 'lowpowermode|powermode' | tr -s ' ' | tr '\n' ';')"
  echo "thermal: $(pmset -g therm 2>/dev/null | grep -v '^$' | tr '\n' ';')"
  echo "swap: $(sysctl -n vm.swapusage)"
  echo "memory: $(memory_pressure -Q 2>/dev/null | tail -1)"
  vm_stat | grep -E 'Pages (free|active|inactive|wired down|occupied by compressor)|Swapins|Swapouts|Pageouts'
}

sample() {
  while :; do
    # time, 1-min load, swap used (MB), free memory %, CPU speed limit (100 = not throttled; absent = never recorded)
    swap=$(sysctl -n vm.swapusage | sed -E 's/.*used = ([0-9.]+)M.*/\1/')
    free=$(memory_pressure -Q 2>/dev/null | tail -1 | grep -oE '[0-9]+%' | tr -d %)
    limit=$(pmset -g therm 2>/dev/null | grep -oE 'CPU_Speed_Limit[^0-9]*[0-9]+' | grep -oE '[0-9]+$')
    echo "$(date -u +%H:%M:%S) $(sysctl -n vm.loadavg | awk '{print $2}') ${swap:-0} ${free:-?} ${limit:-100}" >> "$log"
    sleep 5
  done
}

case "${1:-}" in
  start)
    snapshot before
    : > "$log"
    sample & echo $! > "$pidf"
    ;;
  stop)
    [ -f "$pidf" ] && kill "$(cat "$pidf")" 2>/dev/null
    snapshot after
    awk 'NR==1{mins=$3;maxs=$3;minf=$4;minl=$5} {l+=$2; n++; if($3>maxs)maxs=$3; if($4<minf)minf=$4; if($5<minl)minl=$5; if($2>maxl)maxl=$2}
      END { if (n) printf "samples %d: load mean %.1f max %.1f; swap used max %s MB; free memory min %s%%; CPU speed limit min %s%%\n", n, l/n, maxl, maxs, minf, minl }' "$log"
    ;;
  *) echo "usage: $0 start|stop" >&2; exit 2 ;;
esac
