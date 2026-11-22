# OfflineAudioContext indefinite rendering and explicit close

Studio, November 22 2026, 10:47–10:52 CET. Integrated baseline `09b20608e`, topic `42d454176`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, signed Chrome `158.0.8127.37`. Baseline and candidate use the actual source factory installed in a fresh Chrome page with constructor identity checks. Native uses the same executable without injection. No release or executable-level Pyro claim.

Run `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$MODULE" offline-close` from the topic checkout. Set MODULE to the baseline or candidate JS, or the empty string for native. Nonzero status is expected when a named assertion fails; the full named results are emitted first.

| WPT | Baseline | Candidate | Native |
| --- | ---: | ---: | ---: |
| ctor-offlineaudiocontext.html | 34/37 | 45/45 | 45/45 |
| offlineaudiocontext-close.html | 0/5 | 4/5 | 5/5 |
| Elapsed including startup | 0.533 s | 0.567 s | 0.612 s |

One newly passing page, no observed losses. The constructor's audit helper emits additional assertions after the omitted-length constructor succeeds, so the increase in raw passing outcomes is not a count of independent fixes. Close gains: before render, double close, pending render abort, and closing while suspended with outstanding requests. The remaining failure expects a 300-frame request to produce 512 frames under a custom 256-frame quantum; the current offline engine uses 128-frame quanta and returns 384. This is not claimed fixed.

Independent signed Chrome comparison (`qa/`): two serial runs, bounded stereo indexed samples, FIFO lengths 128/128/256 with zero mismatches, pause/resume, natural completion, explicit close of fresh and suspended finite contexts, and immediate vs fulfilled close state. Same exception type for positional null length; diagnostic wording differs. Run `node probe.mjs /absolute/path/to/webaudio.js` from a disposable copy of `qa/` to replay. The independent probe explicitly enables Chrome's incremental rendering flag; the main native WPT reference uses no such flag.

Finite regressions: 8/8 focused controls and 2/2 WPT (0.549 s); suspension harness 21/21 with five PCM controls (0.600 s). No Cargo, allocated ports, deletion, or security/crash fixtures. Disk remained 12 GiB available; load averages 2.43/3.06/2.27 at start and 1.64/2.08/2.03 near finish; memory_pressure reported 84% free.
