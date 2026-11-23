# Media queries: document viewport versus device screen

Studio LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, compared with Google Chrome stable 158.0.8127.37 at WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Same 8,082 main URLs, six jobs, wptserve and testdriver as `offset-atomic-geometry.md`. The separate 41 worker/environment URLs use their original supplemental reference; 39 withheld URLs have no score. No Chrome 157 data is pooled here.

| Main binary | Strict pages | Passing names | Observed names | Strict +/− vs parent | Names +/− vs parent | Timeouts / hangs |
|---|---:|---:|---:|---:|---:|---:|
| `2845483e` | 5,500 | 177,908 | 194,534 | — | — | 13 / 1 |
| `2e722d05` | 5,523 | 178,058 | 194,540 | +23/−0 | +150/−0 | 11 / 1 |
| `f7335550` (`screenenv`) | 5,523 | 178,114 | 194,540 | +0/−0 | +56/−0 | 11 / 1 |
| `99f4a90b` (`zeroframe`) | 5,524 | 178,127 | 194,540 | +1/−0 | +13/−0 | 11 / 1 |
| `b86bc9a3` (`mqfont`) | 5,526 | 178,130 | 194,540 | +2/−0 | +3/−0 | 11 / 1 |
| Chrome stable | 6,950 | 182,963 | 194,912 | — | — | 10 / 1 |

On the 44 matched media controls, canonical parsing, evaluation and independent live list identity gained 20 strict pages and 145 names without losses. The remaining full-run gains include timing-sensitive results, not all attributable to this change. Compared with Chrome, `2e722d05` has 261 LP-only pages and 5,700 LP-only names; Chrome alone passes 1,688 pages and 10,605 names. Supplemental pass and observation sets did not change: LP 27 pages / 577 names vs Chrome 35 / 619, 874 observed by each. The compressed outcomes and differences preserve the original statuses, missing observations and URL/name pairs.

Propagating the top-level screen environment into child frames, preserving zero CSS viewport dimensions and resolving initial media font metrics accounted for all subsequent gains on `media-device-controls.json`. At `mqfont` the controls pass 35 pages / 1,730 names versus stable's 28 / 1,702; all 1,349 names on `test_media_queries.html` pass. Across the main corpus, Chrome alone passes 1,686 pages / 10,534 names, and LP alone passes 262 / 5,701. None of these three checkpoints lost parent passes or observations on either cohort. The 11 timeouts and one hang remain in the main outcomes, not counted as strict passes.
