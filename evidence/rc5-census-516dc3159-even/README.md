# RC.5 macOS arm64 non-CSS census, local even shards

Local receipts cover 1 of 160 even ordinals, 6 of 15,072 assigned URLs. The full V2 plan has 30,179 eligible URLs across 320 shards. It excludes 1,469 historical holds plus 179 additional variants. The local comparison reuses signed Google Chrome stable 158.0.8127.37 evidence from the pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The runner uses four jobs per shard. Each page's environment, variant, harness status and missing names remain available in comparison/. The previous-LP column refers to 1f1be0324.

This executable came from the release workflow's archive-arm64 artifact 14745758172 for tag v0.0.50-rc.5. Source 516dc3159ff4602bd3a11e98607c262c0ffc65c4, Git tree 39850661325fc24f3cd9e9f591d14df55a5fa359. The complete ZIP and its inner executable were independently hashed against release metadata. The launched Mach-O arm64 executable SHA-256 was 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. The release identity describes the shipped JavaScriptCore executable. The packaged archive supplies no standalone static JSC library for an independent library hash.

| Engine | Passing / observed named occurrences | Strict pages |
| --- | ---: | ---: |
| Shipped LP | 22 / 26 | 1 |
| Signed Chrome stable | 81 / 81 | 6 |
| Historical LP | 22 / 26 | 1 |

Matched asymmetric passes number 0 LP-only and 59 signed-Chrome-only. Current page lifecycle counts are {'complete': 5, 'timeout': 1}. Original raw receipt bytes can be recovered from machine/raw/*.json.gz and checked against completed.jsonl. Each launch and port proof is preserved. Historical and newly withheld URLs remain explicit in the coverage ledger.
