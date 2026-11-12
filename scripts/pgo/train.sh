#!/usr/bin/env bash
# Step 2 of 3: run the training workload against the instrumented binary.
#
# One `serve` process (port PYRO_PGO_PORT, default 9333) runs the whole workload and is
# stopped with SIGTERM, which the instrumented build turns into a normal exit, so one
# .profraw per process lands in target/pgo/profraw. Workload, in this order:
#   1. bench/playwright-suite, PYRO_PGO_ROUNDS passes (default 3): the 38 Playwright
#      feature tests (locators, forms, navigation, screenshots, PDF, a 2,000-row table).
#   2. bench/scripts/css-bench.mjs (port PYRO_PGO_CSS_PORT, default 9445) through `pyro fetch`:
#      style, relayout, append, rects and qsa loops on a 2,000-element page.
#   3. PYRO_PGO_EXTRA, if set: a script run with PYRO_WS=ws://127.0.0.1:<port> and
#      PYRO_BIN=<instrumented binary> for workloads outside the repository (an application's
#      E2E suite, PDF templates). The profile only helps code the training exercised.
# Needs bun (the suite's `bun install` runs if node_modules is missing).
source "$(dirname "$0")/common.sh"
[[ -x "$PGO_BIN" ]] || { echo "no $PGO_BIN: run scripts/pgo/generate.sh first" >&2; exit 1; }
PORT="${PYRO_PGO_PORT:-9333}"
ROUNDS="${PYRO_PGO_ROUNDS:-3}"
CSS_PORT="${PYRO_PGO_CSS_PORT:-9445}"
mkdir -p "$PROFRAW_DIR"
export LLVM_PROFILE_FILE="$PROFRAW_DIR/train-%m-%p.profraw"

"$PGO_BIN" serve --host 127.0.0.1 --port "$PORT" --log-level error \
    --experimental-features cors --load-resources stylesheet --load-resources iframe &
SERVE=$!
trap 'kill -TERM $SERVE 2>/dev/null; wait $SERVE 2>/dev/null' EXIT
for _ in $(seq 1 200); do
    curl -fs -o /dev/null "http://127.0.0.1:$PORT/json/version" && break
    kill -0 $SERVE 2>/dev/null || { echo "serve exited" >&2; exit 1; }
    sleep 0.1
done

echo "== playwright-suite x$ROUNDS"
(
    cd "$ROOT/bench/playwright-suite"
    [[ -d node_modules ]] || bun install
    for i in $(seq 1 "$ROUNDS"); do
        PYRO_PORT="$PORT" TARGET=pgo OUT="$PGO_DIR/suite-$i" bunx playwright test --reporter=dot
    done
)

echo "== css-bench"
bun "$ROOT/bench/scripts/css-bench.mjs" "$CSS_PORT" &
CSS=$!
sleep 1
for mode in style relayout append rects qsa; do
    PYRO_GEOMETRY=flow "$PGO_BIN" fetch --dump none \
        "http://127.0.0.1:$CSS_PORT/?n=2000&rules=1500&mode=$mode&loops=5" >/dev/null 2>&1 || echo "css-bench $mode failed" >&2
done
kill $CSS 2>/dev/null; wait $CSS 2>/dev/null || true

if [[ -n "${PYRO_PGO_EXTRA:-}" ]]; then
    echo "== extra: $PYRO_PGO_EXTRA"
    PYRO_WS="ws://127.0.0.1:$PORT" PYRO_BIN="$PGO_BIN" "$PYRO_PGO_EXTRA"
fi

kill -TERM $SERVE; wait $SERVE || true
trap - EXIT
echo "profiles: $(ls "$PROFRAW_DIR"/*.profraw | wc -l | tr -d ' ') in $PROFRAW_DIR"
