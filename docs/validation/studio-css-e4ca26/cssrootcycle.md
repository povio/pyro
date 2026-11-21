# Studio closeout: CSS namespace and root scrollbar cycles

Source `b2a76c02419f64933e8e00421ed025a029fb86f0` was tested against `csspriority` at `6798e7b953f2805fe0987df6b2223a26c9104d8a`. That base and integrated `fa825a78aed59e718b97553bc2837b1c79048c16` have the same tree. LP 0.0.17, patched JSC `0e0dd7038a7a-6f4649e395e0`, Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Six workers and wptserve/testdriver throughout; worker globals enabled for the supplement only. A strict page requires a completed, successful harness and at least one named assertion, all passing. Named pass sets are keyed by URL and assertion name.

| Immutable LP binary | SHA-256 | Build | Standalone focus |
|---|---|---:|---:|
| Parent (`csspriority`) | `ba6d319580766dc556da9c052a2a95b7638f6f6418e9c6d8f95e54e9e893f3c6` | previously archived | previously archived |
| Namespace only | `202096bb24812feae030597f1e04ad85c2ce2f2408656f7705e6dff0e89f6a9e` | 16 s | 273 URLs, 0.9 s |
| Cumulative | `5cd38ee7811fae2fd76aae14d9fe6bff4a14888960f4bd57f9cef86919e03847` | 58 s | 29 URLs, 0.3 s |

The namespace-only main run (346.5 s) gained both `css/cssom/escape.html` and `css/cssom/CSS-namespace-object-class-string.html`: one surrogate-preservation assertion and four namespace-tag assertions. All five also pass in Chrome. There were no losses or changes in observed names; the supplement (0.3 s) was unchanged. Its exact differences are archived separately from the cumulative result.

The cumulative 29-URL focus recovered the single assertion and strict page on `viewport-units-scroll-no-cycles-001.tentative.html`, with no other changes versus the parent. This restores a previous LP-only pass, not a Chrome gain: the pinned Chrome fails the same assertion. `cssom_namespace_preserves_webidl_string_shape` and `css_root_viewport_condition_cycle_guards_only_scrollbar_properties` passed natively. The latter covers a registered length through a `var()` alias, unrelated opacity, non-root overflow and mutation to an absolute length. The Stylo patch verified in both directions against the integrated base. No expectations or corpus entries changed.
# Matched cumulative result

| Cohort | Parent pages / names | Final LP pages / names | Chrome pages / names | LP-only pages / names | Chrome-only pages / names |
|---|---:|---:|---:|---:|---:|
| Main (8,082 URLs, 344.8 s) | 5,820 / 182,428 | 5,823 / 182,434 | 6,950 / 182,963 | 288 / 5,921 | 1,415 / 6,450 |
| Supplemental (41 URLs, 0.3 s) | 27 / 577 | 27 / 577 | 35 / 619 | 0 / 0 | 8 / 42 |

The main comparison gained three strict pages and six named passes, losing none. LP observed the same 194,673 names as its parent; Chrome observed 194,912 (260 observed only by LP and 499 only by Chrome). Main LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. The supplement observed the same 874 names for both engines. The 39 withheld URLs remain unscored. The Chrome outcome records matched the pinned parent reference byte for byte after decompression for both cohorts. The earlier `if-cycle.html` case 4 loss remains outstanding; this batch did not change it. See the compressed directional differences for the full pass sets and observation differences.
