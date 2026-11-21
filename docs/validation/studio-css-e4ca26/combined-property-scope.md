# Combined CSS validation: registrations and scopes

Studio, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, local Chrome stable `158.0.8127.37`, Lightning Panda 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`. All main runs use the same 8,082 CSS testharness URLs, wptserve, testdriver and six jobs. A strict page requires successful harness completion and at least one named subtest, all passing. Named totals count distinct (URL, name) pairs even when the page fails. No expectation or inventory changes.

| Main run | Strict pages | Passing names | Observed names | Timeouts / hangs |
|---|---:|---:|---:|---:|
| Previous CSS (`a8dbdc9a`) | 5,487 | 177,343 | 194,540 | 11 / 1 |
| Combined (`5b2d00b9f`) | 5,487 | 177,350 | 194,539 | 12 / 1 |
| Registrations (`84ddf13d`) | 5,489 | 177,434 | 194,540 | 11 / 1 |
| Scope rules (`492691d7`) | 5,492 | 177,509 | 194,540 | 11 / 1 |
| Chrome stable | 6,950 | 182,963 | 194,912 | 10 / 1 |

Combined vs previous CSS: +0/-0 strict pages, +7/-0 passes. A navigation assertion was not observed during a timeout. Registrations vs combined: +2/-0 pages, +84/-0 names, all gains also passed Chrome. `at-property.html` went from 58/106 to 106/106; `at-property-cssom.html` from 7/40 to 40/40. Both now match Chrome, including escaped names, omitted universal initial values, descriptor ordering, whitespace, invalid registrations and ignored descriptors. The remaining three gains were interface assertions. The native descriptor parser regression passed. Against Chrome the registration checkpoint has 255 LP-only / 1,716 Chrome-only strict pages and 5,655 / 11,184 named passes.

Scope rules vs registrations: +3/-0 strict pages and +75/-0 named passes, all passing in Chrome. Against Chrome: 255 LP-only / 1,713 Chrome-only strict pages and 5,655 / 11,109 named passes. The 41-URL supplement had identical outcomes. The three stylesheet guard assertions passed together in two of three scope repeats; they remain intermittent. The `scopegrp` artifacts retain exact sets for this checkpoint.

**Timing control:** The three `CSSStyleSheet-constructable.html` import assertions recovered in the combined main run. On three further six-job runs per binary (same 32 URLs), all three passed 3/3 on previous CSS and registrations, but 2/3 on combined. On the failed combined repeat `startTime` was 22.2 ms after a 22.3 ms lower bound. The failing assertion precedes link removal; the two subsequent assertions saw red rather than black. `PageDom::add_resource_timing` maps fetch instants using an already-coarsened event time and a separately sampled monotonic instant, then `entry_of` floors the result again. This is consistent with, but does not independently prove, the observed inversion. Do not count the guard as fixed. `combined-cssom-guard.json.gz` preserves all named results and failure messages for these repeats and the scope candidate.

**Coverage:** Separately measured 4 UTF-16 document and 37 worker URLs; registration and combined checkpoints both had 27 strict / 577 passing names versus Chrome 35 / 619 (874 observed on each). The other 39 crash/security URLs were excluded, not counted as passes or failures. The `outcomes-*` and `difference-*` artifacts for `join5b` and `property` retain every main and supplemental outcome and the exclusive sets against parent and Chrome.
