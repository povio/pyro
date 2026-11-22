# Finite offline Web Audio chunks — Studio, 2026-11-22

Matched source baseline `5a0c728e8`, candidate `bae3648d2`, native signed Google Chrome `158.0.8127.37` (`codesign --verify` succeeded), WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Candidate and baseline use the actual Web Audio JS factory injected into the same browser with positive identity checks. These are not Pyro binary or RC.5 results. Only finite contexts with the default 128-frame quantum are covered; indefinite contexts, custom offline quanta, close, and general render-thread task interleaving are not claimed.

Use `node tests/e2e/audio-finite-chunks.mjs "$CHROME" "$MODULE" "$WPT_DIR"` from the source topic; use an empty module argument for native. The runner only serves two vetted WPT pages and their harness. No withheld or crash fixtures, Cargo jobs, HTTP ports or new build target.

| Matched results | Baseline | Candidate | Native |
|---|---:|---:|---:|
| WPT passing pages (one named test each) | 1/2 | 2/2 | 2/2 |
| Focused queue, state and signal checks | 4/8 | 8/8 | 8/8 |
| Elapsed seconds, including browser startup | 0.559 | 0.708 | 0.614 |

The WPT gain is `current-time-block-size.html`. Incremental data integrity already passed when the baseline returned a single one-shot buffer, so it is not counted as a gain. Newly passing controls cover synchronous running state, actual `[128,256,21]` outputs, final quantum clock and zero-size rejection. No measured losses. Separate suspension regression: 21/21 named harness outcomes and five PCM pause controls, 0.551 seconds.

Independent scripts and raw outcomes are in `evidence/round7-offline-independent` on the evidence branch. The queued stereo checks use an independent sample oracle, not just a comparison with one-shot output. The lifecycle trace includes timer observations, which are diagnostic rather than required frame synchronization.

Studio reported 13 GiB available before and after, load averages 1.32/1.47/1.48 initially and 1.38/1.41/1.45 near completion; `memory_pressure -Q` reported 84% free. No shared artifacts were removed.
