# Custom offline Web Audio render quanta

Studio, November 22, 2026, 10:57–11:02 CET. Baseline `62ca2d29e`, candidate `4b6249f2f`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Google-signed Chrome `158.0.8127.37`. Native used Chrome without injection; the two source runs installed the respective JS factory with constructor identity checks. These are not packaged Pyro or RC measurements.

| Named outcomes | Baseline | Candidate | Native |
| --- | ---: | ---: | ---: |
| Offline render-size constructor and limits | 3/17 | 17/17 | 17/17 |
| Suspend at a 441-frame boundary | 0/1 | 1/1 | 1/1 |
| Offline close, including a chunk rounded to 512 frames | 4/5 | 5/5 | 5/5 |
| Stereo PCM and analyser history, sizes 441 and 33001 | 2/4 | 4/4 | 4/4 |
| Wall seconds including launch | 0.57 | 0.54 | 0.61 |

Three WPT pages gained, 16 named WPT assertions gained, none lost. In the additional signal tests stereo passthrough already passed; both analyser-history checks gained. The large quantum exceeds the analyser history capacity. Run `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$MODULE" offline-quantum` from the topic worktree; use an empty module path for native. Raw JSON is emitted before assertions, so a nonzero baseline exit is expected.

Independent probe: `node qa/compare.mjs [modulePath]`. Five simultaneous bounded contexts exercised k-rate automation, odd quanta, final partial blocks and cross-block delay. Four controls gained (117/4/117/4 mismatched samples became zero); the 128-frame delay control stayed green. Final raw PCM and constructor identities are in `qa/`. Candidate JS SHA-256 `d6449057cdff6b4ad8e8fb5e63b03a567aff9a2198667460fc74acbb824058c1`.

Default-quantum regressions: finite chunk controls and two WPT pages passed (0.59 s); suspension/disconnect and five boundary controls passed (0.56 s). Independent Rust DSP validation: five tests passed via standalone rustc using warm dependencies, including irregular FFT partition boundaries and stereo tail timing (compile 1.263 s; tests 0.361 s). The JS-to-Rust binding was not built or exercised end to end.

No Cargo build, allocated ports, shared-artifact cleanup or RC smoke. Load averages 1.46/1.84/1.94 before the batch and 1.53/1.59/1.74 at finish; final CPU sample 93.27% idle. Disk reported 12 GiB available at both endpoints; memory_pressure reported 84% free during validation.
