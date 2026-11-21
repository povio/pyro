# CSS animations on list-item markers

Studio compared `f3e1c767` to `2beb6602` on the same 8,082 URLs with six workers, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP `0.0.17`, and JSC `0e0dd7038a7a-6f4649e395e0`.

The change gained two strict pages and 42 passing names, all within the 156-URL marker focus. No strict pages, named passes, or observed names were lost. LP now passes 5,620 strict pages and 180,476 names. Chrome alone passes 1,603 pages and 8,355 names; LP alone passes 273 and 5,868. Marker transitions are not implemented by this change. LP retains 11 timeouts and one hang. The 41 supplemental URLs are unchanged (LP 27/577, Chrome 35/619); the other 39 URLs remain withheld and unscored.
# Marker UA defaults

With the same cohorts and pins, `a9dbef6e` gained two strict pages and 33 named passes over `f3e1c767`, with no losses or changes to observed names. All gains reproduced in the 156-URL focus: 32 assertions for default marker styles, one LP-only assertion for intrinsic size. LP passes 5,622 strict pages and 180,509 names. Chrome-only: 1,602 pages and 8,323 names; LP-only: 274 pages and 5,869 names. LP still has 11 timeouts and one hang. The 41-URL supplement is unchanged; the 39 withheld URLs remain unscored. Exact comparisons are in `difference-markerua-*`.
