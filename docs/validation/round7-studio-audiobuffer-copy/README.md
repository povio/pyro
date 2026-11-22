# AudioBuffer channel copies

Matched base `d078351a6`, candidate `db1ce45d2`, pinned WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, signed Google Chrome 158.0.8127.37. Census `01f49f43` identified four ordinary assertions where both copy methods accepted shared Float32 views, with and without the optional offset. The other three failures on this page were task summaries, not three additional defects.

Only `webaudio/the-audio-api/the-audiobuffer-interface/audiobuffer-copy-channel.html` and an independent two-channel signal control ran. The fixture intercepts the page and its four named local dependencies without a server. Candidate and base run the repository's actual JS factory in signed Chrome, whereas native runs Chrome's own API. This isolates this JS-only change without rebuilding, allocating a Cargo target, or claiming a Pyro executable was tested.

| Measurement | Base JS | Candidate JS | Native Chrome |
|---|---:|---:|---:|
| Strict pages | 0/1 | 1/1 | 1/1 |
| Passing assertions | 55/62 | 62/62 | 62/62 |
| Process wall incl. browser launch | 0.60 s | 0.51 s | 0.63 s |

All 55 previously passing names stayed passing. Candidate and native matched on 60 of 62 passing names; two already-passing invalid-channel assertions display `4294967295` versus `-1` in the exception text. The foreign-realm offset copy produced `[-1, 0.25, -0.75, 0]` on candidate and native; base threw TypeError in both directions and left the destination at −1. The unselected channel remained silent. Independent Node 26.13.1 VM review passed 14 controls (seven per method), including overlapping copies, clipping, end offsets, shared/resizable rejection, and invalid channels. No blocking objections.

Reproduce from candidate: `node tests/e2e/audio-buffer-copy.mjs "$CHROME" "$WPT_DIR" [module-path]`. Pass the base or candidate `webaudio.js` for the module runs, omit for native. The base exits 1 after emitting its receipt, as expected. No ports or Cargo slots used; end sample 14 GiB disk available, memory 85% free, load averages 1.41/1.56/1.51. No withheld or crash fixtures ran.
