# Studio: AudioParam nominal ranges

Parent `ca5530344f977baeab6f705f1b5f98b543b70aea`; candidate `e07240ea0ff7ff9b6568b01d66a9ebd136cb7eb9`. Fixed-head census `01f49f43` identified a 96-pass native advantage on the ordinary nominal-range page. On this matched parent it was 93 (327 versus 234). This lane addresses existing AudioParams, not absent factories. WPT checkout `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; only the two explicit allowlists ran. No crash or withheld fixtures.

| Engine | Strict pages (primary) | Passing/emitted | Runner wall (ms) | Additional signal WPTs |
|---|---:|---:|---:|---:|
| Parent, twice | 6/8 | 445/482 | 360, 217 | 3/3, 21/21, 104 ms |
| Candidate, twice | 6/8 | 475/482 | 251, 212 | 3/3, 21/21, 102 ms |
| Signed Chrome 158.0.8127.37 | 8/8 | 544/544 | 711 | 3/3, 21/21, 280 ms |

The nominal page improved from 234 to 264 of 270 emitted subtests. Exact (URL, name) comparison: all 445 parent passes retained, 30 failing assertion names replaced by 30 passing names (all 30 match native passes), seven failing names unchanged. Both repeat status maps were identical. Six factory tasks on the nominal page and the oscillator constructor-options task still fail. No strict-page gain and no corpus-wide or speedup claim.

`tests/e2e/audio-param-range.mjs` compares direct assignments, constructor options, scheduled values and a linear ramp against a unit DC source and negative unit modulation. Parent pan=2 played hard right instead of center; late ramp left sample at frame 96 was 0.38268343 rather than 0.70710677. Candidate and Chrome match across all returned values, including last rendered intrinsic value during suspension, delay/oscillator bounds and biquad gain limit. The renderer still does not provide realtime audio-device output.

Studio used one Cargo build at a time, `CARGO_BUILD_JOBS=2 CARGO_PROFILE_WPTITER_INCREMENTAL=false`, reusing an isolated APFS clone of the warm target. `pyro-build` took 218 s parent, 70 s candidate. Binary SHA-256: parent `de4c216cedad468cb85f14c6d8571f255c5f21ad36f5fa2929f3d3561cc04005`, candidate `d09e99a99e516dcd9643b38ca41129cb81a21705c908e817a23e6c4ad1042622`. Approximately 30-second host samples in `load.log`: CPU 81.12–94.85% idle; memory_pressure 82% free. Disk reached 14 GiB free; 15 GiB available after removing the lane's isolated target/profile. `codesign --verify` succeeded on Google Chrome.app (Google LLC, EQHXZ8M8AV).

To reproduce, install the pinned bench dependencies and run `bun bench/wpt/serve.mjs "$WPT_DIR" 24780`. For each JSON allowlist run `bun bench/wpt/run.mjs --base http://127.0.0.1:24780 --wpt "$WPT_DIR" --dirs webaudio --tests "$LIST" --jobs 1 --bin label="$BIN" --out result.json`. Native launcher uses signed Chrome headless with an isolated profile, `--autoplay-policy=no-user-gesture-required`, and the CDP port supplied by the runner. Run the direct probe against each engine's CDP endpoint with `bun tests/e2e/audio-param-range.mjs http://127.0.0.1:PORT`; the parent prints JSON before its expected assertion failure.
