# Inline module identity / base URL: a04f53cc9 evidence

- Source a04f53cc9b50e7a8752da6d5e0f0890452fe82a4 (tree 2eef0b699b25220f7468905bc13fb22d42c8dac5), base 538339506; candidate.diff is base..a04f53cc9
- Candidate round7-inline-module-a04f53cc9/pyro sha256 227f06fd645eb9dad07f5247c217a5780958153cbd77bc843b55fc279ed9f543; JSC jsc/0e0dd7038a7a-429e21cb3405 libJavaScriptCore.a 5f9afccd…2604 (patch 0038)
- Parent round7-editing/nested-list-conversion/base 2b6590ac…b663; parent runs base-ws.json / base-script.json reused from the earlier archive (round7-inline-module-evidence)
- WPT a421f2fe (wpt-full-b); bench/wpt/run.mjs at base; jobs 3. import-maps under wptserve (59 pages), script dirs static (486 pages)
- Chrome reference (WPT only): chrome158.json, extracted from the signed Chrome 158.0.8127.37 census receipts (round6-reference-158/census/raw, wptserve, 4 jobs); per-receipt sha256 inside. No Chrome 157 data and no direct Chrome fixture runs are used here.
- Controls (no browser reference): expected values are derived from the HTML/import-maps specs, not observed in Chrome. `python3 -I controls.py label=/bin ...` (same-origin server; parser/dynamic inline, base change, exact fragment scope, marker-like external fragment, plain external). controls-a04.json = parent vs candidate; controls-base-held.json = parent vs held 600d64ba8; controls-cand.json = held 0bf550fa7

## Publication

Raw result files are gzip -9 -n; SHA256SUMS hashes the uncompressed originals (`gunzip -k *.gz && shasum -a 256 -c SHA256SUMS`).
Counts here are owner-measured (broad: import-maps +203 named / 0 lost, script dirs +3 / 0 lost). Independent QA admission (owner 1f876e98, exact a04f53cc9) is reported separately.
