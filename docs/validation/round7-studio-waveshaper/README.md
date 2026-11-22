# WaveShaperNode: Studio Round 7

Matched source `bd247dd67babbff29fb87017f9a6bededfb68ea2` → `bbbab701ad5cb8da3de184e3463f2adf1a086c18`. Fixed census `01f49f43` showed a coherent constructor (+39), simple transfer (+14), and interpolation (+7) gap. The matched parent lacked WaveShaperNode. Only the eight ordinary pages in `urls.json` ran; no withheld or crash fixtures and no corpus extrapolation. WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; native Google Chrome 158.0.8127.37, verified with codesign (Google LLC, EQHXZ8M8AV), headless with autoplay enabled.

| Engine | Strict pages | Passing/emitted assertions | Runner wall |
|---|---:|---:|---:|
| Parent twice | 1/8 | 297/323 | 204, 136 ms |
| Candidate twice | 7/8 | 365/370 | 187, 151 ms |
| Signed Chrome | 8/8 | 425/425 | 832 ms |

Both repeats reproduce their engine's status map. All six WaveShaper pages are green. The gain control stayed green; the mixed nominal-range page still fails five unrelated factory tasks. Of exact (URL, name) keys: 294 remained passing, 17 changed Fail→Pass, 54 newly passed, 5 remained failing, 4 failing names disappeared and 3 passing names disappeared. The latter three named the parent's “undefined is not a constructor” error; the actual constructor now rejects the same invalid calls. Zero Pass→Fail. Of the 71 new or improved passes, 65 names match native passes exactly.

The direct probe (`tests/e2e/audio-waveshaper.mjs`) checks independent stereo interpolation and saturation after mutating both the assigned and returned arrays. Candidate and native returned identical samples. With a cubic transfer, the aliased third-harmonic amplitude was 0.24999 without oversampling; at 2×/4× it fell to 0.0000525/0.0000484 on the candidate and 0.0000161/0.0000121 on Chrome. The fundamental stayed near 0.75. These are sample measurements, not a claim that filter coefficients or latency match Chrome. The parent returned `supported:false`.

One deliberate native difference: the candidate follows the Web Audio 1.1 curve setter's InvalidStateError for a too-short or second non-null curve. Chrome reports InvalidAccessError for a single element and allows a second assignment. The raw probes record both; `--check-spec-assignments` asserts the candidate policy separately.

One Cargo build at a time (`-j2`, `wptiter`, incremental disabled, one isolated APFS clone of the warm target). Parent/candidate build wrapper wall 218/71 s. Binary SHA-256: `d4253ff412007e88c512e74149f3f4b299815ac72b18f9a40f586ea28a7fd0ac` / `8f14888c48844e72ed94747aad918b12a2c988589d88be1788ac9658206aecdb`. Host samples in `load.log` cover only the measurement window, not subsequent unrelated overnight work: 79.93–94.9% idle, 81–82% memory free. Disk reached 13 GiB free during the build; 14 GiB free after lane cleanup. No private target retained.

Reproduce using the pinned bench dependencies: `bun bench/wpt/serve.mjs "$WPT_DIR" 24780`, then `bun bench/wpt/run.mjs --base http://127.0.0.1:24780 --wpt "$WPT_DIR" --dirs webaudio --tests docs/validation/round7-studio-waveshaper/urls.json --jobs 1 --bin label="$BIN" --out result.json`. For the direct probe, run `bun tests/e2e/audio-waveshaper.mjs http://127.0.0.1:PORT` against the selected browser's CDP endpoint.
