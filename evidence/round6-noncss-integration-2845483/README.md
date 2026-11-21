# Round 6 non-CSS integration

The verified LP replay is 4bf8195b4d87ad340501bb564ed2cea29b06d75f on 2845483ec2f000509dd999b20646a853320fa753. Both binaries were freshly measured on 4,929 identical upstream WPT URLs in 58 disjoint shards. The comparison includes the original 4,041 priority/calibration controls, 430 feature supplements, 406 second-wave controls, 40 upstream CSS controls, and five storage plus seven metadata variants from separate reviewed overlays. The full non-CSS inventory has 31,827 URLs, with 30,359 in the frozen ordinary run. The overlays keep their original exclusion reasons. Neither their results nor the CSS controls change that denominator.

| Selection | URLs | Base named | Replay named | Gained / lost | Base / replay full pages | Local stable paired | Replay-only / stable-only passes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Priority non-CSS | 2,950 | 181,120 | 186,084 | 5,045 / 81 | 1,385 / 1,568 | 2,950 | 5,091 / 12,018 |
| Priority CSS | 1,091 | 34,156 | 34,156 | 0 / 0 | 849 / 849 | 1,091 | 807 / 1,772 |
| First feature supplement | 430 | 54,034 | 57,966 | 3,937 / 5 | 270 / 284 | 355 | 2,690 / 4,224 |
| Second feature supplement | 406 | 6,137 | 6,321 | 184 / 0 | 369 / 377 | 406 | 77 / 866 |
| Additional upstream CSS | 40 | 1,005 | 1,005 | 0 / 0 | 25 / 25 | 0 | 0 / 0 |
| Wasm storage overlay | 5 | 10 | 12 | 2 / 0 | 4 / 5 | 5 | 0 / 0 |
| Wasm metadata overlay | 7 | 16 | 16 | 0 / 0 | 7 / 7 | 7 | 0 / 0 |

Across the 4,929 selected URLs, LP gained 9,168 named passes and lost 86. Fully passing pages went from 2,909 to 3,115. The local stable pass sets are paired on 4,814 URLs. Unmeasured local stable pages remain null. The largest changes, raw lost cases, new/missing names and harness transitions are recorded in effects.json, lost-names.jsonl.gz and summary.json.

## Reconciliation and controls

The 22 source commits after aa0268b47 through 6c6045cfe replay in the same order. All 130 upstream-only paths and 50 topic-only paths retain their original blobs. Applying the original topic diff independently to the three shared upstream files reproduces the replay blobs. source-audit.json records the prior and replay patch IDs, binary fingerprints and the 538 unchanged resolved package identities. The WPT expectations and shipped Wasm backend are untouched.

The separately measured original 6c6045cfe topic and integrated replay were paired on 152 late-feature URLs. The replay gained 0 and lost 0 named passes, with 52 and 52 strict pages respectively. The exact old-topic comparand, named outcomes and receipts are in prior-focus-*.

The independent Mac Studio Chrome 158 CSS reference covers all 1,131 CSS controls. Studio fully passed 998 pages and passed 36,152 names, with 34,335 names passing on both the replay and Studio, 826 only on LP and 1,817 only on Studio. studio-css-candidate-pages.jsonl.gz and studio-css-candidate-names.jsonl.gz preserve the separate machine columns. The local reference is the primary comparator when present. Studio used CDP, disposable profiles and 16 original shards, with changed extension switches on four shards and the WPT leaf SPKI on the 41-variant service-worker supplement. Other switch, concurrency and driver details are not fully recorded. [Independent signed Chrome 158 CSS reference](https://github.com/povio/pyro/tree/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158)

| Historical occurrences | Base pass / fail / missing / withheld | Replay pass / fail / missing / withheld | Chrome 158 pass / fail / missing / withheld |
| --- | --- | --- | --- |
| Earlier 292 | 161 / 120 / 7 / 4 | 160 / 120 / 8 / 4 | 190 / 91 / 7 / 4 |
| 157 referrer-policy subset | 134 / 23 / 0 / 0 | 135 / 22 / 0 / 0 | 157 / 0 / 0 / 0 |

Original historical classifications and individual outcomes are in historical-292.json. A timeout retains its reported named outcomes. The MacBook and Mac Studio columns remain separate; no Chrome 157 or development-channel measurement enters these totals.

## Reproduction and data

The local runs used WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 (manifest SHA-256 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819), CI JSC 0e0dd7038a7a-6f4649e395e0, wptserve with HTTP/2, testdriver, fresh contexts, manifest-generated globals, four jobs, 800×600 at DPR 1. The MacBook local reference is Google Chrome stable 158.0.8127.37 on macOS 27.2.1 arm64, with verified Google signature, Playwright 1.65.1 and the recorded WPT stable profile. The export uses complete census receipts first, then individually verified four-job same-profile receipts for matching missing URLs. Each observed source blob, environment, protocol, original name, duplicate occurrence and subtest status is retained. Selection hashes and raw receipt/log/config hashes are included. The upstream source selection is held fixed.

Run `shasum -a 256 -c SHA256SUMS` followed by `python3 verify.py .` from the evidence directory. This rechecks page and named pass sets, both directed comparisons, strict pages, missing rows, historical distributions and original duplicate order. The acquisition command is in run.py; export.py, effects.py, studio.py and audit-source.py document the original-cache rebuild. The selected URL-to-shard map and task digests are in selected-urls.json and tasks.json.

Each named row is [URL, canonical name, one-based occurrence, base, replay, local Chrome]. Browser fields are [status, original name] or null. Page fields include source/blob/environment/protocol, then [page status, harness status, strict pass, passing names, reported names, producing shard] for each browser. A strict pass requires a complete page, an OK harness, and at least one subtest with every subtest passing. Studio preserves its own raw numeric harness status and original [reported names, passing names] order. Original wire ports are canonicalized only with the producing WPT server configuration. Unattributed native events and earlier exclusions remain coverage gaps.
