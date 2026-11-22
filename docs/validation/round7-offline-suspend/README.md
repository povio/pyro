# Offline audio suspension boundaries

Fixed-head census `01f49f43`: three signal transitions in `audionode-disconnect-audioparam.html` arrived at sample 11008 rather than signed Chrome's 11136. Matched base `414fc7481` reproduces this. Candidate `53ff52cc8` corrects the scheduler rather than changing graph disconnect. Web Audio's OfflineAudioContext.suspend definition rounds the requested time **up** to a render quantum. Comparing times at the boundary avoids advancing a whole extra quantum when multiplying a boundary time back by its rate introduces roundoff.

| Measurement | Base module | Candidate module | Signed Chrome 158.0.8127.37 |
|---|---:|---:|---:|
| Focused WPT strict pages | 0/1 | 1/1 | 1/1 |
| Passing/emitted | 15/21 | 21/21 | 21/21 |
| Wall including browser launch | 0.63 s | 0.54 s | 0.62 s |

All 15 previously passing names stayed passing. The six gains are three signal assertions and their three summaries. Candidate/native differ on two already-passing exception-message names, not on signal assertions. Independent constant-source controls observed the first silent sample and the reported suspension time. At requested frames 0, 127, 128, 129 the candidate and native both yielded 0, 128, 128, 256. At frame **128.5** the candidate yielded 256 and native 128: the candidate follows the literal time-based rounding rule; this is not full Chrome parity.

Independent Node VM review verified duplicate quantized scheduling, the final partial quantum, signal transitions, and exact boundaries at 44.1/48 kHz. All four exact boundary cases stayed on the requested frame on the candidate; two were early on base. No blocking objection.

Reproduce with `node tests/e2e/audio-offline-suspend.mjs "$CHROME" "$WPT_DIR" [module-path]` from the candidate. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Base/candidate use the exact JS factory in the same signed Chrome build over a pipe; omit module path for native. Base exits 1 after emitting its measurements, candidate/native exit 0. No Pyro binary rebuilt, Cargo slots, listener ports, withheld fixtures or broad replay. Host sample: load 1.26/1.35/1.42, memory 85% free, disk 14 GiB available.
