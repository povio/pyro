# WaveShaper curve: same-origin iframe follow-up

Base `3a6b80301`; candidate `89b7e3adf`. The focused probe `tests/e2e/audio-waveshaper-realm.mjs` launches signed Google Chrome 158.0.8127.37 (Google LLC, EQHXZ8M8AV) over a pipe. With argument 2 pointing at `webaudio.js`, it installs that exact checkout's OfflineAudioContext in the page; without argument 2 it tests native Web Audio. This exercises actual JS realms and the repository renderer, not a rebuilt Pyro browser or its frame bindings.

| Control | Base JS | Candidate JS | Native |
|---|---|---|---|
| Foreign Float32 subarray, offset 1, length 3 | TypeError | accepted | accepted |
| Snapshot after source mutation and iframe removal | null | −1, 0.375, 1 | −1, 0.375, 1 |
| First offline sample, silent input | 0 | 0.375 | 0.375 |
| Wrong typed array with Float32 tag, plain array, DataView, resizable-backed foreign Float32 | TypeError ×4 | TypeError ×4 | TypeError ×4 |
| One-element foreign Float32 | TypeError | InvalidStateError | InvalidAccessError |
| Clear, then assign local Float32 | accepted, accepted (base never assigned) | accepted, InvalidStateError | accepted, accepted |
| Process wall incl. launch and exit | 0.63 s | 0.50 s | 0.49 s |

Reproduce by running `node tests/e2e/audio-waveshaper-realm.mjs "$CHROME" [module-path]` from the candidate with bench dependencies installed. Use `git show 3a6b80301:crates/pyro-webapi/src/webaudio.js` to obtain the base module; its probe is expected to exit 1 after printing the measurements. Candidate and native exited 0. No Cargo slots, listener ports, or full WPT repeats. End sample: 14 GiB disk available, memory pressure reported 85% free, load averages 1.82/1.63/1.50.
# Independent QA

Independent reviewer reported 70 passing assertions using the actual base/candidate module with Node 26.13.1 `vm` realms. Controls included foreign typed-array subclasses, shared/resizable backing buffers, constructor one-shot semantics, and mutation of both source and returned getter data; all 128 candidate samples retained the original 0.25 midpoint. No blocking objections. Reviewer did not rerun the browser fixture.
