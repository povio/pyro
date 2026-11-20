# CSSOM offsets from the parent padding edge

The full vertical range change (`6d064e18`) gained 5 strict pages and 508 named passes, with no losses, on the original 8,082 URLs: 5,124 pages / 165,057 names. Chrome stable 158.0.8127.37 passed 6,950 / 182,963. Exact full sets are in `outcomes-range-full` and `difference-range-full-vs-*`.

On the subsequent padding-edge fix, both LP binaries ran the same 1,625 CSS Grid, Flexbox, Alignment and CSSOM View URLs with six workers. The Chrome rows come from the matching pinned stable reference. All three observed the same 28,732 named assertions.

| Engine | Strict pages | Named passes | Harness OK / error / timeout |
|---|---:|---:|---:|
| LP before | 1,027 | 19,289 | 1,623 / 1 / 1 |
| LP after | 1,215 | 25,506 | 1,623 / 1 / 1 |
| Chrome stable | 1,434 | 26,725 | 1,625 / 0 / 0 |

Gains/losses: +188/-0 pages and +6,224/-7 named passes. All seven losses concern baseline alignment in nested flex/grid containers; Chrome passes each. For example the child in `flex-align-baseline-grid-002.html` expects offsetLeft 110, now reports 107. Do not discard these as timing fluctuations. Exact names and outcomes are archived in `difference-offset-vs-before` and `outcomes-offset-*`. The asymmetric-border regression passes. The broader Rust layout integration suite has 240 passes, 4 failures and 2 ignored; attribution of the hit-test/deep-nesting failures has not been established.

`display-lock-repeats.json.gz` archives the earlier clock investigation: two six-worker predecessor runs reach the last assertion, two clock runs time out in the first test and never execute the last assertion. The predecessor fails an earlier opacity assertion instead. This is not evidence that the last assertion became flaky.

The supplemental 41 URLs remain separate: previously LP 27 strict pages / 436 names versus Chrome 35 / 619. The 39 excluded URLs are not counted; runner testdriver and HTTP/2 limitations still apply.

Full `23a2ec14` result: 5,346 strict pages / 171,743 named passes, up 222/-0 pages and 6,697/-11 names from range geometry. All 11 lost passes are on baseline alignment pages. LP observed 194,309 distinct names, with 7,864 OK, 101 error and 105 timeout harness completions, 11 runner timeouts and one hang. Against Chrome: 237 LP-only / 1,841 Chrome-only pages; 5,373 / 16,593 exclusive named passes. Largest remaining stable-only counts by directory: css-values 3,430, css-fonts 1,283, css-gaps 1,269, css-transforms 910. Directly rerunning the separate 41 URLs with the padding binary and `--worker-globals` confirmed 27 / 436, all harnesses OK. Combined main plus supplement: LP 5,373 / 172,179; Chrome 6,985 / 183,582. No estimate or imputation for the other 39.
