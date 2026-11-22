# Realtime AudioContext render quantum

Studio, 2026-11-22 08:51–08:56 CET. Existing census `01f49f43` identified the ordinary realtime constructor page; no new census. Integrated baseline `9c91af023`, source `6b28b786c`. Signed Chrome 158.0.8127.37 (codesign verified), WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Separate Chrome processes over a pipe, guarded factory installation or native without injection. No actual RC binary or physical output device tested. Offline custom quanta remain out of scope.

| Measurement | Baseline | Candidate | Native |
|---|---:|---:|---:|
| audiocontext-rendersizehint.html | 0/18 | 18/18 | 18/18 |
| Scheduled oscillator end and clock alignment | 0/3 | 3/3 | 3/3 |
| Elapsed seconds, combined | 0.78 | 0.74 | 0.80 |

All 18 named WPT outcomes gained, zero lost; one new strict page. Oscillator controls allow end notification in the quantum containing the stop time. A second native run also passed all 21. Independent QA matched 37/37 native API observations (baseline 0/37), and passed 8/8 deterministic clock, suspend/resume and source-end controls (baseline 2/8). Raw independent controls and outputs in `qa/`.

Reproduce from the source checkout: `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$AUDIO_JS" realtime-quantum`, passing the baseline or candidate factory path, or an empty string for native. The baseline command exits nonzero after writing its results. No build, target, ports, or disk cleanup. Final host load 2.18/1.74/1.60, 83% memory free, 13 GiB disk available.
