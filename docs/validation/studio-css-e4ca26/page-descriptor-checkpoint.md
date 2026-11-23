# Page descriptor CSSOM

`02846648` against `f6579bf9` (pagemedia binary); WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, Chrome stable 158.0.8127.37, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, identical URLs, including the existing 346-URL pagemedia focus set. Three native page tests passed and the Stylo patch round-tripped.

The rejected intermediate build dropped eight assertions: three on `page-rule-declarations-000.html`, two on `page-rule-declarations-001.html`, and three on `margin-rules-002.html`. The corrected build passed all eight. New strict pages: `css/cssom/page-descriptors.html`, `css/css-page/parsing/size-valid.html`, and `css/css-page/parsing/page-orientation-computed.tentative.html`.

Main: 8,082 URLs, +3 strict pages and +48 named passes; focus +3/+44. No lost passes, missing names, or per-URL harness-status changes against the parent. The supplement (41 URLs) is unchanged: LP 27/577, Chrome 35/619. The 39 withheld URLs receive no credit. The comparison log and compressed outcome/difference sets preserve the failures and timeouts as well as the passes.
