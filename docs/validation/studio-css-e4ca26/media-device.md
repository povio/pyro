# Media queries: document viewport versus device screen

Studio LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, compared with Google Chrome stable 158.0.8127.37 at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Same 8,082 main URLs, six jobs, wptserve and testdriver as `offset-atomic-geometry.md`. The separate 41 worker/environment URLs use their original supplemental reference; 39 withheld URLs have no score. No Chrome 157 data is pooled here.

| Main binary | Strict pages | Passing names | Observed names | Strict +/− vs parent | Names +/− vs parent | Timeouts / hangs |
|---|---:|---:|---:|---:|---:|---:|
| `2845483e` | 5,500 | 177,908 | 194,534 | — | — | 13 / 1 |
| `2e722d05` | 5,523 | 178,058 | 194,540 | +23/−0 | +150/−0 | 11 / 1 |
| Chrome stable | 6,950 | 182,963 | 194,912 | — | — | 10 / 1 |

On the 44 matched media controls, canonical parsing, evaluation and independent live list identity gained 20 strict pages and 145 names without losses. The remaining full-run gains include timing-sensitive results, not all attributable to this change. Compared with Chrome, `2e722d05` has 261 LP-only pages and 5,700 LP-only names; Chrome alone passes 1,688 pages and 10,605 names. Supplemental pass and observation sets did not change: LP 27 pages / 577 names vs Chrome 35 / 619, 874 observed by each. The compressed outcomes and differences preserve the original statuses, missing observations and URL/name pairs.
