# Round-seven non-CSS testharness census

All 30,358 selected URLs were measured on LP 1f1be032436bfeee9f48cdb5acbc0f71d21d5af9, binary SHA-256 543a7b85e3ecd8b99400d8c0c6861ff4756621b106d86407ca9d9098ca9b0d4d, with pinned CI JSC key 0e0dd7038a7a-6f4649e395e0. The JSC archive SHA-256 is 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. WPT revision a421f2fea5eb7f91e0db399e44b0fb0779ee1603, manifest SHA-256 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819. The Chrome column reuses signed Google stable 158.0.8127.37 on macOS arm64 from receipt ee24e07f6. The previous LP column reuses the complete 8e7144b7e census from receipt 9a195fb10. Each fresh LP shard used four jobs with new browser contexts, pinned manifest inventory, wptserve, HTTP/2, generated globals, testdriver and an 800×600 viewport. The allocations controller adjusted the number of concurrent shards between launches.

| Matched result | Round-seven LP | Chrome 158 stable | LP only | Chrome only |
|---|---:|---:|---:|---:|
| Named passes | 1,690,753 | 1,755,476 | 12,258 | 76,981 |
| Fully passing pages | 15,433 | 24,533 | 616 | 9,716 |

Against LP 8e7144b7e the same URLs gained 2,818 and lost 106 named passes. Fully passing pages changed from 15,327 to 15,433, with 116 gained and 10 lost.

The complete 31,827-URL non-CSS inventory includes 1,468 previously withheld variants and one later observed Chrome hang. These have explicit omission reasons. Observed page statuses were {'timeout': 1586, 'complete': 27369, 'error': 1403}. Subtest pass sets preserve names reported before incomplete page outcomes. A fully passing page requires a complete/OK harness result, at least one named subtest and every reported subtest passing.

The 48-URL exact-head smoke run used one worker. Its 41 non-CSS variants each received a fresh four-job census observation. Seven CSS variants stay in the separate smoke evidence. Frozen 305840f45 data is available for 260 distinct non-CSS URLs across archived receipts, of which 259 overlap the census. Each archive keeps its original profile and named sets. Per-receipt counters can have overlapping URLs. Static, single-worker, two-worker and four-worker observations remain individually labeled.

## Receipt layout

- Each pages.jsonl.gz row contains the manifest URL, round-seven LP, Chrome, prior LP, contributing receipt keys in that order, and withholding reasons. A page column contains lifecycle, harness status, full-pass flag, passing names and observed names.
- Each named.jsonl.gz row contains URL, canonical name, one-based occurrence, then round-seven LP, Chrome and prior LP. Each non-null outcome includes its original name and status.
- archive-pages.jsonl.gz and archive-named.jsonl.gz retain the separate frozen and smoke observations, including every comparison with an observed round-seven census URL. See archive-provenance.json for exact profiles and binary hashes. round7-smoke includes all three original columns.
- areas.json groups directional named sets by WPT area. historical-292.json tracks each historical occurrence. The full manifest inventory preserves source, environment, blob, eligibility and original reason codes.

Commands used were python3 plan.py, python3 measure.py, python3 export.py evidence, python3 archives.py evidence and python3 verify.py evidence. Run python3 verify.py . inside this folder to independently reconstruct the totals from exported page and name rows.
