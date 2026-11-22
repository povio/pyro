# RC.5 macOS arm64 non-CSS census, local even shards

Local receipts cover 160 of 160 even ordinals, 15,072 of 15,072 assigned URLs. The full V2 plan has 30,179 eligible URLs across 320 shards. It excludes 1,469 historical holds plus 179 additional variants. The local comparison reuses signed Google Chrome stable 158.0.8127.37 evidence from the pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The runner uses four jobs per shard. Each page's environment, variant, harness status and missing names remain available in comparison/. The previous-LP column refers to 1f1be0324.

This executable came from the release workflow's archive-arm64 artifact 14745758172 for tag v0.0.50-rc.5. Source 516dc3159ff4602bd3a11e98607c262c0ffc65c4, Git tree 39850661325fc24f3cd9e9f591d14df55a5fa359. The complete ZIP and its inner executable were independently hashed against release metadata. The launched Mach-O arm64 executable SHA-256 was 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. This workflow archive is intended for the ordinary GitHub release download. The publication-comparison file records the separately observed status of the public release and npm packages. The executable loads the system JavaScriptCore framework. On this local Mac running macOS 27.2.1 build 26B131, dyld loaded UUID C083204E-71DA-3BC5-AFF4-524B5EAFF741, bundle version 22625.2.8.11.9. The executable reports fork:false. The npm darwin-arm64 package is a distinct workflow product with a patched static JavaScriptCore and fork:true. Its conformance is unmeasured by this run.

| Engine | Passing / observed named occurrences | Strict pages |
| --- | ---: | ---: |
| Shipped LP | 825,072 / 874,074 | 7,664 |
| Signed Chrome stable | 856,367 / 879,997 | 12,174 |
| Historical LP | 823,620 / 874,330 | 7,697 |

Matched asymmetric passes number 6,671 LP-only and 37,966 signed-Chrome-only. Current page lifecycle counts are {'complete': 13513, 'error': 791, 'timeout': 768}. Original raw receipt bytes can be recovered from machine/raw/*.json.gz and checked against completed.jsonl. Each launch and port proof is preserved. Historical and newly withheld URLs remain explicit in the coverage ledger.
