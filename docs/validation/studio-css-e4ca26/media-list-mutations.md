# CSSOM MediaList mutations

Implementation `d5cb5a3c`, parent `5340075b8` (`areainset`). Studio Chrome stable 158.0.8127.37; WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603; LP 0.0.17 with JSC 0e0dd7038a7a-6f4649e395e0. Six workers, wptserve and testdriver; worker globals enabled only for the supplement.

| Cohort | Parent strict pages / named passes | Candidate | Chrome |
|---|---:|---:|---:|
| Main (8,082) | 5,791 / 181,960 | 5,792 / 181,962 | 6,950 / 182,963 |
| CSSOM media focus (13, inside main) | 11 / 60 | 12 / 62 | 13 / 63 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

Both gains are on `medialist-appendmedium-parse-single-and-dedup.html`: a comma-separated argument leaves the empty and populated list unchanged. No strict or named losses and no changed observed names. Native rule-backed controls passed for duplicate suppression, deleting a list argument, removing all equal queries and throwing for a missing query. The open focus failure is import-rule media identity, a different issue. Main Chrome-only 1,445 pages / 6,907 names; LP-only 287 / 5,906. LP had 11 runner timeouts and one hang, Chrome ten and one. The 39 withheld URLs are not scored. Exact differences and candidate outcomes use the `mqitems` label.
