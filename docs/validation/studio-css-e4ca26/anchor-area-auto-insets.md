# Automatic insets and margins inside a position area

Implementation `eaa93142`, parent candidate `nestedit` (`b59bd97a`). Same Studio Chrome stable 158.0.8127.37, LP 0.0.17 / JSC `0e0dd7038a7a-6f4649e395e0`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, six workers, wptserve and testdriver. CSS Anchor Positioning's position-area sizing uses zero auto insets and margins in a resolved area. Preserve the single-specified-inset default alignment path and the ordinary layout path when no anchor resolves. The native control compares resolved and missing anchors; all 35 anchor-positioning tests passed. The regenerated Taffy patch passed the vendor round-trip check.

| Cohort | Parent strict pages / names | Candidate | Chrome stable |
|---|---:|---:|---:|
| Main (8,082 URLs) | 5,788 / 181,863 | 5,791 / 181,960 | 6,950 / 182,963 |
| Focus (336, subset of main) | 230 / 14,409 | 233 / 14,506 | 319 / 14,708 |
| Supplement (41 URLs) | 27 / 577 | 27 / 577 | 35 / 619 |

The three newly strict pages are `position-area-basic.html` (+45 names), `position-area-wm-dir.html` (+44), and `position-area-with-insets.html` (+1). Seven other named gains span five pages. The focus and full suite agree: 97 named gains, no lost pages or names. No change to observed names. Chrome-only 1,446 pages / 6,909 names; LP-only 287 / 5,906. LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. The supplement remains separate, and the 39 withheld URLs are unscored. Exact comparisons and outcomes are archived under `areainset`.
