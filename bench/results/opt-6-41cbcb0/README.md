# opt-6: browser threads own their connection sockets (option C)

Commits ca5124f..41cbcb0 on top of b8d8e01. Host: Apple M1 Max, macOS 27.0.1, busy (load 16-22).

- `compare.md`: `bench/scripts/cdp-perf-ab.sh b8d8e01 41cbcb0 -- --rounds 5` (all 31 scenarios, Playwright off).
  0 regressions, 3 improvements, 10 unclear, 18 unchanged. Browser CPU per command drops 30-52% on small
  commands (getVersion 0.027 -> 0.013 ms, evaluate 0.028 -> 0.018 ms, callFunctionOn 0.029 -> 0.016 ms);
  wall time -16..-43% (several p = 0.056, short of the 5-round significance bar on this noisy host).
  Pipelined throughput (256 in flight) is +8% wall (below the 10% threshold) with -20% CPU.
- `stages.md`: perf-trace build, 3 rounds. The cross-thread stages are gone (`io.to_browser` was 8-10 us,
  `io.outbound_queue` ~5 us, `outbox.push` 1.6 us); per command the socket read is ~4 us and the write ~4 us,
  both on the browser thread.
- `option-a-alone-compare.md`: the wake-pipe change (ca5124f) measured alone against b8d8e01: no gain.
- Idle CPU (connected client, page open, no commands, 5 s): 0.01% (b8d8e01) vs 0.00%. Page with a perpetual
  requestAnimationFrame loop: 0.87% vs 1.04% of one core.
- Full check with `--playwright --quick`: all 40 scenarios pass.
