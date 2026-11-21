# Intrinsic and stretch constraints on floats

Studio LP 0.0.17 (JSC `0e0dd7038a7a-6f4649e395e0`), Google Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Main inventory 8,082 URLs; 41 separate supplemental URLs; 39 withheld, not scored. No expectations changed.

| Main build | Strict pages | Named passes | Strict +/− vs parent | Named +/− vs parent |
|---|---:|---:|---:|---:|
| Class-prefix parent `4d50f0cd` | 5,551 | 178,487 | — | — |
| Block float clamp `5a98b7e2` | 5,549 | 178,500 | 0/2 | 15/2 |
| Inline and block floats `a910bdbe` | 5,552 | 178,540 | 3/0 | 40/0 |
| Chrome stable | 6,950 | 182,963 | — | — |

The floated-element keyword WPT passed 39/74, 45/74, and 74/74 across the three LP builds; Chrome passed 74/74. The block float path did not apply intrinsic width constraints, while the inline formatting path always placed floats in max-content available space. Both paths must respect the containing block and the float's margin box. Ten matched cross-feature controls gained 35 names and one strict page over class-prefix, with no losses. Two earlier intermittent strict losses (offscreen transition and root scroll-marker activation in an iframe) occurred at the intermediate checkpoint and returned in the final run.

The final main set gained 1 strict page and 53 names without losses versus class-prefix; against combined `2845483e`, it gained 52 pages and 632 names without losses. Chrome alone passed 1,670 pages and 10,225 names; LP alone passed 272 and 5,802. LP observed 194,540 names (11 timeouts, one hang); Chrome observed 194,912 (10 timeouts, one hang). Supplemental sets remained unchanged: LP 27 pages / 577 names, Chrome 35 / 619. Directional artifacts retain exact URL/name sets, with candidate on the left.
