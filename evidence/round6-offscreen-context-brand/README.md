# Native offscreen canvas 2D interface

OffscreenCanvas.getContext('2d') now creates an OffscreenCanvasRenderingContext2D with its own prototype, constructor and receiver checks. It uses the existing native drawing operations. The element-only canvas interface and focus drawing method are exposed in Window as specified. The ordinary Web IDL accessor installation path now gives setters one argument, matching the optimized accessor path.

| Same 401 ordinary URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP cbdeb1193 | 6,133 | 367 |
| LP 51a4772ae | 6,309 | 375 |
| Local Chrome stable 158.0.8127.37 | 7,102 | 395 |

LP gained 176 named passes and eight full pages with zero losses and identical observed names. All 401 pages completed on each browser, without browser restarts. The earlier context-only revision showed one worker IDL setter arity loss; fixing the fallback accessor descriptor removed that loss. The same pinned WPT selection, testdriver, HTTP/2 and four jobs were used on all three builds.

A fresh normal browser probe produced the same values as signed Chrome for Window and worker interface identity, illegal cross-brand calls, drawing pixels from an offscreen source, ImageBitmap transfer and source clearing, PNG encoding/decoding, and resizing. Seventeen native canvas integration tests passed. This changes the bindings and retains the existing raster backing; it is independent of the isolated wide-gamut experiment. The names table preserves original names, occurrences and all three statuses, with null for missing names. Browser order is control, candidate, local Chrome.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --server wptserve \
  --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --dirs non-css --jobs 4 --tests selection.json --bin "lp=$BIN" --out "$OUT"
```

[Shared reference and inventory](https://github.com/povio/pyro/blob/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158/README.md)
