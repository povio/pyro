# Buffer resampling and stitching (Studio, 2026-11-22)

The `01f49f43` census identified deficits of one in buffer-resampling and two in sub-sample-buffer-stitching. All three reproduced on integrated `89f8cf807`. The larger Panner constructor group depends on HRTF rendering, which is not implemented; it was inspected, not executed or counted here.

| Batch | Baseline | Candidate | Native | Wall seconds (same order) |
|---|---:|---:|---:|---|
| Resampling and stitching | 0/3 | 3/3 | 3/3 | 0.98 / 1.41 / 1.14 |
| Reverse loop and oscillator | 40/40 | 40/40 | 40/40 | 0.52 / 0.54 / 0.52 |
| Grain and scheduling | 58/58 | 58/58 | 58/58 | 0.74 / 0.75 / 0.75 |

No threshold changes. The last fractional interval now continues the final slope (constant for a one-sample buffer). An exact scheduled start rounded to a render frame no longer acquires a tiny negative initial offset. No extra tail frame is rendered. Named gains: `Interpolation of AudioBuffers to context sample rate`, `buffer-stitching-1`, `buffer-stitching-2`; zero regressions in this sample. Elapsed times are single observations, not a benchmark.

Source `7e51e0be8`; candidate JS SHA-256 `348c96efc5355df4bf26fb6ff11a92b65617c58484a721d15f60e4ea22c652a6`. Signed Chrome 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The source topic's `tests/e2e/audio-grain-duration.mjs` takes Chrome path, WPT root, module path (empty for native), and batch (`resampling`, `reverse-loop`, or `grain`). Baseline expectedly exits nonzero after saving outcomes. This compares injected JS in Chrome with native Chrome; it is not release-binary evidence.

Independent stereo ramp/flat probe rerun on the final hash: baseline 92/96 exact samples, candidate 96/96. At 24→48 kHz a one-sample buffer holds its value for both output frames; a four-sample ramp advances from 0.5 to 0.5625 before silence. The two 48→48 kHz controls are unchanged. Exact PCM and per-render times are in `independent/receipt.txt`. To replay, export the baseline and candidate JS as `baseline.js` and `candidate.js` in the working directory before running the independent probe.

Host samples 92.57% and 94.40% idle; 13 GiB free before and after. No ports, Cargo jobs, cleanup, backend changes or compressor changes. Its reduction meter remains an acknowledged numerical residual.
