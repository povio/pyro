# Bounded combined replay on b62d66442

LP 97a4b379d2eb2dd0f0a3f64e38126b5cded5f098 integrates 26 reviewed commits on b62d66442df323c3e294630e4832eb17d1802c29. Its production replay, the stacked fragment parser and Navigator changes, and the DOMParser delta retained their original ordered patch IDs. All changed source blobs match their reviewed source. The five later CSS source changes use disjoint paths and retain the exact upstream blobs. The branch has the pinned JSC and production Wasm backend.

Across 458 freshly measured URLs, LP gained 4,289 named passes and lost 85. Fully passing pages changed from 273 to 279. All selected URLs have a page receipt from each LP binary.

| Cohort | URLs | Base → head named passes | Named gains / losses | Base → head full pages |
| --- | ---: | ---: | ---: | ---: |
| focus | 32 | 62 → 172 | 110 / 0 | 5 → 7 |
| guard | 33 | 778 → 779 | 1 / 0 | 27 → 27 |
| navigator | 20 | 34 → 279 | 245 / 0 | 5 → 7 |
| production | 58 | 5,340 → 9,188 | 3,932 / 84 | 11 → 14 |
| css | 315 | 3,966 → 3,966 | 1 / 1 | 225 → 224 |

The two CSS name changes were the transition generation start time (Fail → Pass) and the offscreen readiness time (Pass → Fail). The latter also lost a strictly passing page. The candidate matched the Studio LP flexcrossbound page and named pass sets on all 315 matched CSS guards. Both timing assertions varied in earlier Studio repeats, and neither observation has been discarded. On the 58 selected production URLs, 63 of 84 named losses belonged to referrer-policy, seven to Web Audio, six to subresource-integrity, five to input-events, two to HTML and one to UI events. Among these, 25 named losses repeated from the 4,929-URL guard. The benchmark preserves each status and missing name in base-to-head-changes.jsonl.gz. The parser and Navigator named and full-page pass sets matched their individual reviewed precursors on all 85 URLs.

## Comparator coverage

| Cohort and comparator | Paired URLs | Head-only / comparator-only names | Head-only / comparator-only strict pages |
| --- | ---: | ---: | ---: |
| focus, prior-local | 32 | 0 / 0 | 0 / 0 |
| focus, chrome-local | 32 | 0 / 303 | 0 / 23 |
| guard, prior-local | 33 | 0 / 0 | 0 / 0 |
| guard, chrome-local | 33 | 5 / 14 | 1 / 5 |
| navigator, prior-local | 20 | 0 / 0 | 0 / 0 |
| navigator, chrome-local | 20 | 0 / 39 | 0 / 5 |
| production, prior-local | 58 | 69 / 70 | 7 / 1 |
| production, chrome-local | 58 | 1,332 / 2,753 | 0 / 29 |
| css, chrome-local | 8 | 0 / 2 | 0 / 2 |
| css, studio-lp | 315 | 0 / 0 | 0 / 0 |
| css, chrome-studio | 315 | 52 / 237 | 6 / 54 |

Local Chrome stable and Studio Chrome stable are 158.0.8127.37 on WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Per-machine columns retain separate original page and subtest statuses, raw assertion names, occurrence indices and missing cases. Studio LP uses the published flexcrossbound outcomes from b62d66442. The previous local comparison uses 93117170d for the 65 parser URLs, bab023c81 for Navigator and 8e7144b7e for the selected production guards. A null column means no measurement was used. Local base and head use matching worker counts, two per shard except the four-worker Navigator batch. The earlier production controls and local Chrome census used four workers. Studio browser profiles and worker counts differ from local runs.

## Selection and prior evidence

The 32 focus and 33 guard URLs preserve both previously verified parser inventories. The 20 Navigator URLs preserve its earlier matched guard. The 58 production URLs cover the prior observed named losses and up to two largest named-gain URLs per non-CSS directory, after deduplicating the feature selections. Of the 317 unique URLs in the Studio sizing and timing control lists, 315 were measured. Two CSS URLs belonged to interrupted shard css-full-067 with ambiguous execution, and remain withheld in holds.json. The recorded Chrome WebSocket hang was absent from the plan. The earlier complete frozen non-CSS census and 4,929-URL integration guard retain their original runtime provenance.

The local base and candidate completed the six ordinary parser lifecycle checks recorded in controls-base.json and controls-head.json. The earlier Chrome and reviewed DOMParser results are retained in controls-chrome-original.json and controls-prior.json. The earlier independently measured checkpoints are listed below.

- [Full frozen Chrome 158 census and cbdeb1193 comparison](https://github.com/povio/pyro/tree/ee24e07f6b15b399d6e8e2157c763809341b6e44/evidence/round6-chrome158)
- [Fresh 4,929-URL guard on 8e7144b7e](https://github.com/povio/pyro/tree/14497626454a63994536f6314b088f6e7e5784c1/evidence/round6-noncss-integration-52d8b7c)
- [Stacked fragment parser and Navigator](https://github.com/povio/pyro/tree/05599687f3cbfa613aca16d3a29a3b1c697a9208/evidence/round6-template-navigator-combined)
- [Inert DOMParser source and measurements](https://github.com/povio/pyro/tree/44867401f66428823d8774448ba96fa991c6f022/evidence/round6-inert-document-markers)

## Reproduction

Build each exact Git tree with the round6 build-exact wrapper, a separate Cargo target directory for each binary and the JSC archive identified in source.json. Run plan.py and run.py in the cache layout used by the earlier receipts, followed by audit.py, export.py and verify.py. The frozen selections and runner options are included. To verify the published data independently, run `shasum -a 256 -c SHA256SUMS` and `python3 verify.py .`. Each named row starts with the URL, canonical assertion name and one-based occurrence followed by the browser columns listed in columns.json. Page columns record page status, harness status, strict pass, passing names and reported names. A page passes strictly when the harness completes successfully with at least one subtest, all passing. Timed-out pages retain named observations.
