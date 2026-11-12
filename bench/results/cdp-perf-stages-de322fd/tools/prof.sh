#!/bin/bash
# usage: prof.sh WORKLOAD [SECONDS]
set -u
S=${OUT:-/tmp/pyro-prof}; T=$(dirname "$0"); W=$1; SEC=${2:-8}
BIN=${ROOT:?set ROOT to the repo}/target/release/pyro
SAMP=${ROOT:?set ROOT to the repo}/bench/rss-sampler/target/release/rss-sampler
PORT=$((9500 + RANDOM % 300)); SPORT=$((9900 + RANDOM % 90))
mkdir -p $S/prof
node ${ROOT:?set ROOT to the repo}/bench/scripts/cdp-perf-site.mjs $SPORT >/dev/null & SITE=$!
LIGHTPANDA_DISABLE_TELEMETRY=true $BIN serve --host 127.0.0.1 --port $PORT --log-level error --experimental-features cors --load-resources stylesheet >/dev/null 2>&1 & Pyro=$!
sleep 0.7
bun $T/load.mjs ws://127.0.0.1:$PORT/ http://127.0.0.1:$SPORT $W $((SEC + 2)) > $S/prof/$W.load &
L=$!
until grep -q READY $S/prof/$W.load 2>/dev/null; do sleep 0.1; done
c0=$($SAMP $Pyro - --once | awk '{print $4+$5}')
if [ "${PROF:-sample}" = xctrace ]; then rm -rf $S/prof/$W.trace; xctrace record --template "Time Profiler" --attach $Pyro --time-limit ${SEC}s --output $S/prof/$W.trace >$S/prof/$W.xctrace.log 2>&1; else sample $Pyro $SEC 1 -mayDie -file $S/prof/$W.sample >/dev/null 2>&1; fi
c1=$($SAMP $Pyro - --once | awk '{print $4+$5}')
wait $L
kill $Pyro $SITE 2>/dev/null
echo "$W $(grep OPS $S/prof/$W.load) browserCPUms_in_window=$(( (c1 - c0) / 1000000 ))"
