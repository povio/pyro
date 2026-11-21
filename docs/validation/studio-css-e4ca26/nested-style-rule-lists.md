# Nested style rules and style block replacement

Matched against `shortgram` at integrated `faeb008f0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Studio Chrome stable `158.0.8127.37`, LP 0.0.17 / JSC `0e0dd7038a7a-6f4649e395e0`, six workers. Compared sets of (URL, name) and strict pages, not only totals. The focus includes nesting, CSSOM rule/sheet operations, scope, page descriptors and (in the final run) whole-value variable blocks.

| Main cohort (8,082) | Strict pages | Named passes | Compared to |
|---|---:|---:|---|
| shortgram | 5,770 | 181,740 | parent |
| nestlist `81b3c8a8` | 5,783 | 181,851 | +13/−0 pages; +113/−2 names vs shortgram |
| stylegen `41b8661f` | 5,785 | 181,859 | +2/−0 pages; +8/−0 names vs nestlist |
| nestblock `4bab0afb` | 5,786 | 181,861 | +16/−0 pages; +121/−0 names vs shortgram |
| Chrome stable | 6,950 | 182,963 | reference |

Style rules expose a live list of children and keep the leading declarations separate from subsequent declaration runs. Editing a style element's sheet now overrides the DOM text only for that style block generation. Replacing the element's child text takes effect without another CSSOM sheet lookup. The first implementation misclassified the two whole-value `var()` braced declarations as nested selectors. Both recovered in the expanded 158-URL focus and final main cohort. All 16 CSSOM parser tests and three targeted native API controls passed.

Final main Chrome-only: 1,451 pages / 7,007 named passes; LP-only: 287 / 5,905. LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. The 41-URL supplement was unchanged throughout (LP 27 pages / 577 names, Chrome 35 / 619); 39 withheld URLs are not scored. Outcome archives and directional comparisons for each candidate preserve pass sets and differences in observed names.
