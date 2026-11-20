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
