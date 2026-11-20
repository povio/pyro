# Round 6 non-CSS testharness census

The pinned WPT manifest contains **31,827** non-CSS testharness URL variants. This census preserves every URL, environment, source blob, named case and duplicate occurrence. Its final export is dated 2026-11-20T20:49:45.432801+00:00.

| Browser and source | Reported URLs | Fully passing URLs | Named passes | Unobserved URLs |
| --- | --- | --- | --- | --- |
| LP e4ca26f48 | 31,054 | 14,699 | 1,663,290 | 773 |
| LP 4c13e694a | 30,516 | 14,735 | 1,663,418 | 1,311 |
| Local Chrome stable (WPT profile) | 31,054 | 24,925 | 1,946,896 | 773 |
| Public Chrome stable | 31,826 | 26,857 | 1,959,117 | 1 |
| Public Chrome canary | 31,827 | 28,164 | 1,965,712 | 0 |

A reported URL can have a timeout, error, hang or crash. A fully passing URL requires an OK harness, at least one named result and all reported subtests passing. A timeout retains any named results observed before it. A case not reported by one side remains paired with null rather than disappearing.

## Comparisons on the same observed URLs

| Left → right | Paired URLs | Named left only | Named common | Named right only | Full pages left only | Full pages right only |
| --- | --- | --- | --- | --- | --- | --- |
| LP e4ca26f48 → LP 4c13e694a | 30,516 | 292 | 1,616,529 | 46,889 | 46 | 175 |
| Local Chrome stable (WPT profile) → LP 4c13e694a | 30,516 | 101,989 | 1,653,091 | 10,327 | 10,288 | 544 |
| Public Chrome stable → LP 4c13e694a | 30,515 | 108,118 | 1,653,473 | 9,943 | 11,753 | 523 |
| Public Chrome canary → LP 4c13e694a | 30,516 | 113,319 | 1,654,594 | 8,824 | 12,842 | 475 |
| Local Chrome stable (WPT profile) → LP e4ca26f48 | 31,054 | 291,650 | 1,655,246 | 8,044 | 10,742 | 516 |

The named comparisons above canonicalize encoded WPT host ports only when the producing server configuration authenticates the mapping. No test expectation was changed. The complete original-name comparisons, per-page port changes and any collisions are also included. Duplicate names are indexed in the order the harness reported them.

## Coverage and provenance

The runner classifies 31,358 official URLs as eligible. The 469 inventory-only URLs comprise 209 requiring BiDi, 89 requiring PAC, 78 requiring a JS shell, six requiring extension automation, 43 explicit crash or vulnerability fixtures, 24 remaining historical gate exclusions, and 20 raw HTTP/1 parser URLs for which the mapping proxy cannot preserve origin response bytes. A separate path/title/specification review retained 304 additional eligible URLs outside the ordinary execution scope. No known vulnerability was reproduced to obtain coverage.

The clean base and corrected local Chrome measured all 31,054 ordinary URLs. The later LP checkpoint measured 30,516 and withheld the 538 Wasm browser URLs from replay after earlier exact-JSC runs recorded native faults. Those URLs are still counted in the official denominator. Their earlier LP results are a separate column. Of the base results, 3,988 came from already-completed runs of the exact binary on compatible protocol, timeout, source and viewport settings; the source index records the original receipts.

The local stable reference is Chrome 157.0.8103.80 on macOS 27.2.1 arm64. It uses the pinned ordinary WPT popup, certificate, media autoplay, viewport and WebNN settings, including normal GPU initialization. The local CDP/testdriver adapter uses fresh incognito-like contexts; it is not identical to upstream WebDriver. Hardware/device service and browser-driver differences can change outcomes. The earlier local diagnostic profile blocked unactivated popups and disabled GPU initialization. Its results are kept separately.

