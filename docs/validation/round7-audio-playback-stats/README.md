# Realtime playback statistics — Studio, November 22, 2026

One ordinary WPT page (`audiocontext-playbackstats.html`), selected from fixed-head non-CSS census `01f49f43d`. WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The three runs used the same signed Chrome 158.0.8127.37, routed page, harness, and autoplay policy. Native used Chrome's own implementation; the other two injected only the Pyro JS factory. These are not Pyro executable results.

| Run | Source | Named passes | Wall seconds |
| --- | --- | ---: | ---: |
| Native | Google Chrome | 4/4 | 8.21 |
| Before | `7558420fc` | 0/4 | 5.05 |
| After | `c39d98913` | 4/4 | 8.11 |

Four gains, zero losses within this page: playback growth, fresh JSON matching getters, latency reset, and execution-cycle snapshot stability. All four baseline failures were missing `playbackStats`. Raw JSON and timing stderr are adjacent. Run `node tests/e2e/audio-grain-duration.mjs "$CHROME" "$WPT" "$MODULE" playback-stats` from the topic checkout (empty module argument for native; `git show 7558420fc:crates/pyro-webapi/src/webaudio.js` for baseline). Candidate module SHA-256: `27b8a8145deefa574aaae38a7ebc92cf9eccb8cee7cc3aabb3a15f74e2b4ec54`.

Independent visible-page oscillator controls (`independent/`) confirmed duration in seconds, stable snapshots while suspended/closed, identity per context, receiver errors, and fresh six-field JSON. Waking at ~1.15 s intervals exposed sampling differences: candidate 0 → 1.157 → 2.309 s, native approximately 0 → 1.000 → 2.002 s. Pyro samples its running virtual clock on observation or halt; it does not model device underruns or physical latency. Native latency was ~17.6 ms before reset, candidate latency stayed zero. Prototype own-property order also differs. Neither visible-page test validates the hidden-document microphone permission override.

`media_decoding::playback_stats_window_binding` on the topic tests the actual browser Window on insecure and secure origins, constructor identity and 1.5 seconds of virtual clock advancement. MacBook serial validation requested separately; no binary or release result claimed here. Studio used zero Cargo slots, no listener ports, no new build target, and no cleanup. Host CPU idle at opening/closing samples: 92.32% / 97.67%; closing load averages 1.53/1.42/1.36; available disk 12 GiB. No other audio residuals were retested or counted.
