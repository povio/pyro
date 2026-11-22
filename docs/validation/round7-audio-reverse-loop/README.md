# Reverse playback: loop entry and end-of-buffer offsets

Studio, 2026-11-22, starting 14:37 CET. Fixed-head census `01f49f43` flagged negative playback and basic oscillator output. The latter already matches the signed reference (33/33); no oscillator gain is claimed. The negative-playback WPT page includes a survival-only case and was not executed. Seven independently runnable bounded PCM scenarios cover this change instead.

| Check | Baseline | Candidate | Native |
|---|---:|---:|---:|
| Reverse/forward PCM | 2/7 | 7/7 | 7/7 |
| Basic oscillator WPT | 33/33 | 33/33 | 33/33 |
| Grain/scheduling controls | 58/58 | 58/58 | 58/58 |
| Focused wall time | 0.52 s | 0.52 s | 0.59 s |
| Grain wall time | 0.76 s | 0.76 s | 0.81 s |

Five new exact PCM matches: reverse lead-in from above the loop, reverse start below the loop, start at buffer duration, start past duration, and fractional lead-in through the interpolation seam. No losses. The fractional seam expectation uses the observed native midpoint of the last and first loop samples; it is not a linear interpolation into the post-loop tail. The baseline exits nonzero as expected after writing all its outcomes.

The driver is `tests/e2e/audio-grain-duration.mjs` at source `b8ec59d26`. It ran signed Chrome 158.0.8127.37 (codesign verification succeeded) with WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Arguments: Chrome executable, WPT root, module file (empty for native), and `reverse-loop` (omit batch for the grain checks). Baseline injected the exact `crates/pyro-webapi/src/webaudio.js` from `aa1a137c9`; candidate injected that file from the source topic, SHA-256 `8906ebab6f5a2ca68db94da0dc0d7a1698f259446ad9f6dc768c05087fced93d`. Constructor identity is checked. No Rust/backend changes, builds, release binary tests, or release claims. No change to the compressor or its documented meter residual.

Host samples: 92.19% idle before, 94.33% idle after primary validation; 13 GiB available at both samples. No target, Cargo slots or ports used, no cleanup.

Independent stereo validation (`independent/`): signed native and candidate matched on all 1,920 samples across six 160-frame cases (including a render quantum boundary). Baseline differed on 640 samples, all in reverse starts above or below the loop. All forward controls remained exact. Native/baseline/candidate took 261/280/340 ms. Executable signature verification passed; the separate strict app-bundle check reported Finder metadata in the framework (see signature receipt).
