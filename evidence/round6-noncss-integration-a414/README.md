# Non-CSS integration topic on a414fab44

All 14 source commits replayed cleanly. The range diff matches every patch. The integration tree contains all 49 upstream paths and all 26 non-CSS paths byte-for-byte from their respective heads. No paths overlapped, and the source audit reconstructed the original feature tree independently. All 13 vendored patches round-tripped against the original source base. There are no new Cargo package versions or checksums; two already-linked packages gained a direct dependency edge. Wasmi is outside both trees.

## Matched integration results

| Same 2,445 non-CSS URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| Original LP 902fe811f | 170,940 | 908 |
| Original feature topic 797312da6 | 177,308 | 1,017 |
| Integration base a414fab44 | 170,940 | 908 |
| Replayed topic 52a207a13 | 177,308 | 1,017 |
| Local Chrome stable | 188,343 | 1,896 |

On the original base the changes gained 6,371 names and lost 3. On current integration they gained 6,371 and lost 3. The original topic compared with the replayed topic gained 0 and lost 0. Each summary preserves every raw lost key. The three missing-constructor error-message cases on the earlier integration comparison are unreported by both the implemented browser and Chrome; the new constructor assertions pass. The last editing change alone gained 959 named passes and 12 full pages on the original integration suite.

On 1,050 matched CSS URLs, upstream and replayed topic passed 33,314 and 33,313 names respectively, with 0 gains and 1 losses. The selection adds 12 URLs to the earlier guard, covering every changed URL in the upstream main comparison. The 41 supplemental worker and UTF-16 URLs gained 0 and lost 0. The sole raw CSS loss measured the offscreen transition start time 0.1 ms apart. In five alternating repeats this assertion passed 1/5 on upstream and 1/5 on the replayed topic. css.json retains all status changes. The prior integration topic's independent historical-loss and timing observations remain separately available. [Prior platform evidence](https://github.com/povio/pyro/tree/ff87c6239b92966d0c91000ae2e25993e5337380/evidence/round6-noncss-platform-batch)

## Reproduction

All local runs used the pinned WPT and JSC recorded in provenance.json. The local Chrome column reuses the same-revision stable macOS census. The full census uses its original frozen LP checkpoint. Select each of the disjoint selection.json and extra.json sets; these together contain the full 2,445-URL non-CSS guard. The normal browser runner performed fresh observations for all four LP binaries. The no-native-replay runner records a connection loss as a missing observation without retrying the URL.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

Each named JSONL table retains URL, canonical name, occurrence and the ordered (status, original name) observations. Null denotes an unreported name. Pages retain outer, harness and strict pass status. integrated compares the original base and original feature topic; a414 compares the integration base and replayed topic; rebase compares the original and replayed feature heads; editing-integration isolates the last editing change on the original topic. The third column in each is matched Chrome stable. Supplemental CSS tables identify each cohort explicitly.

[Editing source and 710-page guard](https://github.com/povio/pyro/tree/3b9f656ffc9326319adb7e800355deb40a623498/evidence/round6-anonymous-right-line) · [Earlier 1,475-page comparison](https://github.com/povio/pyro/tree/2579385b4f836ee56db8a36a391de11a5c8de807/evidence/round6-noncss-census-followups)
