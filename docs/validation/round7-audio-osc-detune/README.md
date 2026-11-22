# Oscillator detune limiting — Studio, 2026-11-22

Saved census `01f49f43` identified two failed detune-limiting checks. Both reproduced at integrated baseline `7509a250e`. Final source topic `81ded3b45` (includes `12000da7c`) silences the oscillator if its fundamental is at or beyond either Nyquist limit and advances its phase using the bounded frequency. No change to Panner, compressor, or backend registration.

| Batch | Baseline | Final | Signed native | Wall seconds, same order |
|---|---:|---:|---:|---|
| Oscillator | 33/43 | 43/43 | 43/43 | 0.56 / 0.73 / 0.61 |
| PeriodicWave and constructor controls | 95/95 | 95/95 | 95/95 | 0.55 / 0.54 / 0.65 |
| Buffer resampling and stitching | 3/3 | 3/3 | 3/3 | 1.40 / 1.41 / 1.02 |

Of the ten gains, two are WPT outcomes (large fixed detune; automated detune crossing Nyquist) and eight are focused audible/silent/audible controls covering both frequency signs for each built-in waveform. Basic oscillator remained 33/33. No lost named passes. Single run wall times are not performance benchmarks.

Independent phase probe: final source matches all 20 observed silent frames; both signs recover the native phase within the same render quantum to six decimal places. **Residual:** if the five-frame excursion starts at frame 125 and crosses the 128-frame boundary, the restored sine has opposite phase to Chrome for both signs. Passing silence tests does not imply complete phase or band-limited waveform parity. See `independent/independent.txt` (12 samples per case starting two frames before the excursion). Re-run the independent probe with `node probe.mjs base.js candidate.js` from a temp directory containing those module snapshots.

Matched runner: `tests/e2e/audio-grain-duration.mjs` at the source topic, arguments Chrome executable, WPT root, JS source file (empty for native), and batch `osc-detune`, `periodic` or `resampling`. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Google Chrome 158.0.8127.37 with verified executable signature. Candidate module SHA-256 `61baa8410dc59004073c21da03667ce021596276ab7bed6af4a8c7c663a15d20`. Source injection, not release-binary validation. Baseline osc-detune exits nonzero as expected after writing results.

Batch started 14:49 CET. Sampled 91.37% and 94.2% CPU idle and 13 GiB free; no Cargo jobs, listening ports, target allocation or cleanup. No Panner or compressor gain claimed; the existing compressor meter residual remains unchanged.
