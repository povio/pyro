# Retrospective AudioParam scheduling

Studio, 2026-11-22 08:58–09:02 CET. Baseline `917ff8f30`, source `212a5503f`. Selected the three failing retrospective ramp/curve pages in census `01f49f43` and the two adjacent set/target controls. No new census. Signed Google Chrome 158.0.8127.37, codesign verified; WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Separate pipe processes for baseline and candidate JS factories and unmodified native. No RC binary tested; Studio had no verified shipped RC.5 executable available.

| WPT | Baseline | Candidate | Native |
|---|---:|---:|---:|
| Set value | 0/1 | 1/1 | 1/1 |
| Set target | 0/1 | 1/1 | 1/1 |
| Linear ramp | 6/9 | 9/9 | 9/9 |
| Exponential ramp | 6/9 | 9/9 | 9/9 |
| Value curve | 6/9 | 9/9 | 9/9 |

11 gains, zero previously passing names lost, five strict page gains. Two passing audit labels embed different numerical diagnostics from native. Process wall times 0.66/0.76/0.66 seconds (base/candidate/native). The earlier cancellation regression group remains 108/108 plus 5/5 PCM controls on this candidate.

Independent controls compared all 896 PCM samples per render to native, tolerance 0.00002. All seven past-time cases failed before and passed after. Current-time, future-time, and negative-time controls remained passing: 21/28 to 28/28 overall. The source validates negative times before clamping and uses the owning context clock for every scheduling and cancellation method. See `qa/results.txt` and the three raw PCM receipts.

Reproduce from candidate checkout: `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$AUDIO_JS" retrospective` with the baseline or candidate factory, or an empty string for native. Baseline returns nonzero after writing results. No build, target, or port reserved. Final sample: 13 GiB available, 84% free memory, 1.77/1.55/1.52 load.
