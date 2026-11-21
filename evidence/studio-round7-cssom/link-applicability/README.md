# Disabling loaded data-URL stylesheets

Frozen ancestor `305840f45afbc4f55bfa24f303c1e858d30e4c1d`. Parent `1d1f9138ef70faf5f751e7d8a715be48acbc1edf` differs from the tested parent executable (`0213488a`) only by evidence. Candidate `dd19a5242aa96fbcb82a61e548a28a50814d0535`. Patched JSC `0e0dd7038a7a-6f4649e395e0`, Chrome `158.0.8127.37`, WPT `a421f2fea5eb`. All ten URLs are completed archive cases outside the withheld list. Runs were sequential with isolated wptserve ports and four jobs.

| Run | All-pass pages | Named passes | WPT wall |
| --- | ---: | ---: | ---: |
| Parent | 4/10 | 46/52 | 217 ms |
| Candidate | 6/10 | 48/52 | 120 ms |
| Chrome | 10/10 | 52/52 | 858 ms |

Only `HTMLLinkElement-disabled-001.html` and `HTMLLinkElement-disabled-002.html` changed: the reflection and applicability subtest now passes on each. Identical named-subtest inventories, no losses, all harnesses completed, no watchdog kills or restarts. Alternate-link cases 004 and 006, stylesheet-title, and style-sheet-interfaces-001 still have one failure each.

Native DOM integration: two pre-existing controls passed on both builds. The new applicability revision check failed on the parent and passed on the candidate. Parent/candidate native command times including compilation: 48.97 s / 6.63 s. Candidate browser build: 14.00 s. The JSON files are exact runner output; native receipts start at the test results.
