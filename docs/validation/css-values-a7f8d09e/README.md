# CSS Values focused differential

The candidate at `672ba0c74` recovered 27 of the 1,221 Chrome-only named passes in the supplied flexcontent inventory. Across 26 pages it increased named passes from 964 to 991 of 1,032 observed names, and fully passing pages from 17 to 20. Both runs observed the same names. All 14 animation and transition parsing guard pages retained their base outcomes.

| Change | Recovered names |
| --- | ---: |
| Evaluate exceptional pow(), log() and round() operands during folding and resolution | 19 |
| Resolve constant duration and delay expressions before CSSOM serialization | 8 |

The time arithmetic uses Stylo’s existing finite f32 time range. Pinned Chrome uses a wider range for infinity. The WPT requires a sufficiently large finite result. Element-dependent times, including expressions with relative font units, still need an element computation context.

## Inventory

The URL-level classification and all 27 recovered names are in `evidence.json`. The buckets record the feature and observed symptom. Unimplemented feature families have not all been individually traced.

| Chrome-only names | Feature or cause |
| ---: | --- |
| 728 | Reserved calc-size and interpolate-size layout, interpolation and composition |
| 246 | Inline if() substitution, conditions, cycles and invalidation |
| 78 | random() in functions, conditionals, descriptors and computed values |
| 54 | Numeric evaluation, clamping and serialization |
| 25 | Tree-counting functions in keyframes, descriptors and shadow trees |
| 22 | Animated calc() extrema and used-value clamping |
| 19 | Angle test names incorporate each engine’s serialized rotation matrix. All 38 page assertions passed on the base. |
| 15 | Typed attr() parsing and substitution |
| 14 | Font-relative metrics, invalidation and line height |
| 10 | Viewport units, scrollbar reservation and media queries |
| 9 | URL resolution and request modifiers |
| 1 | inherit() substitution |

The largest available family, if(), needs conditional branch substitution and dependency tracking in Stylo. Its substitution machinery on the assigned base recognizes var(), env() and attr(). The numeric batches use the existing evaluation and timing paths.

## Reproduction and cost

Use the source and binary SHAs, WPT revision and JSC cache key in `evidence.json`. Extract the URL list from the `results[].url` field in `base.json.gz`. The two compressed reports contain every named result and the runner configuration. Use the Round 6 no-native-retry runner with wptserve, manifest inventory, the pinned manifest, the URL list and one browser job. Each run started its own server on a free port. Both runs completed with zero restarts and watchdog kills.

The baseline render-enabled wptiter build took 312 seconds with two Cargo jobs. Incremental builds for arithmetic, time handling and final review changes took 60, 38 and 60 seconds. Final WPT page execution took 1.154 seconds on base and 0.671 seconds on the candidate, excluding server startup. The native arithmetic integration test and five timing unit tests passed. Their final builds took 32 and 10 seconds after a 65-second initial native build. That initial run used the earlier library while a new log() assertion was being added and failed on the negative-zero case. Both evaluation paths passed on the rebuilt final source. The vendor patch round-trip also passed.

From checkout alignment through validation and packaging, elapsed time was approximately 13 minutes, including overlapping implementation and builds. Studio combined validation remains pending.
