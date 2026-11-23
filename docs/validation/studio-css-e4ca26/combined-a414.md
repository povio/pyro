# Studio CSS at the combined integration revision

Tested `a414fab44075afe8d7a50a66a7db047e4c5793d9`, not a CSS-only parent binary. All 13 vendor patches apply and reverse-apply against the upstream reconstructed at its first parent `902fe811`. LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable 158.0.8127.37. Same six-job wptserve/testdriver inventories and stable reference as the percentage-bases checkpoint.

| Cohort | Engine | Strict pages | Passing names | Observed names |
|---|---|---:|---:|---:|
| Main, 8,082 URLs | Prior CSS head | 5,460 | 176,773 | 194,540 |
| | Combined | 5,459 | 176,777 | 194,540 |
| | Chrome | 6,950 | 182,963 | 194,912 |
| Supplemental, 41 URLs | Prior CSS head | 27 | 572 | 874 |
| | Combined | 27 | 577 | 874 |
| | Chrome | 35 | 619 | 874 |

Main combined/prior exclusive: 0/1 strict pages, 5/1 names. The loss is `transition-ready-time-offscreen.html`. Combined/Chrome exclusive: 253/1,744 strict pages, 5,647/11,833 names. Both engines had one hang; LP timed out on 11 URLs, Chrome on 10. No missing URLs. Supplemental (4 UTF-16, 37 worker): +5/-0 named, zero page changes from prior; no timeouts or missing. The 39 security/crash exclusions remain unrun. Sibling `difference-combined-*` files contain exact page, passing and observed sets exclusive to either side; `outcomes-combined-*` preserve all outcomes.

## Why the two mask failures remain

On the pinned `mask-valid.sub.html`, both engines pass 45 of 55 names, but not the same 45. LP fails two “should not set unrelated longhands” checks. `shorthand-testcommon.js` removes every longhand named by the test and then checks declaration length. Both maps omit `mask-border-mode`; setting `mask` declares it, leaving exactly one. [CSS Masking §7.9](https://www.w3.org/TR/css-masking-1/#the-mask) explicitly includes this reset. On `mask-shorthand-subproperties-reset.html` LP runs and passes all 14, including the luminance-to-alpha reset. Chrome's support guard skips all six mask-border longhands and Chrome passes the eight remaining names. Do not drop the declaration, hide it from CSSOM, or count the skipped Chrome assertions as passes. No expectations changed.