Public Chrome stable 157.0.8103.38 on Linux 22.04 is ten WPT commits before a421f2fea5eb7f91e0db399e44b0fb0779ee1603. One pinned URL was unreported; 19 pinned sources or URLs differed or were unavailable. Even an unchanged test source can use changed support resources. Chrome canary 159.0.8143.0 provides the exact-revision independent reference. The page index records source matches. [Stable run](https://wpt.fyi/results/?run_id=5478394868006912) · [Exact-revision canary run](https://wpt.fyi/results/?run_id=4907012216061952)

Both LP browser builds use the CI arm64 JavaScriptCore archive 0e0dd7038a7a-6f4649e395e0. Its static library SHA-256 is 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. WPT source bytes for the full local selection were verified against the pin. Benchmark and census evidence stay tied to their measured heads; later topic branches have their own targeted guards.

## Main gaps

| Area | Paired URLs | Chrome-only named passes | LP-only named passes | Chrome full pages | LP full pages |
| --- | --- | --- | --- | --- | --- |
| html | 9,062 | 15,246 | 1,366 | 7,913 | 5,492 |
| editing | 710 | 15,185 | 1,891 | 426 | 120 |
| webnn | 610 | 13,093 | 0 | 549 | 1 |
| webgl | 22 | 4,248 | 0 | 17 | 0 |
| webcodecs | 283 | 3,666 | 0 | 259 | 1 |
| webaudio | 348 | 3,647 | 80 | 316 | 62 |
| WebCryptoAPI | 334 | 6,395 | 3,016 | 185 | 220 |
| mathml | 216 | 3,040 | 4 | 181 | 26 |
| svg | 787 | 2,514 | 60 | 669 | 258 |
| fetch | 918 | 2,490 | 293 | 715 | 446 |

The archived 538-URL Wasm comparison and withdrawn JSC candidates are separate from this later-head ranking. The isolated Wasmi experiments did not alter the shipped backend. Their evidence package includes the dispatch comparison, complete selected-WPT results, exact source and binary hashes, controls and application workloads. [Wasm decision and evidence](https://github.com/povio/pyro/tree/experiment/round6-wasmi-evidence/evidence/round6-wasm-evaluation)

## Verified integration topics

The complete LP-to-LP comparison is e4ca26f48 against 4c13e694a. The latest integrated ordinary branch extends it with separately measured prioritized task scheduling, SVG reflection, Biquad/IIR audio, wheel dispatch, form-context parsing and editing boundary fixes. The later 120-page editing guard gained 167 named passes without loss, its 34-page Selection guard was unchanged, and the wider 12-page editing guard gained 836. The latest srcdoc referrer fix passed 27 ordinary header checks and five existing WPT pages. No local full-suite score is attributed to the later branch. [Latest ordinary integration branch](https://github.com/povio/pyro/tree/round6/noncss-measured-batch4) · [Full-census browser branch](https://github.com/povio/pyro/tree/round6/noncss-measured-batch2)

The Wasmi sources are confined to two isolated experimental branches; neither was merged into the ordinary browser work. The optimized interpreter lacks sufficient JS interoperability and broad WPT parity for a production switch. [Portable experiment](https://github.com/povio/pyro/tree/experiment/round6-wasmi) · [Optimized experiment](https://github.com/povio/pyro/tree/experiment/round6-wasmi-auto) · [Measured tradeoffs and reproducible workloads](https://github.com/povio/pyro/tree/experiment/round6-wasmi-evidence/evidence/round6-wasm-evaluation)

## Files and commands

The pages file includes harness outcomes, masks for missing execution, source hashes and result provenance. The names file contains one row per URL, exact name and one-based occurrence; column order is in summary.json. The replacement-page file and port-change trace give the audited comparison without discarding the original names. The fully-passing-pages file enumerates each browser’s exact page pass set.

Run the repository harness from the verified runner branch with the pinned manifest and WPT checkout. List mode emits all 31,827 official records. An execution must also specify a URL selection. The census used four workers per browser, fresh contexts, 800×600 at DPR 1, a 15-second default timeout, and 45 seconds for long tests. The URL argument selects a predeclared, eligible ordinary shard. The current LP selection omits the separately recorded native-fault area. The archive retains the full plan, withheld URLs, and original receipts for audit. Use ordinary controls for any fresh run. [Runner branch](https://github.com/povio/pyro/tree/topic/round6-reviewed-testharness-census)

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --dirs non-css \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --list --out inventory.json
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --dirs non-css \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --tests "$URLS" \
  --jobs 4 --bin "lp=$LP_BINARY" --out results.json
```

## Wasm bound and driver recoveries

With the final observed name union, keeping the measured 145,314 public-stable-only Wasm passes unavailable caps LP at 1,864,235 named passes. Even if every other observed name passed, this would leave a 94,882-case shortfall. The earlier 96,255 estimate used a smaller set of local observations. This is a conditional bound; additional dynamically generated names unobserved by all measured engines are unknown.

The default Playwright controller was 1.59.1. Three corrected-profile Chrome shards encountered driver-level errors during target attachment or dialog dismissal. Their first-observed rows were preserved, and the 220 missing URLs were measured with unmodified Playwright 1.65.1 on the same Chrome executable. The page table records the exact controller on those results. Long-running shards split around benchmark windows preserve disjoint original page receipts. Driver failures were not converted into passing assertions.

## Pass-set encoding

The names table contains arrays of URL, original name, one-based occurrence and one result per column in summary.json. Null means the occurrence was not reported by that side. PASS values give the exact named pass set. Audited replacement rows apply only to the URLs present in that table. The original-name rows remain available.

This bundle contains 2,010,179 occurrence rows. Ordinary measured test names are preserved exactly. For 198,981 rows from inventory-only, scope-held, or runtime-withheld URLs (or names containing local host identifiers), the name field is a deterministic SHA-256 of the original name. This preserves identity and result counts without redistributing out-of-scope fixture text. The local original export remains intact.

## Cross-machine evidence

The compressed archive contains the full-page table, pass-set comparison, complete occurrence matrix, source hashes, and validation checks. [Download the evidence bundle](noncss-census-evidence.tar.gz)

The archive SHA-256 is `22b711e03677a8dc8666dbbb0c9e7868df0a820117d8571f8992c1ac6a37ebf6`. SHA256SUMS verifies individual published files. The underlying ordinary browser topics and experimental backend evaluations have separate histories; this evidence branch contains no production backend change.

The measured macOS Chrome launcher, exact-occurrence comparison utility and audited WPT endpoint mapping are also provided alongside the archive. Set WPT_CHECKOUT to the pinned WPT checkout and WPT_CHROME_STATE_DIR to an isolated writable directory when using the launcher. It accepts the runner's --port argument and creates an independent test profile. The published source manifest is available from the wpt.fyi manifest API at the full revision. [Chrome launcher](chrome-wpt-profile.py) · [Comparison utility](compare-named.py) · [Endpoint normalizer](wpt_ports.py) · [Upstream source manifest](https://wpt.fyi/api/manifest?sha=a421f2fea5eb7f91e0db399e44b0fb0779ee1603)
