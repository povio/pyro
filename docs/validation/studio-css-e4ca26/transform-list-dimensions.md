# Transform dimensionality and legacy readback

Matched direct runs on Studio: 185 CSS transform/gap-rule URLs from `transformdim-controls.json`, six workers. LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`; Chrome stable 158.0.8127.37 reference restricted to those same URLs. A strict page requires a complete, successful harness and all named assertions passing. Names are identified by URL and test name, not ordinal.

| Revision | Change | Strict pages | Passing names | Losses vs prior LP (pages/names) |
|---|---|---:|---:|---:|
| f3dff3c72 | Parent | 152 | 9,425 | — |
| ca745a33 | Preserve 2D transform-list interpolation | 155 | 9,505 | 0/0 |
| 64833ed4 | Use result matrix dimensionality in CSSOM | 159 | 9,529 | 0/0 |
| 1ef0cfb3 | Canonical property alias lookup | 159 | 9,529 | 0/0 |
| a69223b2 | Resolve legacy transform/perspective readback through longhand | 159 | 9,532 | 0/0 |
| Chrome stable | Same URLs | 182 | 9,657 | — |

The final three gains are prefixed transform assertions in `css/css-transforms/parsing/transform-computed.html`; the page does not gain a strict pass. At this checkpoint the focus has 25 Chrome-only and two LP-only pages, 128 Chrome-only and three LP-only names. Gap-rule color's 24 previously identified misses are not claimed as gains. Exact focused outcomes and set differences accompany this report. This is not a full-corpus comparison: the main 8,082, supplemental 41, and withheld 39 URLs remain separate. Withheld URLs are unscored.
## Full run: ca745a33 only

Against f3dff3c72, main gained three strict pages and 80 named passes with no pass losses (5,587 strict and 179,982 named). Chrome-only: 1,635 pages / 8,783 names; LP-only: 272 / 5,802. One previously observed **nonpassing** name disappeared (`Navigate between URLs`, `css/css-navigation/tentative/navigation-002.html`); the candidate recorded 12 timeouts versus 11. The supplemental cohort remained at 27 strict / 577 named with no changes to either pass set. These numbers do not describe the later CSSOM revisions.
