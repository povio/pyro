# CSSOM and CSS Values: combined Studio checkpoint

Chrome stable 158.0.8127.37, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, LP 0.0.17, JSC `0e0dd7038a7a-6f4649e395e0`. Six workers, wptserve, testdriver; worker globals only on the supplement. Both immutable candidates ran the same 8,082 main and 41 supplemental URLs. The 39 withheld URLs are unscored. The standalone combined focus took 4.16 s over 614 URLs; only its 607 main URLs enter the focus pass-set comparison. No out-of-cohort result is credited.

| Main measurement | Strict pages | Named passes | Observed names |
|---|---:|---:|---:|
| Last archived `mixmath` | 5,798 | 182,109 | 194,673 |
| Corrected CSSOM `4c2cb61a` | 5,803 | 182,123 | 194,673 |
| Integrated `b773a9ce` | 5,819 | 182,427 | 194,673 |
| Chrome | 6,950 | 182,963 | 194,912 |

The CSSOM changes gained five pages and 14 names without losses. Tokenizing declarations initially lost the CSSUnparsedValue roundtrip; escaping decoded identifiers recovered it in two matched repeats and the full suite. CSSOM executable SHA-256 `2068ca90e52ee23674787d718263572cbafaacb80796f101767b6ccc2ae3944e`; 487-page focus 3.53 s, main 346.26 s, supplement 0.29 s.

The combined binary (SHA-256 `bce7c5671d828eb4d2e4a72402bf853bdb0c18aaa34d04ac2b56643d2a5dca73`) gained 22 pages and 320 names and lost one page and two names since `mixmath`. No observed names changed. Of the 299 distinct original Chrome-only tuples from the CSS Values evidence, all 299 pass in the main run: 27 already passed on `mixmath` and the CSSOM parent, 272 newly pass. Chrome-only: 1,418 pages / 6,456 names; LP-only: 287 / 5,920. Main LP had 11 timeouts and one hang; Chrome had ten timeouts and one hang. Supplement unchanged: LP 27 pages / 577 names, Chrome 35 / 619. Both integrated vendor patches passed forward/reverse verification against `9a84bcad`. Build 68 s, main 345.9 s, supplement 0.3 s. Roughly 21 minutes from the previous evidence commit through regression repeats: about 909 net names and 60 net strict pages per elapsed Studio hour; excludes other-machine development time.

Both losses reproduced on each of two 29-URL runs on the CSSOM parent and combined candidate. `if-cycle.html`, case 4, expects an empty value; the combined branch returns `value`. The upstream topic documents its rationale; it remains a pinned loss. `viewport-units-scroll-no-cycles-001.tentative.html` loses its sole assertion and strict page: root overflow conditioned on a registered `100vw` tracker computes `hidden` instead of `visible`. This is a repeatable regression, not a timeout. Neither expectation was edited. Compressed repeat reports accompany the complete directional differences and candidate outcomes.
