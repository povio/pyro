# Logical size constraints in CSSOM

`fefe3c70` (axislimits) against `5c0a8569` (tryproto), WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`. Resolve logical size constraints on the element's writing-mode axis before CSSOM handles physical values; non-flex/grid items now report zero for automatic logical minimums just as for physical minimums.

103 focus URLs: inheritance and computed min-inline-size/min-block-size became strict passes, with no loss among logical sizing, flex and grid controls. The same +3 pages/+4 names were the only pass-set changes over 8,082 URLs. LP 5,752 / 181,696; Chrome 6,950 / 182,963; Chrome-only 1,479 / 7,168; LP-only 281 / 5,901. Observed names unchanged. LP 11 timeouts/one hang; Chrome ten timeouts/one hang. The separate 41 worker URLs had identical outcomes (LP 27 / 577, Chrome 35 / 619). The 39 withheld URLs are not scored.
