# IndexedDB Web IDL and event-handler bindings

The control, candidate and signed Chrome 158.0.8127.37 ran the same 60 ordinary upstream WPT URLs on macOS arm64. Each LP run completed 55 pages and recorded five harness errors. No browser restarted or timed out. The full page and named results retain all error states. Independent of the WPT counts, the candidate and Chrome passed eight supplemental descriptor, cross-realm handler lifecycle and transaction probes. The control passed two.

| Browser | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP control | 825 | 44 |
| LP candidate | 1,225 | 44 |
| Chrome stable | 1,280 | 60 |

All 400 new named passes occur on the four IndexedDB interface pages (100 each). The comparison lost zero passes or reported names. The other 56 pages are guards that exercise database upgrades, request results and errors, transactions, object stores, indexes and cursor traversal. Window passes 206/207 interface cases; each dedicated/shared/service worker passes 197/207. The remaining interface differences concern prototype placement of Window.indexedDB and the representation of lazy interface globals in workers. LP's five harness errors remain on four transaction-abort variants and a dedicated worker createIndex variant.

The raw compressed browser receipts retain each original named case, occurrence order, message and harness state. The local binary paths are shortened to basenames. Verify with `python3 compare.py` and `shasum -a 256 -c SHA256SUMS`. The manifest-first WPT reference runner uses the signed stable profile and fresh contexts, with four jobs per browser.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 \
  --tests selection.json --bin "candidate=$BIN" --out result.json
```

The separate probe is served as probe/idb.html beside the pinned WPT resources/ and fonts/ directories. Invoke the same runner with `--wpt "$PROBE_ROOT" --kind testharness --dirs probe --jobs 1`. It checks arity, descriptor enumerability, receiver identity, brand rejection, read/write ordering, listener-slot reactivation, cancel-on-false, object/primitive assignment and a long-lived request after removing the frame that supplied a borrowed setter. [IndexedDB IDL](https://w3c.github.io/IndexedDB/) · [Web IDL ECMAScript bindings](https://webidl.spec.whatwg.org/#ecmascript-binding)
