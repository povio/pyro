# CSS Typed OM numeric type algebra

Independent QA compared exact `cd02ceb1bb3117e0a35c8a9d8f1d71e56e20d2b6` with direct parent `e05d64f5d60e8fb3aabd407f59ee2832303c4c6e` on ten frozen Window testharness pages from pinned WPT a421f2fea. Studio Chrome 158.0.8127.37 passed all 191 named occurrences. Pyro rose from 173 to 186 (+13/−0). Four type-addition, three multiplication and four arithmetic targets passed. Correct rejection of incompatible calc units and successful clamp parsing supplied two further gains. All ten harnesses completed without restarts.

Matched direct probes moved from 2/10 to 10/10 agreement with live signed Chrome. The native guard covers inversion, cancelling powers with a retained percentage hint, conflicting hints, returning independent type dictionaries, and same-unit arithmetic. It passed in 0.07 seconds after a 46.17-second native build. The one-job executable build took 33 seconds. [CSS Typed OM numeric type operations](https://drafts.css-houdini.org/css-typed-om-1/#numeric-typing)

The remaining five failures are parsing calc sums with compatible lengths or numbers and converting a clamp, reciprocal unit, or complex expression with to(). They remain included in the score. This batch changes Typed OM value objects and leaves CSS layout calculation outside its scope.

## Representative costs

Eight fresh serial processes ran in ABBA/BAAB order. Each fixture used one warm-up and five measured samples of 1,024 operations per process. Wall medians include the page evaluation; CPU means are from process-tree counters. The nested product returned the same length exponent on both heads, but only the candidate also preserves the required percent hint.

| Fixture | Parent median ms | Candidate median ms | Parent / candidate CPU ms |
| --- | ---: | ---: | ---: |
| unitSum | 0.753 | 0.671 | 2.5 / 2.5 |
| mixedType | 1.526 | 1.656 | 4.5 / 3.5 |
| nestedProduct | 2.871 | 2.677 | 4.0 / 4.5 |

Host occupancy averaged 42.02% over 2.710 seconds, with 42.02% during baseline arms and 42.02% during candidate arms. These short-run timings are descriptive. [Per-arm host receipt](host.json)
