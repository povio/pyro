# LP non-CSS census at 902fe811f

This snapshot reports 30,513 of 30,513 selected URLs. The full pinned inventory has 31,827 variants. All 1,314 withheld URLs retain a coverage reason. Pending observations also remain explicit in inventory.jsonl.gz. The historical-loss ledger was sampled at 30,513 URLs and covers all 292 canonical historical lost occurrences.

The LP binary SHA-256 is 9f2bfe099b2d87b0626040695a0583d460c9f7b3c8a401941761c2d9b6595cb4. Its CI JSC is 0e0dd7038a7a-6f4649e395e0. WPT is a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Each browser runner uses four jobs and the published WPT profile. This census used up to three sharded runners concurrently. Individual timing outcomes can vary.

Local Chrome stable 157.0.8103.80 and prior LP 4c13e694a are the original same-revision macOS arm64 results from the [published census](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census). The original census records exact controller versions and public references. original-runner-shards.json identifies the shards completed with the original runner. Subsequent shards disable page replay after browser or connection loss. Both runner variants and their source patches are identified in provenance.json. This snapshot reuses those local receipts. It does not rerun Chrome.

pages.jsonl.gz stores URL, environment and five triples (page status, harness status, fully passing). named.jsonl.gz stores URL, canonical name, one-based occurrence and five pairs (reported status, original name). The browser order is current LP, prior LP, same-pin local Chrome stable, public Chrome stable 157.0.8103.38 (Linux 22.04, run 5478394868006912, WPT 1eecaa0c300058f0a47472f8a4733552f929cc63, ten commits before the pin), and public Chrome canary 159.0.8143.0 (Linux, run 4907012216061952, exact WPT pin). The final inventory field records whether the public stable test source matches the pin. Public browser statuses retain their original casing. Null means missing. Canonicalization applies only to encoded WPT ports authenticated against the actual server configuration. The final distribution retains the per-page role maps and source configuration hashes. Run python3 verify-published.py from that directory for offline verification.

The run command and exact eligible selections are derivable from start.py, inventory.jsonl.gz and the pinned runner. To reproduce the reports with the original manifest-runs cache, run python3 compare.py, python3 inventory.py and python3 historical-losses/update.py. The checksums cover all distributed artifacts. The final shard-selections.json.gz maps every measured shard to its ordered URL selection; shard-provenance.json identifies the producing runner, raw receipt and log. Extract one selection to SELECTION and supply absolute input/output paths. Use the reviewed runner and add --measure-reviewed-skips only for the legacy-reviewed shards.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --server wptserve \
  --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory --dirs non-css \
  --jobs 4 --tests "$SELECTION" --bin "lp=$BIN" --out "$OUT"
```

## Matched outcomes

| Reference | Matched URLs | LP-only passes | Reference-only passes | Common passes | LP-only full pages | Reference-only full pages |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| LP 4c13e694a | 30,513 | 9,249 | 272 | 1,663,146 | 288 | 33 |
| Local Chrome stable | 30,513 | 10,775 | 93,397 | 1,661,620 | 578 | 10,065 |
| Public Chrome stable | 30,512 | 10,447 | 99,582 | 1,661,946 | 557 | 11,530 |
| Public stable on unchanged test sources | 30,494 | 10,446 | 99,450 | 1,661,935 | 557 | 11,527 |
| Public Chrome canary | 30,513 | 9,282 | 104,737 | 1,663,113 | 503 | 12,613 |

LP passed 1,672,395 named occurrences and 14,990 complete pages. The process coverage report records native or watchdog interruptions separately from assertion failures.

## Largest local Chrome-only areas

| Area | Matched URLs | Chrome-only named passes | LP-only named passes |
| --- | ---: | ---: | ---: |
| editing | 710 | 13,452 | 2,201 |
| webnn | 610 | 13,093 | 0 |
| html | 9,060 | 10,937 | 1,376 |
| WebCryptoAPI | 334 | 6,395 | 3,016 |
| webgl | 22 | 4,248 | 0 |
| webcodecs | 283 | 3,666 | 0 |
| webaudio | 348 | 2,883 | 145 |
| mathml | 216 | 2,568 | 19 |
| fetch | 917 | 2,285 | 301 |
| svg | 787 | 2,145 | 75 |
| webrtc | 265 | 1,972 | 1 |
| input-events | 34 | 1,788 | 81 |

The earlier 292 lost occurrences now comprise 131 passes, 149 failures and 12 unreported names. Of 157 referrer-policy occurrences, 105 pass and 52 still fail. The historical-loss directory retains all original and current name and page statuses.

Keeping the earlier measured Wasm reference-only set fixed and granting every other name in the combined observed census universe, LP would still be 94,751 named passes short of public stable. Runtime-generated names outside these runs remain unknown. [Isolated Wasmi evaluation](https://github.com/povio/pyro/tree/f0d5ed36e047d68e41e33521d28636a6ca3c13bb/evidence/round6-wasm-evaluation)
