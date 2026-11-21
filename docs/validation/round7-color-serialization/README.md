# Round 7: missing polar components in computed CSS colors

Compared with integrated batch 11 (`1ee620d451c093a2ddce8766172970ed0ce8275b`), source `492fe14cf4c81aeff5c9d2c29062019af1b3dbcf` gains 132 named passes and loses 16. Of the gains, 16 also pass pinned Chrome and 116 do not. All 16 losses previously passed only LP. **No strict page gained or lost.** None of the four declared-value controls changed.

| Focus | Named before / after / Chrome | Gains also Chrome / LP only | Losses also Chrome / LP only | Remaining Chrome only / LP only |
|---|---:|---:|---:|---:|
| computed-powerless | 423 / 447 / 310 | 0 / 40 | 0 / 16 | 49 / 186 |
| computed-none | 594 / 677 / 420 | 7 / 76 | 0 / 0 | 1 / 258 |
| computed-color-mix-function | 964 / 973 / 945 | 9 / 0 | 0 / 0 | 3 / 31 |
| All 12 URLs | 9147 / 9263 / 8826 | 16 / 116 | 0 / 16 | 56 / 493 |

All runs observed exactly 9,405 distinct (URL, name) pairs, 12 complete OK harnesses, zero restarts or watchdog kills; strict (complete OK, every assertion passes) 7 / 7 / 6. Focused runner wall times before / after / Chrome: **464 / 271 / 1400 ms**; browser CPU 0.70 / 0.54 / 1.86 s. Single samples, not a performance comparison. Raw receipts include per-page times and failure messages. Run `python3 compare.py` in this directory to regenerate the uncompressed `differences.json.gz` content, including every named gain, loss, remaining mismatch, and control count.

## Interpretation

CSS Color 5, “Serializing color-mix()”: a missing component prevents conversion of a polar HSL/HWB mix result to sRGB. Previously the conversion collapsed the distinction between a missing hue and a numeric hue. CSS Color 4, “HSL Colors”: only negative absolute saturation is floored; extended HSL channels must survive reparsing. HSL ↔ HWB via sRGB avoids XYZ round-off on a mathematically zero channel. The native guard exercises the missing/non-missing distinction and the extended-coordinate round trip. No WPT expectation or existing native assertion changed.

`computed-none` closes seven of eight batch-11 Chrome-only assertions. The remaining one mixes `lab(none 10 10)` with an HSL operand. `computed-powerless` closes **none** of its 49 Chrome-only cases. Its 16 losses are single-operand mixes with authored HSL/HWB coordinates. Inline CSSOM serializes these operands as byte RGB before computed-style reparsing, which can erase an explicit powerless hue. Retaining missing result channels now makes that distinction visible. This is an unresolved semantic limitation, not grounds to rewrite the expectations. An unshipped experiment kept polar mix operands as authored; it regressed 123 named declared-mix assertions. Changing the public specified serialization is not a suitable fix for the loss of internal operand precision.

## Reproduction and pins

- Positive allowlist `urls.json` only; all 12 are members of the frozen safe main plan. Exact baseline and Chrome URL status, harness status, ordered names and outcomes matched their archived entries at `0d8a4e1097a3459530259d3e10214dd87238dfdf` (`docs/validation/studio-css-round7/{batch11/outcomes-main,frozen/outcomes-chrome-main}.json.gz`). No withheld URLs or full runs.
- WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; frozen `bench/wpt/run.mjs --kind testharness --wpt "$WPT_DIR" --dirs css --server wptserve --testdriver --jobs 2 --tests urls.json --bin "$label=$binary" --out "$label.json"`. Sequential runs, wptserve ports before 62018, after 62526, Chrome 63601.
- LP binaries SHA-256 before `9cc924676e94442da81d71354e1402e4dca3d78fadd9ab31145fec56cccab980`, after `5b26e6dace1135966958179e83636e2f9c39d01c646e0e19178cb50193f438ed`. `pyro-build`, `CARGO_BUILD_JOBS=3`, dedicated `$LP_CACHE/studio-r7/color-serialization/target`, profile `wptiter`, final build 72 s. Patched JSC `0e0dd7038a7a-6f4649e395e0`, static library SHA-256 `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`. Chrome stable `158.0.8127.37` (installed bundle version), not Canary.
- Native guard: `cargo test --locked --profile wptiter -p pyro-webapi --test integration missing_polar_color_components_round_trip -- --test-threads=1`, same target/jobs/profile. One passed, 0.06 s test, 5.39 s wall. `scripts/vendor/regen-patch stylo --base 1ee620d4 --check` passed forward/reverse. The generated unified patch contains mandatory space-prefixed blank context lines; source whitespace checked separately.
