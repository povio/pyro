# Import and namespace rules in CSSOM

Sources `bb4a5533` and `610b47ea`; executable baseline `3e068926` (`counteredit`). LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, Chrome stable `158.0.8127.37`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Six jobs, testdriver, 8,082 matched main URLs. The 314 focus URLs are a subset of main; the 41 supplemental URLs are separate and 39 withheld URLs remain unscored.

| WPT page | Parent named | Candidate named | Chrome named |
| --- | ---: | ---: | ---: |
| CSSGroupingRule-insertRule | 6 | 7 | 7 |
| at-namespace | 0 | 1 | 1 |
| delete-namespace-rule-when-child-rule-exists | 0 | 1 | 1 |
| import-namespace-invalid-prelude | 6 | 8 | 8 |
| nested-import-namespace-invalid | 0 | 4 | 4 |
| url-request-modifiers-import-parsing | 6 | 10 | 10 |

The preliminary `cssprologue` full run lost six existing valid URL modifier assertions while gaining three invalid-modifier rejections on that same page. Its comparison log and directional main differences are retained. The accepted `importurl` build uses the style engine's URL parser and restores all six names while retaining the invalid-modifier gains and canonical serialization. All 16 CSSOM parser tests passed.

Main: LP 5,724 → 5,730 strict pages and 181,532 → 181,545 named passes, with no lost strict passes, named passes or observed names. All six strict pages and 13 gains were in focus. Chrome passed 6,950 pages / 182,963 names. Chrome-only 1,500 / 7,319; LP-only 280 / 5,901. LP had 11 timeouts and one hang, Chrome ten timeouts and one hang. Supplement unchanged: LP 27 / 577, Chrome 35 / 619. The `importurl` outcomes and directional differences preserve the exact sets.
