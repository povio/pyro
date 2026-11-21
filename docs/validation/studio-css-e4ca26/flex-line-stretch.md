# Flex stretch bounds: matched Studio checkpoint

Sources: `5775c48f` caps the automatic main minimum; `2d55e9c8` applies a stretch cross minimum or maximum against the used flex line, respecting a fixed minimum. This does not establish correctness for all multiline layouts.

Chrome stable 158.0.8127.37; WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603; LP 0.0.17 on JSC 0e0dd7038a7a-6f4649e395e0. Main 8,082 URLs, six jobs. Directional sets are `difference-*-vs-*.json.gz` (left is newer); `*-parent` refers to the integrated `floatboth` baseline, not necessarily the immediate source parent.

| Candidate vs previous binary | Strict gained/lost | Named gained/lost |
|---|---:|---:|
| flexsuggest vs inlinecross | 0/0 | 1/1 |
| flexcrossbound vs flexsuggest | 1/1 | 15/1 |
| flexcrossbound vs inlinecross | 1/1 | 15/1 |

The new strict pass is `css/css-sizing/keyword-sizes-on-flex-item-001.html`. The lost strict page is `css/css-animations/transition-ready-time-offscreen.html`. On the 314 sizing control URLs the line change gained one strict page and 14 names, with no losses. Main totals: LP 5,555 strict / 178,660 named; Chrome 6,950 / 182,963. Chrome-only 1,667 / 10,105; LP-only 272 / 5,802. Observed names 194,540 / 194,912; timeouts 11 / 10; one hang each.

The stage-one named loss was the transition generation start time; the stage-two loss was offscreen transition readiness. Three matched 6-job repeats per binary on the same control URLs passed both names 3/3 on inlinecross and flexsuggest. On flexcrossbound the generation assertion passed 2/3 and offscreen readiness 1/3. These are unresolved intermittent candidate losses, not recovered passes; all repeat outcomes are retained.

Supplement (41 URLs): unchanged at LP 27 strict / 577 named vs Chrome 35 / 619, 874 observed names each. The remaining 39 withheld URLs are unscored, not passes. No expectations or corpus entries changed.
