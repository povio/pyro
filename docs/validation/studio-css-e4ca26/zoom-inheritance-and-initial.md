# Zoomed lengths: inheritance and initial values

Studio, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable `158.0.8127.37`, LP `0.0.17`, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, the same 30 URLs (`zoomtypes-urls.json`), same runner and testdriver settings. Chrome's pinned main reference is restricted to these URLs. Strict pages / passing names / observed names:

| Run | Strict pages | Passing names | Observed names |
|---|---:|---:|---:|
| Marker UA parent | 19 | 459 | 580 |
| Composite inheritance (`zoomcomp`) | 20 | 516 | 580 |
| Initial values (`zoominit`) | 24 | 561 | 580 |
| Chrome stable | 25 | 578 | 597 |

`explicit-inherit/all-length-properties.html` changed from 97/154 to 154/154 (Chrome 168/168). The WPT embeds the child's computed value in the name: 57 previously failing names disappear and 57 corrected names appear. No passing names disappear. The initial-value change takes `computed-initial.html` from 171/207 to 207/207 (Chrome 210/210). It gains four strict pages and 45 named passes in focus without losing any passes or observations. Different counts of generated assertions are not treated as equivalent coverage. `difference-*-direct-*` and `outcomes-*-direct-*` retain exact sets and outcomes. These are focus results, not full-corpus totals; 41 supplemental and 39 withheld URLs are outside this focus.
## Full comparison

The 8,082-URL main runs reproduced the focused gains exactly: `af1d46ad` gained 1 strict page and 57 passing names; `e8e746cb` gained another 4 pages and 45 names, without losing any passing names or strict pages. The first change replaced the 57 previously failing dynamically generated names described above; the second did not change the observed set. LP now passes 5,627 pages / 180,611 names versus Chrome 6,950 / 182,963. Chrome alone passes 1,597 / 8,227; LP alone passes 274 / 5,875. LP had 11 timeouts and one hang, Chrome 10 timeouts and one hang. The separate 41-URL supplement stayed at LP 27 / 577 versus Chrome 35 / 619 (874 observed on each). The 39 withheld URLs are not counted. The full pass sets, observation differences, and completion states accompany the `zoomcomp-compare.log` and `zoominit-compare.log` reports.
