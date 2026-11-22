# Shipped RC.5 non-CSS testharness census

Both macOS arm64 machines ran the exact RC.5 release workflow executable at 516dc3159ff4602bd3a11e98607c262c0ffc65c4, SHA-256 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. Both hosts reported macOS 27.2.1 build 26B131 and the same dynamically loaded system JavaScriptCore UUID C083204E-71DA-3BC5-AFF4-524B5EAFF741, bundle 22625.2.8.11.9. The measured workflow archive reports fork:false. The separately packaged npm darwin-arm64 browser uses a patched static engine. The Studio archive supplies a per-ordinal completion ledger, original run logs, traces, configurations and machine/executable provenance; separate per-shard launch start records are available only in the even archive. The even partition covered 15,072 URLs locally and the odd partition covered 15,107 URLs on Studio. The 320 disjoint shards account for all 30,179 eligible URLs at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The comparator was signed Chrome stable 158.0.8127.37; the historical LP comparator is 1f1be0324. Reference results retain their original execution provenance.

| Engine | Named passes / observations | Strict pages |
| --- | ---: | ---: |
| Shipped LP | 1,691,766 / 1,785,720 | 15,277 |
| Signed Chrome stable | 1,754,948 / 1,802,021 | 24,385 |
| Historical LP | 1,690,433 / 1,789,453 | 15,344 |

Confirmed LP-only passing occurrences total 12,623, and confirmed Chrome-only occurrences total 75,805. Per-attempt port proof is missing for 0 shards and affects 0 occurrence rows. The confirmed ledgers exclude these rows, which are retained with the source shard, original names and statuses in unresolved-mappings.jsonl.gz. Raw-identity asymmetries are reported separately in the complete comparison counters. The official inventory has 31,827 URLs, including 1,469 historical holds and 179 additional held variants. Coverage and per-environment dispositions, page errors, incomplete runs, missing named observations and native exits are preserved in comparison/. Raw-input commits retain individual receipt and launch provenance.
