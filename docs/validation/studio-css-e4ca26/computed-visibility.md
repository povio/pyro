# Computed visibility preserves collapse

`e23c3f68` (visenum) versus `9412472a` (boxused), LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`. The rendering predicate intentionally groups hidden and collapse; CSSOM now serializes the actual computed value. The predicate itself is unchanged.

Over 8,082 matched URLs, +3 strict pages and +4 named passes: inheritance, computed visibility, Typed OM and SVG presentation attributes. The 56-URL focus includes visibility animation and checkVisibility controls. No prior page, pass or observed assertion disappeared. LP 5,748 pages / 181,622 names, Chrome 6,950 / 182,963. LP-only 281 / 5,901; Chrome-only 1,483 / 7,242. LP 11 timeouts and one hang; Chrome ten timeouts and one hang.

The 41-URL worker supplement is unchanged (LP 27 / 577, Chrome 35 / 619); 39 withheld URLs remain unscored. The visenum comparisons and outcomes preserve exact directional sets separately by cohort.
