# AudioParam cancellation and hold

Studio, November 22, 2026, 08:44–08:49 CET. Baseline `a3cfe90f2`, candidate `6a537df87`. The fixed-head census `01f49f43` recorded twelve failing assertions on `audioparam-cancel-and-hold.html`; the integrated baseline reproduced all twelve. The adjacent `cancel-scheduled-values.html` had one failing assertion. These are ordinary offline parameter automation pages; neither requires user input or external resources beyond the allowlist. No withheld or crash fixtures.

| Measurement | Base | Candidate | Native |
|---|---:|---:|---:|
| cancel and hold WPT | 94/106 | 106/106 | 106/106 |
| cancel scheduled WPT | 1/2 | 2/2 | 2/2 |
| Browser PCM boundary controls | 3/5 | 5/5 | 5/5 |
| Entire matched process, seconds | 0.71 | 0.75 | 0.59 |

13 WPT gains, zero previously passing names lost, two strict page gains. All three harnesses completed both pages. Native and candidate pass counts agree; three passing audit labels contain different diagnostic strings. The held ramp endpoint is stored with float precision. Curves retain their original interpolation duration and an exact, separate cutoff; ordinary cancellation removes an overlapping curve rather than truncating it. Idle holds do not introduce a spurious future ramp anchor.

Independent Node VM QA found a floating-point endpoint regression in an intermediate implementation: at 48 kHz a new event at frame 27 overlapped a supposedly truncated curve starting at frame 3. Exact cutoff storage fixed it. Rerun of the unchanged independent controls: 8/17 baseline, 17/17 final. Every successful render compared all 1024 frames with a numerical oracle. `qa.cjs` and the final rerun `qa.txt` are included.

Signed Google Chrome 158.0.8127.37 (`codesign --verify` succeeded), WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Base and candidate ran the actual JS factories in separate Chrome processes over a pipe, with an installation/identity guard; native used no injection. This is not an actual RC binary measurement. The RC5 pilot found locally at start identified its binary as transfer SHA `c3372ad21`, not the RC5 release. No Cargo target, build slots, listening ports, or census shards used. At completion: 13 GiB disk available, host load 1.07/1.36/1.50, free-memory sample 84%.

Reproduce from the candidate checkout with `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT_DIR" "$MODULE_PATH" hold`. For native pass an empty module path. Extract baseline source with `git show a3cfe90f2:crates/pyro-webapi/src/webaudio.js > /tmp/base-audio.js`. For the independent suite run `node qa.cjs /tmp/base-audio.js /path/to/candidate/webaudio.js`. The WPT runner deliberately returns nonzero for baseline failures after emitting raw outcomes.
