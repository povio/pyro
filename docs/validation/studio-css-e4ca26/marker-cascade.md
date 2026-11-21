# CSS animations on list-item markers

Studio compared `f3e1c767` to `2beb6602` on the same 8,082 URLs with six workers, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP `0.0.17`, and JSC `0e0dd7038a7a-6f4649e395e0`.

The change gained two strict pages and 42 passing names, all within the 156-URL marker focus. No strict pages, named passes, or observed names were lost. LP now passes 5,620 strict pages and 180,476 names. Chrome alone passes 1,603 pages and 8,355 names; LP alone passes 273 and 5,868. Marker transitions are not implemented by this change. LP retains 11 timeouts and one hang. The 41 supplemental URLs are unchanged (LP 27/577, Chrome 35/619); the other 39 URLs remain withheld and unscored.
