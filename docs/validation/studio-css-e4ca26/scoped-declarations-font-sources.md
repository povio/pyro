# Scoped declarations and font source selection

Matched main cohort (8,082 URLs), six jobs, Studio Chrome stable 158.0.8127.37, WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Same JSC and runner as `combined-property-scope.md`. Gains and losses are distinct sets, not net changes.

| LP checkpoint | Strict pages | Passing names | Pages vs parent | Names vs parent | Chrome-only pages / names | LP-only pages / names |
|---|---:|---:|---:|---:|---:|---:|
| Scope declarations d6f210a7 | 5,493 | 177,513 | +1 / −0 | +5 / −1 | 1,712 / 11,105 | 255 / 5,655 |
| FontFace sources ed824dea | 5,492 | 177,550 | +1 / −2 | +40 / −3 | 1,713 / 11,068 | 255 / 5,655 |
| Weighted palette mixing 2067b7b8 | 5,495 | 177,676 | +3 / −0 | +126 / −0 | 1,711 / 10,982 | 256 / 5,695 |

The palette focus cohort gained 123 passing names and one strict page. Three renderer pixel tests passed, including weighted three-palette equivalence and transparent zero-weight output. The supplemental outcomes were identical to the parent. Both vendor patches reproduced their trees forward and in reverse. The remaining computed-value case using font-relative math was not fixed by this batch.

Both observed 194,540 names, with 11 timeouts and one hang. Chrome had 6,950 strict / 182,963 passing / 194,912 observed. The 41 supplemental URLs were unchanged at 27 / 577 / 874 versus Chrome 35 / 619 / 874. 39 URLs remain withheld, not passes. Main and supplemental `scopedecl2` and `fontsrc` archives retain exact outcome and exclusive sets.

Keeping direct declarations in `@scope` exposed a repeatable failure on `scope-implicit-constructed-document.html`: the document element receives `z-index: 1` instead of `auto`. The renderer currently associates the adopted sheet with a synthetic style element. This is an open regression; discarding the declaration would only hide it.

The focused 30-URL font cohort gained 40 names without losses (`format-specifiers-variations.html`: 8/48 → 48/48). The full font-source run lost one name each on constructable-disallow-import, transition-ready-time-offscreen and CSSTransition-startTime. In a separate 35-URL six-job triplicate, the disallow-import page passed all runs of both immediate parent and candidate; offscreen failed on one parent and two candidate runs; startTime had a failure on every run of both. Do not subtract those losses from the measured main difference or claim they are fixed. `css-font-scope-repeats.json.gz` preserves all nine runs including the earlier scope-group control.
