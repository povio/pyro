# Counter-style descriptor parsing and CSSOM

Parser source `48c01748`, compared with `4c2d2ce3` (`vtrule`). LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, Chrome stable `158.0.8127.37`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Six jobs, testdriver, same 8,082 URLs. The 52 counter-style, list and CSSOM controls are a subset of main.

| Cohort | Parent strict / named | Candidate strict / named | Chrome strict / named |
| --- | ---: | ---: | ---: |
| Main | 5,713 / 181,469 | 5,724 / 181,532 | 6,950 / 182,963 |
| Focus | 39 / 1,036 | 50 / 1,098 | 44 / 841 |
| Supplemental | 27 / 577 | 27 / 577 | 35 / 619 |

Main gained 11 strict pages and 63 named passes, with no losses or changes in observed names. Focus accounts for 62 of the names; do not attribute the additional full-run pass to the parser without a targeted repeat. Chrome-only: 1,506 pages / 7,332 names. LP-only: 280 / 5,901. LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. Supplemental outcomes were unchanged; another 39 withheld URLs are unscored.

The first parser trial rejected valid image symbols and `spell-out`; the second accepted images outside the descriptor context and lost a list-style parsing control. Both trials were rejected. The accepted build retains those prior passes. The change supports CSSOM syntax and serialization; it does not paint image markers or synthesize speech. The `counterrules` artifacts contain exact per-cohort differences and outcomes.

Mutation follow-up `3e068926` (`counteredit`) retains the parsed Stylo rule for CSSOM getters, serialization and validation of writes. A binding test checks escaped renaming, invalid writes, fixed-system parameter changes, and mutation after detachment. All 14 CSSOM parser tests and the binding test passed. The matched main, 52-URL focus and supplemental comparisons against `48c01748` show identical strict, named and observed sets, not just equal totals. See the `counteredit` artifacts. Both vendor patches round-tripped against their upstream sources.
