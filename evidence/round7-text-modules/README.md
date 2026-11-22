# Typed text modules on pinned JavaScriptCore

LP base 936e35acb, candidate de5bf15ab. Signed Chrome stable 158.0.8127.37 on macOS arm64. The selector was frozen before testing. It contains 25 windows, eight dedicated workers and eight shared workers across text modules, JSON modules and JavaScript charset controls. Other non-CSS cases and environments are outside this focused measurement.

| Engine | Named passes / observed | Strict pages / 41 | Page outcomes |
|---|---:|---:|---|
| LP base | 413 / 777 | 21 | 39 complete, one error, one timeout |
| LP candidate | 720 / 784 | 34 | 41 complete |
| Chrome stable | 724 / 784 | 38 | 41 complete |

Of the 307 gains, 300 previously failed and seven became observable and passed. All 307 also pass Chrome. All 413 prior passes were retained. The previous error on charset.html and timeout on charset-2.html became strict passes. Each ignore-content-type variant gained 86 names. Four Chrome-only names remain: the JSON repeated-import single-fetch assertion in all three globals and the window repeated-import MIME-mismatch retry assertion. Sixty names failed in both Chrome and the candidate. Both LP runs recorded zero restarts and watchdog kills.

The unchanged CI JSC archive supports a text-typed module key. The browser enables the import-text option by default, UTF-8 decodes the response, and exports the inert JSON-quoted string through the pinned typed loader. Text and JavaScript imports at one URL have separate cache entries. The network destination for text is text, Accept is text/plain,*/*;q=0.5, and CDP reports Script. An explicit JSC_useImportText=false disables the feature.

## Reproducing the measurements

The scripts use the round7-text-modules cache layout, including sibling round6-reference-158/census/runner, wptci/wpt-full and round6-inventory directories. Use the unchanged sources and manifests in provenance.json and chrome-identity.json. Build each exact source tree with the recorded CI JSC archive, profile and render feature. Place the baseline executable at base-pyro and candidate at pyro.

    python3 run.py base
    python3 run.py candidate
    node browser-probe.cjs base ./base-pyro
    node browser-probe.cjs candidate ./pyro
    node protocol-probe.cjs candidate ./pyro
    node order-probe.cjs override-off ./pyro off
    python3 import-reference.py
    python3 report.py

The Chrome WPT rows retain the original four-job signed census publication ee24e07f6, across 39 shards. import-reference.py joins named rows to each page and validates the recorded counts. Per-shard receipt hashes are included. Chrome restart and watchdog totals are unavailable for this slice. Direct Chrome probes were separately executed against the same stable executable. The pinned runner used wptserve, HTTP/2, testdriver, generated globals, a fresh page context and four jobs per invocation. Corpus holds were applied before freezing urls.json.

The native test command was cargo test --locked --offline --profile wptiter -p pyro-js --test integration runtime::typed_module_requests_reach_the_fetcher. The C bridge source and gate outputs demonstrate the default and explicitly enabled settings on the unmodified pinned archive.

named.jsonl.gz rows contain URL, original subtest name, occurrence (1-based), then baseline, candidate and Chrome status. Null means unobserved. pages.json rows contain URL and each engine's outcome, harness status and strict-pass flag. A strict pass requires a completed page, OK harness, and at least one subtest with every name passing.

## Independent verification

Independent source review covered 83c44d5e8. The subsequent one-line CDP match arm at de5bf15ab was checked against signed Chrome network events. Independent runtime QA repeated the candidate WPT run, CDP/cache order, wire-header and feature-override probes using the same binary digest and original baseline/reference receipts. All 784 named rows and all 41 page rows matched the implementation run exactly. The separate candidate receipt digest is in qa/summary.json.
