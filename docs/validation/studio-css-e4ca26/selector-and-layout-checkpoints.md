# Selector, shorthand and writing-mode checkpoints

Studio LP 0.0.17 with JSC `0e0dd7038a7a-6f4649e395e0`; Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Main: identical 8,082 URLs, wptserve, testdriver, six jobs. Supplemental: 41 separate URLs, worker globals enabled. The remaining 39 are withheld, not passes. Historical Chrome 157 is not pooled. A strict page requires completion, zero harness status and at least one real named assertion, all passing. Name identity is `(URL, name)`.

| Main candidate | Strict pages | Passing names | Pages gained/lost | Names gained/lost |
|---|---:|---:|---:|---:|
| `798563e7` inset serialization | 5,526 | 178,178 | 0/0 | 48/0 |
| `a3b6d3b8` strict selector feature queries | 5,531 | 178,206 | 5/0 | 45/17 |
| `6f731d5b` column-rule serialization | 5,531 | 178,214 | 2/2 | 11/3 |
| `f2f3c213` vertical collapsed margins | 5,534 | 178,360 | 3/0 | 146/0 |
| `199b5d43` highlight cascade and pseudo selectors | 5,542 | 178,417 | 9/1 | 58/1 |
| `4d50f0cd` dash-terminated class prefixes | 5,551 | 178,487 | 9/0 | 71/1 |
| Chrome stable | 6,950 | 182,963 | — | — |

Each LP main run above observed 194,540 names, with 11 timeouts and one hang; Chrome observed 194,912, with 10 timeouts and one hang. Each LP supplemental run passed 27 pages and 577 names against Chrome's 35 and 619, both observing 874 names. The compressed outcomes preserve per-URL status and names; the directional differences include strict pages, named passes and observations. `leftOnly` means candidate-only, not reference-only.

The column-rule change gained both focused shorthand pages and 11 names. Three unrelated full-run timing losses are retained. In three matched parent/candidate repeats the offscreen transition passed 2/3 vs 1/3, the root scroll marker 3/3 vs 3/3, and the transition start-time page passed 4/5 assertions in each parent repeat and 4, 4, 3 in the candidate. The first two names returned in the margin checkpoint; those are not credited as layout improvements. The layout change accounts for 144 new anchor-position names and makes all 1,296 assertions on `anchor-position-writing-modes-001.html` pass. The highlight checkpoint again lost the offscreen transition in the full run.

The 17 strict-query losses were selector capability gaps, not timeout noise. On 66 matched controls, pre-strict / strict / repaired binaries passed 34 / 39 / 44 strict pages and 3,170 / 3,198 / 3,230 names. The repair gained 32 names with no losses over the strict parser; against pre-strict it gained 61 and lost one, `:is(.foo-*)`. The highlight repair includes wildcard serialization, zero specificity, computed cascade and painting rule matching, not just `CSS.supports()`. Recognition of spelling, grammar, search and checkmark selectors does not imply a native UI for those features.

`4d50f0cd` implements class prefixes in selector parsing, serialization, specificity, DOM and style matching, and mutation invalidation. On 73 matched controls it gained eight strict pages and 70 names with no losses. All eight class-prefix WPTs pass, including quirks, `:has()`, querySelector and invalidation. The pre-strict `:is(.foo-*)` assertion now passes through actual prefix support.

Against `2845483e`, the class-prefix binary gained 51 strict pages and lost none; it gained 580 named passes and lost one: “The start time of transitions is based on when they are generated”. That previously intermittent loss is retained. On the main cohort Chrome alone passes 1,671 pages / 10,278 names and LP alone passes 272 / 5,802. LP alone observed 309 names, Chrome alone 681. The supplemental pass sets were unchanged; the 39 withheld URLs remain unscored.
