# Combined parser checkpoint

The two parser source commits from the reviewed inert-template branch were replayed on combined LP `8e7144b7e7de68be3b5b345138abe2c1dcc6d232`. Candidate source is `054ffa4c43fa84bdd609b0a02cac2ccb1bb04760`. The only conflict involved adjacent module declarations in pyro-dom/src/lib.rs. The combined file retains the two SVG modules and the template patch module. Added and removed source lines across both reviewed and replayed patches match.

| Matched selection | Base named passes | Candidate named passes | Chrome named passes | Full pages (base / candidate / Chrome) |
|---|---:|---:|---:|---|
| 32 template pages | 62/494 | 138/494 | 475/494 | 5 / 7 / 30 |
| 33 parser controls | 778/801 | 778/801 | unmeasured here | 27 / 27 / — |

All 76 gained names pass in Chrome. Both LP runs completed all 65 URLs. No named losses, missing names, timeouts, watchdog kills or restarts occurred. These runs measure the incremental parser change on this exact combined base. They cover detached inert setters and insertion. Live documents, script-activating parsers and network-backed templates remain incomplete.

Both LP columns were freshly measured. The focus Chrome column reuses the same signed macOS arm64 Chrome stable 158.0.8127.37 receipt published on the standalone parser topic at 22ce341dd. All receipts identify WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The combined base uses the verified binary from integration-52d8b7c (SHA-256 `daea9cf80d257a1a0574e625ba38e58358104defbf2b7dee408fd8af667d72f8`). The candidate uses the same pinned JSC `0e0dd7038a7a-6f4649e395e0` and wptiter build profile (binary SHA-256 `26b68bbb2315c5940ec0f6dd902ff573060fbd07800cc99a1e9df31e91f590a0`).

The local harness used this invocation for each LP binary and selection. The named tables use URL, original test name and occurrence as the key. The page tables retain harness status, completion status and whether every named assertion passed. Run compare.py after decompressing the raw JSON files in its directory to reproduce the summary.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 2 \
  --tests "$SELECTION" --bin "$LABEL=$BINARY" --out "$LABEL.json"
```
