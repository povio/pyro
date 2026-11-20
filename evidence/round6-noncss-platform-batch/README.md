# Ordinary non-CSS platform batch

The cumulative topic is 35dfb7474, based on b26c9619e. It includes form-target iframe load ordering, form newline serialization, whole-element BYOB queue reads, intrinsic BufferSource view conversion, RFC 7932 Brotli decompression and SVG geometry metrics. The Cargo lockfile adds two direct uses of existing shipped crate versions (svgtypes and brotli-decompressor 6.1.0). The resolved package inventory and pinned JavaScriptCore backend are preserved.

| Same 2,445 URLs | Named passes | Fully passing pages | Harness timeouts | Harness errors |
| --- | ---: | ---: | ---: | ---: |
| LP b26c9619e | 175,667 | 940 | 88 | 35 |
| LP 35dfb7474 | 176,349 | 1,005 | 88 | 32 |
| Local Chrome stable | 188,343 | 1,896 | 13 | 4 |

Chrome additionally recorded two hangs in the reused reference. LP gained 682 named cases and lost zero on these pages; the observed names were identical. Gains by directory were 201 SVG, 191 HTML, 184 compression, 70 FileAPI and 36 streams. The separate 95-page BufferSource comparison gained 12 named passes across four global environments and lost zero. LP uses the existing intrinsic view helpers for typed arrays absent from the JSC C enum.

The separate 159-page historical-loss selection recorded 103 gains and 55 losses, with three timeouts on each LP. Of those 55 raw losses, 46 were referrer-policy assertions, six were opaque signature labels that did not match, two were canvas video assertions and one was a Fetch status assertion that also failed in the local Chrome reference. These results remain in the comparison. On the four referrer pages with the most flips, three alternating repeats returned 35/37/40 of 48 named passes on control and 37/40/42 on this head. The evidence establishes variability; it does not assign a cause to every change. Of the 292 earlier lost occurrences, this targeted candidate run reported 154 passes, 127 failures and 11 absent names. The frozen 902fe811f census has its own complete disposition.

The 1,038-page CSS guard gained zero and lost two named passes. Both raw failures differed from the assertion threshold by 0.1 ms. In five alternating isolated repeats, the offscreen transition assertion passed 1/5 on control and 0/5 on candidate, and the stylesheet import timing assertion passed 5/5 on both. The 41-page worker and UTF-16 guard preserved all statuses. The guards are reported separately because their URLs overlap other selections.

The named JSONL tables carry URL, canonical name, one-based occurrence and (status, original name) for each browser. Null means absent. Page tables carry each browser's outer status, harness status and strict full-page result. The main/historical browser order is LP control, LP candidate, local Chrome stable. The CSS files use control and candidate. The integrated main selection combines disjoint selection.json and extra.json. The dedicated historical comparison uses loss-urls.json. Source pins, hashes and concurrency are in provenance.json. Chrome results reuse the same-pin WPT profile on macOS.

Use the pinned reviewed runner, the full WPT checkout, its pinned manifest and an absolute LP binary. Substitute the selected JSON file. The candidate runner disables replay of an observation whose browser connection was lost; these matched runs had zero restarts.

```sh
bun bench/wpt/run.mjs --kind testharness --wpt "$WPT" --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out "$OUT"
```

[Form and iframe results](https://github.com/povio/pyro/tree/cd49c1041bac0b54b03878b46f8f96e19e395bd3/evidence/round6-named-form-target) · [BYOB results](https://github.com/povio/pyro/tree/6ec5129a0e79f667d62a298fbec60f8eab674bfd/evidence/round6-byte-stream-queue) · [SVG results](https://github.com/povio/pyro/tree/5b9d485f9be0abd3515ea83a2e35e4a8c1bc5670/evidence/round6-svg-path-metrics) · [Brotli workload and WPT](https://github.com/povio/pyro/tree/07e9d9a15521bd866e035657f4f91df3ae357a88/evidence/round6-brotli-decompression)
