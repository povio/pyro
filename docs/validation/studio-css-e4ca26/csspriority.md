# CSSOM setProperty priority

`c8c26dfd` handles null priority like the empty string, and removes a declaration for an empty value before validating the converted priority. A native guard exercises null, invalid priority with a nonempty value, and invalid priority with an empty value. The guard and matched WPT passed.

Compared with `cssomvalues`, one assertion and its strict page (`css/cssom/setproperty-null-undefined.html`) gained a pass. No page, name, or observation was lost. Main LP: 5,820 pages / 182,428 names; stable Chrome: 6,950 / 182,963. LP-only: 287 / 5,920; Chrome-only: 1,417 / 6,455. The preceding two CSS Values losses remain outstanding. The 41 supplemental URLs had identical LP outcomes (27 / 577), and 39 withheld URLs remain unscored. Same WPT, Chrome, JSC and cohort configuration as `cssomvalues.md`.

Binary SHA-256 `ba6d319580766dc556da9c052a2a95b7638f6f6418e9c6d8f95e54e9e893f3c6`; build 21 s, focused run 3.9 s, main 344.9 s, supplement 0.3 s. From `mixmath` through this comparison, 22 net strict pages and 319 net names in about 26 elapsed minutes on Studio (~51 pages and 736 names per elapsed hour, excluding parallel development on other machines).
