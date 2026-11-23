# Used width and height of flex items

Implementation `76991fe7`, parent `mqitems`. The resolved width/height of a rendered flex item uses its layout result even when its preferred dimension was specified in pixels. The native test covers a row with a display:contents intermediary, a border-box item in a column, and an ordinary block as a negative control.

| Matched cohort | Parent pages / names | Candidate | Chrome stable |
|---|---:|---:|---:|
| Main (8,082) | 5,792 / 181,962 | 5,792 / 181,987 | 6,950 / 182,963 |
| Focus (418, inside main) | 328 / 7,962 | 328 / 7,987 | 409 / 8,981 |
| Supplement (41) | 27 / 577 | 27 / 577 | 35 / 619 |

The row flex-basis page gains 13 passing names and the column page gains 12; neither is fully passing. No lost pages or names, changed observed names, or status changes. Main LP-only 287 pages / 5,906 names; Chrome-only 1,445 / 6,882. Matched WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, Chrome stable 158.0.8127.37, LP 0.0.17 / JSC 0e0dd7038a7a-6f4649e395e0. Same six-worker settings as `mqitems`; 39 withheld remain unscored. See `flexused` archives for exact sets.
