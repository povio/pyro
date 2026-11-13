#!/usr/bin/env bash
# Step 2 of 3: run the training workload against the instrumented binary and merge its
# profiles into target/pgo/merged.profdata.
#
# Workload: bench/scripts/cdp-perf.mjs runs every scenario once against `serve`, then again
# against `serve --fork`: CDP round trips, Runtime and DOM calls, navigation, input, request
# interception, PDF pagination, Playwright over CDP, the lp.* API and a signed-in app flow, all
# from bench/ fixtures on localhost (no network, no private data). Fork mode runs code that plain
# `serve` does not (the forked page processes), so a profile without it leaves fork mode
# unoptimized.
#
# Every process writes a profile of its own: LLVM_PROFILE_FILE names the server's by pid (%p) and
# a forked process names its own (crates/pyro-cdp/src/pgo_training.rs). When a run's processes are
# gone, its files are merged into a part, skipping any whose process was killed while it wrote
# (cdp-perf SIGKILLs a server that outlives its SIGTERM by 3 s, fork mode a retired template
# after 500 ms); the count is printed. One file shared by all processes (`%m`) would not survive
# that: a single truncated write loses the whole profile.
#
# PYRO_PGO_EXTRA=<script>: one more workload, for an application's own (E2E suite, PDF templates):
# it runs with PYRO_WS=ws://127.0.0.1:<PYRO_PGO_PORT, default 9333> (a plain `serve` of the
# instrumented binary) and PYRO_BIN=<instrumented binary> for processes of its own.
# PYRO_PGO_EXTRA_WEIGHT (default 1) multiplies its counts in the merge: one page load counts for
# little against cdp-perf's loops (the ToS SPA measurement in Cargo.toml weighted it 10).
#
# Needs bun; bench/node_modules (playwright-core) is installed from bench/bun.lock if missing.
source "$(dirname "$0")/common.sh"
[[ -x "$PGO_BIN" ]] || { echo "no $PGO_BIN: run scripts/pgo/generate.sh first" >&2; exit 1; }
[[ -x "$PROFDATA" ]] || { echo "missing $PROFDATA: rustup component add llvm-tools" >&2; exit 1; }
mkdir -p "$PROFRAW_DIR"
rm -f "$PROFRAW_DIR"/*.profraw "$PGO_DIR"/part-*.profdata
export LLVM_PROFILE_FILE="$PROFRAW_DIR/lp-%p.profraw"
[[ -d "$ROOT/bench/node_modules/playwright-core" ]] || (cd "$ROOT/bench" && bun install --frozen-lockfile)

# Processes of the instrumented binary still running (they write their profile as they exit).
running() {
    if [[ "$(uname -s)" == Darwin ]]; then
        ps -axo comm= | grep -cxF "$PGO_BIN" || true
    else
        local n=0 p
        for p in /proc/[0-9]*; do
            [[ "$(readlink "$p/exe" 2>/dev/null)" == "$PGO_BIN" ]] && n=$((n + 1))
        done
        echo "$n"
    fi
}

# Merges the profiles written so far into part-NAME.profdata.
part() {
    local name="$1" files
    for _ in $(seq 1 240); do
        [[ "$(running)" == 0 ]] && break
        sleep 0.5
    done
    files=("$PROFRAW_DIR"/*.profraw)
    [[ -e "${files[0]}" ]] || { echo "$name: no profile written" >&2; exit 1; }
    "$PROFDATA" merge --failure-mode=warn -o "$PGO_DIR/part-$name.profdata" "${files[@]}" \
        2> "$PGO_DIR/merge-$name.log"
    echo "$name: ${#files[@]} profiles merged, $(grep -c 'warning:' "$PGO_DIR/merge-$name.log" || true) skipped (truncated)"
    rm -f "${files[@]}"
}

for mode in plain fork; do
    flags=()
    [[ "$mode" == fork ]] && flags=(--fork)
    echo "== cdp-perf, serve${flags[*]:+ ${flags[*]}}"
    # A failed scenario still trained everything before it; its log says which.
    (cd "$ROOT/bench" && bun scripts/cdp-perf.mjs --bin "train=$PGO_BIN" --playwright --rounds 1 \
        ${flags[@]+"${flags[@]}"} --out "$PGO_DIR/train-$mode.json" > "$PGO_DIR/train-$mode.log" 2>&1) ||
        echo "warning: a cdp-perf scenario failed ($PGO_DIR/train-$mode.log)" >&2
    part "cdp-$mode"
done

inputs=("$PGO_DIR"/part-cdp-plain.profdata "$PGO_DIR"/part-cdp-fork.profdata)
if [[ -n "${PYRO_PGO_EXTRA:-}" ]]; then
    echo "== extra: $PYRO_PGO_EXTRA"
    port="${PYRO_PGO_PORT:-9333}"
    "$PGO_BIN" serve --host 127.0.0.1 --port "$port" --log-level error &
    serve=$!
    trap 'kill -TERM $serve 2>/dev/null || true' EXIT
    for _ in $(seq 1 200); do
        curl -fs -o /dev/null "http://127.0.0.1:$port/json/version" && break
        kill -0 $serve 2>/dev/null || { echo "serve exited" >&2; exit 1; }
        sleep 0.1
    done
    PYRO_WS="ws://127.0.0.1:$port" PYRO_BIN="$PGO_BIN" "$PYRO_PGO_EXTRA"
    kill -TERM $serve
    wait $serve || true
    trap - EXIT
    part extra
    inputs+=(--weighted-input="${PYRO_PGO_EXTRA_WEIGHT:-1},$PGO_DIR/part-extra.profdata")
fi

"$PROFDATA" merge -o "$MERGED" "${inputs[@]}"
"$PROFDATA" show "$MERGED" | sed -n '/Total functions/p;/Total count/p'
echo "profile: $MERGED"
