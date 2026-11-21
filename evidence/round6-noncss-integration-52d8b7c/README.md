# Non-CSS integration guard on 52d8b7c57

LP 8e7144b7e7de68be3b5b345138abe2c1dcc6d232 on 52d8b7c576c82ff38b5709f989dca24a8cba141d completed all 4,929 preselected URLs in 58 disjoint shards on both exact-JSC builds. The 22 replayed commits have identical ordered stable patch IDs to the previous topic. This checkpoint measures the selected 4,929 URLs. The completed 30,359-URL ordinary non-CSS census on cbdeb11931f74c3de4417a25fb9103f1df84d54e remains separately published at ee24e07f6b15b399d6e8e2157c763809341b6e44. Both binaries used JSC 0e0dd7038a7a-6f4649e395e0 with archive SHA-256 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. Source and measured executable digests are in source-audit.json.

Against its integration base the candidate gained 9,161 named passes and lost 80. Fully passing pages went from 2,908 to 3,115. A fully passing page completed with an OK harness and at least one subtest, all passing.

| Cohort | URLs | Base / candidate named passes | Gained / lost | Base / candidate full pages | Chrome paired URLs | Candidate-only / Chrome-only names |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| priority/non-css | 2,950 | 181,121 / 186,083 | 5,037 / 75 | 1,384 / 1,568 | 2,950 | 5,090 / 12,018 |
| extra-feature/non-css | 430 | 54,034 / 57,966 | 3,937 / 5 | 270 / 284 | 430 | 3,371 / 5,121 |
| priority/css | 1,091 | 34,155 / 34,156 | 1 / 0 | 849 / 849 | 1,091 | 807 / 1,772 |
| upstream-css/css | 40 | 1,011 / 1,011 | 0 / 0 | 25 / 25 | 0 | 0 / 0 |
| additional-noncss/non-css | 406 | 6,137 / 6,321 | 184 / 0 | 369 / 377 | 406 | 77 / 866 |
| wasm-metadata/non-css | 7 | 16 / 16 | 0 / 0 | 7 / 7 | 7 | 0 / 0 |
| wasm-storage-supplement/non-css | 5 | 10 / 12 | 2 / 0 | 4 / 5 | 5 | 0 / 0 |

## Comparison with the previous checkpoint

The two integration heads, 4bf8195b4d87ad340501bb564ed2cea29b06d75f and 8e7144b7e7de68be3b5b345138abe2c1dcc6d232, have identical guard URLs. The new head gained 74 named passes and lost 69 against the previous head, and gained 11 and lost 11 strictly passing pages. In their paired base-to-head comparisons, 20 named losses repeated, 66 were seen only in the previous pair and 60 only in the new pair. These are observed transitions with no assumed causal attribution. prior-comparison.json and directional-loss-summary.json break down the scope and directories. prior-named-changes.jsonl.gz retains both observed status columns and their original names for each changed occurrence.

## Corpus and reference

Chrome stable 158.0.8127.37 and WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 were pinned. Local saved Chrome results cover 4,889 of the 4,929 URLs. Results use the frozen census first, then same-profile addenda. The selection includes two reviewed Wasm overlays (5 storage variants and 7 metadata variants), outside the original ordinary-eligible inventory. Missing page or name results are represented explicitly. Studio CSS outcomes use a separate profile and are published separately in studio-css-candidate-pages.jsonl.gz and studio-css-candidate-names.jsonl.gz. Chrome 157 observations remain historical.

Receipt provenance (base / candidate): 0 / 0 process restarts. No selected URL equals the recorded Chrome WebSocket hang. Named matching normalizes only server ports identified by each receipt and groups duplicate original names by occurrence. pages.jsonl.gz and named.jsonl.gz preserve every selected URL and every observed occurrence.

## Reproduction

Build the base and candidate with the archive pinned above, using the round6 build-exact wrapper and separate Cargo targets. Run python3 run.py base and python3 run.py new with the same selections and unmodified no-native-retry harness, then run audit-source.py, export.py, effects.py and verify.py with the output directory as their argument. Run prior-readback.py with the previous and new export directories. The SHA256SUMS file verifies all published exports. The previous receipt set was published at 57d377e7a81eac04e9478f68a64b56a2404f8395.

Each named row in named.jsonl.gz contains URL, canonical name, one-based occurrence, base, candidate, and local Chrome. The browser columns retain the status and original name, or null. Each page row in pages.jsonl.gz contains URL, shard, cohort, scope, source/blob/environment/protocol, and the three browser columns. Browser page columns carry page status, harness status, strict pass, passing names, reported names, and producing shard. Timed-out pages retain observed subtests. All local runs used four jobs, fresh contexts, wptserve with HTTP/2, testdriver, manifest-generated globals, and an 800 × 600 viewport at DPR 1.

Among the 292 earlier recorded transitions, both current LP binaries observed 288 names or their pages. Four names on the historical fetch/metadata/window-open.https.sub.html hold retain unobserved-page status. historical-repeat.json compares the prior and current matching results for each binary. historical-292.json preserves the original causes and individual names.

Read back the published artifacts with `shasum -a 256 -c SHA256SUMS` and `python3 verify.py .`.
