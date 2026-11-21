# Round 7: lossless color conversion split

Integration candidate `ddc83e77435044726e315df883270bf368ebc8dc` is based directly on batch 11 (`1ee620d451c093a2ddce8766172970ed0ce8275b`). The earlier polar mix-finalization experiment `492fe14cf4c81aeff5c9d2c29062019af1b3dbcf` is on a separate branch, not an ancestor.

| Split vs. | Gains also passing Chrome | Gains LP only | Losses also passing Chrome | Losses LP only |
|---|---:|---:|---:|---:|
| Batch 11 | 7 | 6 | 0 | 0 |
| Previous candidate | 0 | 16 | 9 | 110 |

The split restores **all 16** named `computed-powerless` passes lost by the previous candidate. It does so by leaving mix finalization unchanged, **not** by solving internal operand storage. It gives up 119 gains from the previous candidate. Against batch 11, all 13 gains are in `computed-none` (594 → 607 / 678); one Chrome-only assertion remains there. `computed-powerless` exactly matches batch 11 (423 / 570; 49 Chrome-only). Every named outcome on the other ten URLs, including computed and declared mix controls, is unchanged versus batch 11.

| Run | Named passes / 9405 | Strict pages / 12 | Complete OK | Wall ms | Browser CPU s | wptserve port |
|---|---:|---:|---:|---:|---:|---:|
| Batch 11 | 9147 | 7 | 12 | 374 | 0.62 | 55378 |
| Previous candidate | 9263 | 7 | 12 | 296 | 0.59 | 55625 |
| Split | 9160 | 7 | 12 | 283 | 0.54 | 56227 |
| Chrome stable | 8826 | 6 | 12 | 1085 | 1.64 | 55880 |

## Why split

Absolute HSL floors negative saturation; lightness and saturation above the reference range are not clipped before serialization. HSL and HWB share the sRGB primaries: converting through sRGB directly avoids introducing XYZ round-off. Both fixes are independent of how mix results or operands are stored.

The scoped alternative is preferable to rewriting the public serialization of a mix operand. `StyleData::set` produces declaration text, and `style_attribute()` subsequently parses the resulting attribute for Stylo. Rule edits can also write stylesheet text. Retaining semantic operands while exposing canonical CSSOM would require coordinating parsed state with inline mutation, attribute replacement, and stylesheet rebuilding. The earlier experiment that changed the specified representation instead lost 123 declared-mix assertions. No parser shortcuts or expectation changes are included here.

## Reproduction

`urls.json` is the original 12-URL positive allowlist, verified against the frozen safe main. Run each binary sequentially with the frozen runner: `node bench/wpt/run.mjs --kind testharness --wpt "$WPT_DIR" --dirs css --server wptserve --testdriver --jobs 2 --tests urls.json --bin "$label=$binary" --out "$label.json"`. All four runs were executed afresh with WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome `158.0.8127.37`, and unchanged runner/settings. Batch 11 and Chrome reproduced the archived ordered names, outcomes and harness statuses in `0d8a4e10`; the previous binary reproduced the receipt at `14f9dc0c`. No full, withheld, crash, or HDR suite was run. Single samples are not performance measurements.

LP executable SHA-256: batch 11 `9cc924676e94442da81d71354e1402e4dca3d78fadd9ab31145fec56cccab980`; previous `5b26e6dace1135966958179e83636e2f9c39d01c646e0e19178cb50193f438ed`; split `c519ef5b3d811bcd6e60c440b627dac0d739412a5f0d95b33883c40ea2f8c44c`. The previous executable was cloned before rebuilding. `pyro-build`, exclusive color lane target, `CARGO_BUILD_JOBS=3`, 38 s build. Both use patched JSC `0e0dd7038a7a-6f4649e395e0`, static library SHA-256 `cdd57b33b09f8b2e7d7648d63dc88fbd1f1d444ecd08eea91fe67db192773c0b`.

Native: `cargo test --locked --profile wptiter -p pyro-webapi --test integration missing_polar_colors_across_declaration_paths -- --test-threads=1`, same target/jobs/profile. Checks `setProperty`, direct `style` attribute replacement and stylesheet rules, plus the specified mix getter. Passed (0.08 s test; 9.17 s wall). Vendor patch regeneration and forward/reverse verification passed.

Run `python3 compare.py` in this directory to reproduce the exact uncompressed `differences.json.gz`, including recovered names, all directional gains/losses against both LP baselines, and final LP-only/Chrome-only names. Raw compressed receipts include failure messages and per-URL timings.
