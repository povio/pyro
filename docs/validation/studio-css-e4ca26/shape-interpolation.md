# Shape interpolation on Studio

WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Studio Chrome stable `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Each main comparison matches all 8,082 URLs by exact URL and subtest name. A strict page requires an OK harness and every named assertion to pass. Each candidate used six jobs, wptserve and testdriver. The 41 supplemental URLs are scored separately; 39 withheld URLs are not passes.

| Binary | Main strict pages | Main passing names | Gains/losses relative to preceding row (pages; names) |
|---|---:|---:|---|
| Parent `fafdfaa4` (`eventobject`) | 5,695 | 181,218 | — |
| `217431bf` (`viewbox`) | 5,697 | 181,249 | +2/−0; +31/−0 |
| `122f9ca0` (`bordershape`) | 5,698 | 181,329 | +1/−0; +80/−0 |
| `229e0e59` (`shaperead`) | 5,699 | 181,335 | +1/−0; +6/−0 |
| `f23382a0` (`outlineauto`) | 5,700 | 181,367 | +1/−0; +32/−0 |

The final change interpolates automatic outline color as currentcolor on non-auto outlines, leaving automatic outlines distinct. Its only main gains were on the outline-color interpolation page. The 61 focused pages and 41 supplemental URLs lost no passes. Relative to `fafdfaa4` the four changes gained five strict pages and 149 named passes with zero losses. Chrome-only: 1,528 pages / 7,494 names; LP-only: 278 / 5,898. Coverage and timeout counts stayed unchanged.

Resolved CSSOM now omits the default reference box for each contour and collapses identical contours with the same effective box. The specified value and paint representation stay intact. This converts the computed-value page to an LP-only strict pass; it does not reduce the Chrome-only strict page count. Main LP-only / Chrome-only: 278 / 1,529 pages, 5,898 / 7,526 names. Supplement and timeout counts unchanged.

The object-view-box fix gained 24 interpolation assertions, one transition-mutation assertion and six generic transition assertions. Border-shape gained all 80 previously missing names on its animation page, including incompatible boxes, two contours, and shape commands. Neighboring clip-path and shape-outside focused controls lost no passes. The vendor patches reconstruct the upstream and reverse-apply.

Chrome has 6,950 strict main pages and 182,963 passing names. At `122f9ca0` there are 1,529 Chrome-only pages / 7,532 names versus 277 LP-only pages / 5,898 names. No missing URLs. Main LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. Both supplemental runs are unchanged: LP 27 strict / 577 names vs Chrome 35 / 619. See each `*-compare.log`, `difference-*` and `outcomes-*` for exact pass sets and per-URL states.
