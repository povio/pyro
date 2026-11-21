# Scroll axis locking

Studio, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, stable Chrome `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. The parent and candidate ran the same 239 URLs, six workers: the main cohort's overflow and overscroll-behavior directories plus XML initial-value enumeration. The stable reference uses the same URLs. Outcomes include missing names and harness statuses.

`3db4c45b` adds the non-inherited discrete CSS property to Stylo. With `none`, the UA must not lock the scroll to an axis; with `auto`, locking is optional. LP already consumes wheel delta on each axis independently. No input behavior or test expectations changed.

| Build | Strict pages | Named passes |
|---|---:|---:|
| Parent `a69223b2` | 116 | 1,361 |
| Candidate `3db4c45b` | 120 | 1,420 |
| Chrome stable | 216 | 1,696 |

Four new strict pages: valid parsing, computed value, inheritance and discrete interpolation. The 59 new passes comprise 6, 8, 2 and 42 names on those pages, plus one new XML enumeration assertion. Zero LP-only page or name losses. Candidate-exclusive versus Chrome: 3 pages / 52 names; Chrome-exclusive: 99 / 328. Touch, viewport and subframe input failures remain in the results; no interaction-test gain is claimed. This focused set does not replace the 8,082 main, 41 supplemental and 39 withheld accounting.
## Viewport and nested-frame scrolling

Full axis-lock checkpoint: the 8,082 main URLs gained four strict pages and 59 named passes (5,595 / 180,068), with zero page, pass or observed-name losses versus `a69223b2`. Stable-only: 1,627 pages / 8,697 names; LP-only: 272 / 5,802. Both LP builds recorded 11 timeouts and one hang. The 41 supplemental URLs were unchanged at 27 / 577, against stable 35 / 619; the 39 withheld URLs remain unscored.

The negative-direction wheel checks failed **before** dispatch: the viewport clamped `scrollLeft` to 3,208 rather than 3,223; inside the frame it clamped to 800 rather than 815. The scrollbar occupied those 15 CSS pixels. The clamp now uses the visible scrollport instead of the entire frame; root client geometry still uses the layout viewport on mobile. The two wheel pages strictly pass (+2 pages, +4 names; no losses across the 239 URLs). All 298 additional scroll-snap, writing-mode, and CSSOM View control URLs had identical passing and observed sets against the parent (128 strict, 1,900 named). The desktop Chrome controls passed 256 strict / 2,334 named. This change does not claim new touch coverage.

## Remaining gap ranking at the full matrix checkpoint

The subsequent full scrollport-range checkpoint (`2a384504`) reached 5,597 strict pages and 180,072 named passes, gaining 2 / 4 versus axis-lock. No passing pages, passing names or observed names were lost. LP-only vs stable: 272 / 5,802; stable-only: 1,625 / 8,693. All 8,082 main URLs were present, including 11 LP timeouts and one hang; the 41 supplemental and 39 withheld were unchanged and remain separate.

The exact main differences at `64833ed4` contain 1,631 Chrome-only pages and 8,759 Chrome-only names. Grouping names gives 837 across `calc-size/` (20 Chrome-only strict pages), 369 across CSS mixins (24), 331 across random-function pages (6), 242 across conditional-substitution pages (7), and 148 rounded-polygon names (10). The first four are investigation families, not proven single root causes. Overflow (99), conditional rules (93), scroll snap (92), and anchor positioning (91) led by Chrome-only page count before the axis-lock work. The 24 unresolved repeater-color names are a subset of 30 Chrome-exclusive names on the two rule-color interpolation pages. No rounding tolerance was changed.
