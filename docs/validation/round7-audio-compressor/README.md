# DynamicsCompressorNode: matched source and signal measurements

Studio, November 22, 2026, 14:25–14:32 CET. Integrated base `95c73c5c1`, separate candidate `c28cc0e46`. The saved `01f49f43` census had deficits of 9, 4 and 5 passing outcomes for the basic, constructor and connected k-rate compressor pages. Unlike the already integrated waveshaper, analyser, AudioParam and render-quantum improvements, this DSP node was still absent on the current base.

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Google-signed Chrome `158.0.8127.37`. The same harness used an empty module path for native and the two JS source factories for before/after, removing the native compressor constructor before injection and verifying the installed context and constructor identity. The Rust lazy-global registration was edited but not compiled; these measurements are not packaged or release-binary results.

| Named outcomes | Baseline | Candidate | Native |
|---|---:|---:|---:|
| Basic defaults and reduction | 4/5 | 13/13 | 13/13 |
| Constructor and options | 0/4 | 4/4 | 4/4 |
| Five connected k-rate parameters versus automation | 0/5 | 5/5 | 5/5 |
| Elapsed including startup | 0.55 s | 0.93 s | 1.15 s |

Three newly passing pages; passing named outcomes increased by 18 with no passing-to-failing outcomes. The basic audit emitted eight additional outcomes once its factory worked. Rerun from the source checkout: `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$MODULE" compressor`. A failing baseline emits JSON before exiting nonzero.

Independent measurements (`qa/`): ordinary stereo impulses and a sustained signal with a quiet probe on the opposite channel, then a release segment. At 44.1/48 kHz the candidate matched native delay exactly (264/288 frames). At both rates both channel permutations passed the independent 5% gain tolerance. At 48 kHz, candidate/native quiet gain was 1.52442/1.52412 and steady loud gain 0.86791/0.86797. At 150 ms into release candidate/native probe gain was 1.52442/1.48891. These are windowed ratios, not sample-exact envelope parity. **Remaining difference:** near the end of the loud segment at 48 kHz the candidate reduction meter was −4.89 dB versus native −7.41 dB. The qualitative recovery was present, but the meter is not numerically matched. The source digest is recorded in the QA filenames. Run the two QA scripts from a disposable directory with fresh output names; the controls script writes native.json before running the candidate comparison.

Candidate regression batches: cancellation/hold 113/113 (0.78 s), retrospective automation 29/29 (0.68 s), offline quanta 27/27 (0.55 s). No Cargo, new target, listening ports, WebCodecs edits or shared-artifact cleanup. Free disk 13 GiB; load averages 1.39/1.47/1.36 at start and 0.98/1.35/1.34 near finish, with 92% CPU idle at the final sample.
