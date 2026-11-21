# Frozen LP versus stable Chrome 158 across the pinned manifest

LP cbdeb11931f74c3de4417a25fb9103f1df84d54e and Chrome stable 158.0.8127.37 have paired records on 38,353 of 39,989 testharness URL variants at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The non-CSS comparison uses the local signed Chrome build. CSS uses Studio stable on every paired URL. Both are macOS 27.2.1 arm64. The local CSS Chrome results remain available separately. Later LP feature topics have different binaries and measured scopes.

| Paired scope | URLs | LP full pages | Chrome full pages | LP named passes | Chrome named passes | LP-only passes | Chrome-only passes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Local non-CSS | 30,359 | 15,274 | 24,533 | 1,683,159 | 1,755,476 | 11,679 | 83,996 |
| Studio-reference CSS | 7,994 | 5,406 | 6,872 | 175,494 | 181,219 | 5,712 | 11,437 |
| Combined | 38,353 | 20,680 | 31,405 | 1,858,653 | 1,936,695 | 17,391 | 95,433 |

A full page requires a completed harness, OK harness status, and at least one named result, all passing. Name identity includes the manifest URL, name and one-based occurrence. CSS uses exact original names. The non-CSS join from the parent receipt normalizes encoded WPT URL ports only when supported by each server's recorded port-role map and keeps original names. A named pass counts even when another test on that page failed. The observed deficit is 78,042 named passes and 10,725 full pages. Studio and local Chrome each have four exclusive passes on their 1,091 shared CSS pages. See the [profile details, exclusions, and original Chrome receipts](../round6-chrome158/README.md).

## Coverage

The original matched LP and local Chrome ledgers each finished 329 shards. A separate 73-shard supplement used the same LP binary and 6,998 disjoint CSS URLs. Its 72 completed shards provide 6,903 LP observations with zero recorded process restarts or watchdog kills. The five previously untouched shards finished on resumption. On shard 067, the saved partial contains the initial selection and zero checkpointed rows. The server log only records startup, as do neighboring completed shards' logs. The runner writes an initial empty partial before starting the server and then checkpoints after 100 rows or 30 seconds of progress. The shard had 95 URLs. Neither the last active page nor the cause of interruption can be established. The claim, partial and startup traces were preserved locally. All 95 rows are explicitly unobserved on LP and none were replayed.

The other 73 CSS omissions consist of 34 LP holds and 39 Studio exclusions. The 1,468 non-CSS omissions retain their original reasons. All 1,636 unmatched URLs remain in the inventory without pass credit. For 129 of those URLs there is Studio CSS reference data only. The original guarded Chrome websocket hang and restart remain recorded on the local non-CSS selection. Separate five-URL Wasm storage and seven-URL metadata controls retain the parent evidence's distinct provenance and are outside these frozen selection totals.

## Readback

Run `python3 evidence/round6-single-head-158/readback.py` from this checkout to regenerate the summaries without running a browser. The parent evidence supplies the original named sets and page observations. The supplement folder contains compressed, hash-checked raw LP receipts and the original selection, ledger, runner invocation, interruption evidence and holds. The pinned JSC build is 0e0dd7038a7a-6f4649e395e0. The frozen LP executable SHA-256 is 81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2.

- `inventory.jsonl.gz` has one row per manifest URL with URL, source, blob, environment, scope, LP page, LP shard, LP omission reason, local Chrome receipt and Studio Chrome receipt. The LP page records outer status, harness status and strict pass. Both Chrome columns retain the parent inventory's source and coverage context.
- `css-names.jsonl.gz` records URL, original test name, one-based occurrence, LP outcome, Studio Chrome outcome and local Chrome outcome. Null means unobserved.
- [Non-CSS named rows](../round6-chrome158/cbdeb-names.jsonl.gz) include URL, comparison name, occurrence and each side's original result. Filter those rows to non-CSS URLs when combining with the CSS ledger here.
