# AudioBufferSourceNode buffer assignment — Studio, November 22, 2026

The fixed-head signed census (`01f49f43d`) identified a failing `validate .buffer: Validation of AudioBuffer in .buffer attribute setter`. The matched `7558420fc` baseline accepted a second non-null buffer after the first had been cleared. The final source `4b8452245` tracks whether any non-null buffer has been assigned. Clearing a started source also releases its acquired playback data, so the old signal does not play after clearing.

| Ordinary WPT (named cases) | Native | Baseline | Candidate |
| --- | ---: | ---: | ---: |
| audiobuffersource-channels.html | 1/1 | 0/1 | 1/1 |
| audiobuffersource-null.html | 1/1 | 1/1 | 1/1 |
| Wall seconds for both pages | 0.77 | 0.54 | 0.61 |

One named gain; no named losses. Chrome `158.0.8127.37`, executable signature verified, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The native pages used Chrome Web Audio; both Pyro versions used the same Chrome harness with the JS module installed and identity checked. Reproduce from the source checkout: `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT" "$MODULE" buffer-assignment` (empty module argument for native). No release binary result is claimed.

Independent controls (`independent/probe.cjs`) used 14 operation sequences, 384 frames at 48 kHz, and two opposite-sign buffers. Setter/getter and exception histories matched native in 10/14 baseline, 14/14 intermediate, and 14/14 final. Exact full-array PCM matched 9/14, 11/14 and 14/14 respectively. Independent QA caught that the intermediate still played the original signal after clearing. On final source, clearing during suspension leaves frames 0–127 unchanged and silences 128–383; a rejected reassignment without clearing preserves the original signal. The lane reran only the final candidate using the independently authored probe and compared all histories and PCM to the saved native data (0.62 s). `intermediate.json` is diagnostic, not a final result.

Batch started 15:22 CET. Studio sample: 99.39% idle, load averages 1.11/1.33/1.33, 12 GiB free. No Cargo, ports, new targets, cleanup or changes to the playback-stats topic. Null-start ended-event timing and realtime hardware playback were not validated here.
