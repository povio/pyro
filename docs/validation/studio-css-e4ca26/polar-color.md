# Powerless hues on conversion

Studio pins, cohort rules and six-worker configuration match `rounded-polygons.md`. The 75-URL focus covers CSS Color and gap-rule interpolation. On conversion, Lab and OKLab use distinct chroma thresholds; HSL handles near-achromatic and extended-range RGB. Same-space authored polar coordinates do not go through the conversion. CSSOM syntax and WPT tolerances were not changed.

| Focus | Strict | Passing names | Observed names |
|---|---:|---:|---:|
| Rounded polygon parent | 56 | 12,800 | 13,420 |
| Polar color | 56 | 12,887 | 13,420 |
| Stable Chrome | 62 | 12,616 | 13,420 |

The committed candidate gains 87 names and loses no pages, passes or observed names. Its stable-only sets are 9 pages / 145 names, its LP-only sets 3 / 416. An experimental change that also normalized HSL/HWB source coordinates was withheld: two color-mix assertions lost their authored hue after passing through CSSOM serialization. The result above excludes that experiment.

Across all 8,082 main URLs, LP passes 5,607 pages and 180,307 names; Chrome passes 6,950 and 182,963. The 87 gained names are the only parent/candidate differences. Chrome alone passes 1,615 pages and 8,521 names; LP alone passes 272 and 5,865. LP has 11 timeouts and one hang. The separate 41-URL supplement is unchanged (LP 27 pages/577 names, Chrome 35/619); 39 withheld URLs have no pass credit. `polar-compare.log` and the `difference-polar-*` files record both directions and observation coverage.
