# Offline audit of 106 round-seven non-CSS named losses

All 106 occurrences passed on LP 8e7144b7e and lost that pass on LP 1f1be0324. The signed Chrome 158.0.8127.37 reference passed 86, failed 14 and had no matching name for six. The 21 URLs were originally measured on WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The audit joins the frozen comparison to the underlying three browser receipts by URL, original name and one-based normalized occurrence. Every raw result and server log was hash-checked against published provenance. No browser was launched for this audit. [Frozen census](https://github.com/povio/pyro/tree/01f49f43da24a20ecb20d99c967fc5de54272120/evidence/round7-noncss-census-1f1be0324)

| Round-seven observation | Occurrences | Direct evidence |
|---|---:|---|
| Page timeout with occurrence missing | 50 | The Fetch Metadata window-location variant yielded 21 passing names in 45,008 ms. The earlier LP reported 72 and passed 70. That accounts for 49 lost names. A separate form-submit/window-stop timeout accounts for one. The underlying causes remain unverified. |
| Proxy connection failed before test results | 18 | Seventeen User Timing names and one WebSocket constructor name came from the same shard. Both pages report ERR_PROXY_CONNECTION_FAILED against the same local WPT HTTP endpoint and zero subtests. |
| CDP local address unavailable | 2 | The XHR worker page records connectOverCDP EADDRNOTAVAIL and zero names. |
| Fixture load returned HTTP 502 | 4 | The html5lib isindex page reached complete/OK with a single failing replacement harness name reporting “fetch isindex.dat: 502”. Each of the four parser assertions was absent. |
| Live signature embedded in test name | 6 | The pinned WPT signs the runtime authority and inserts the signature into each name. The original six names were absent on both newer LP and Chrome. With the live signature replaced for diagnostic grouping, each corresponding group contains two passing cases on all three engines. All six original losses remain in the strict accounting. |
| Observed named Fail | 24 | Thirteen plaintext formatting assertions and one Fetch 204-versus-200 assertion also failed on Chrome. Ten failures pass Chrome. These cover four moved-script events, two source-image decode rejections, a transparent canvas pixel, a 0.1 ms timeline difference, a blocked-versus-upgraded request and a resource count of two versus one. |
| Observed named Timeout | 2 | Two wide-gamut ImageBitmap cases timed out. Both pages report an unhandled source-image decode rejection, and each had harness error/Error on both LP heads. |

The first five groups account for 80 missing round-seven occurrences. Across all 106, 50 were on pages transitioning from complete/OK to timeout, 20 on pages transitioning from complete/OK to error, 34 on pages retaining complete/OK and two on pages retaining error/Error. The diagnostics establish the observed termination or assertion. They do not identify a specific production commit as causal.

## Later measurements

The offline search indexed 232 local round-seven WPT result files as of 2026-11-21 20:31:50 UTC. For matching URLs it required the pinned WPT revision, an exact LP source tree, a matching binary SHA-256, pinned JSC provenance and an original named occurrence. The receipt index retains server, worker-count and testdriver settings per observation. The withdrawn cached attribution of binary 543a7b85 to e1a29d483 was excluded. The separate verified rebuild c299b087 remains identifiable.

Twelve nested plaintext formatting occurrences had 408 later observations across 34 receipts and 11 exact source commits, including seven independent QA receipts. Every observation failed. Those twelve also failed in the official Chrome reference. The 94 remaining occurrences have no qualifying later observation in the scanned receipts. The two later-published whole-suite CI snapshots available during the audit measured 30aa3adf57 and 598ed3f30a. Both source commits predate the frozen round-seven head and do not contain it. Source identities and reachability at the audit cutoff are recorded separately for published branches and locally verified candidates.

## Files

- losses.json keeps the exact URL, canonical name, occurrence, original named outcomes and diagnostics for all three engines, and all later matches for that occurrence.
- page-observations.json retains the full-page flag, harness state, timing, reported counts and errors for every affected page and browser.
- later-index.json inventories source identities, binary hashes, receipt hashes, profiles and matching occurrences. signature-diagnostics.json preserves the runtime-signature examples without changing acceptance keys.
- Use python3 verify.py for a standalone readback. The original offline commands were python3 audit.py, python3 cross.py and python3 verify.py. The audit scripts require the local receipt archives for regeneration.
