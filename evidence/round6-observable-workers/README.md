# Observable and Subscriber in workers

All 48 ordinary URL variants under dom/observable completed. The two LP binaries used the same CI JavaScriptCore and matched WPT configuration. LP gained 237 named passes and lost zero, moving from 25 to 46 fully passing pages. Local Chrome stable and the candidate agree on all 507 named outcomes. The reference rows come from the completed corrected-profile macOS Chrome census at the same WPT revision. The measurements cover Window and dedicated workers. Shared/service worker exposure uses the common WorkerGlobalScope binding and lacks a dedicated upstream Observable run in this selection.

The compressed receipts contain every selected page and its ordered named rows. LP receipts retain runner metadata with the local binary path shortened to its basename. The Chrome receipt is a selection of unchanged historical result rows. Run `python3 compare.py` in this directory to verify the named and page totals. Use the census manifest runner to reproduce the LP measurement.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out result.json
```

## Remaining two failures

The same named test in Window and dedicated workers expects a throwing async iterator `next` property getter to notify the observer in a later microtask. Both browsers deliver this error synchronously. The Observable draft runs GetIterator(value, async) at subscription and immediately invokes subscriber.error on throw. ECMAScript GetIteratorFromMethod calls GetIteratorDirect, whose first step reads the iterator's next property. A getter exception therefore occurs while acquiring the iterator record. Calling the acquired next method is a later step with separate promise-based error handling. LP retains the two observed failures and the synchronous draft behavior. The test expectations are unchanged. [Observable conversion](https://wicg.github.io/observable/#from-async-iterable-conversion) · [ECMAScript GetIteratorDirect](https://tc39.es/ecma262/multipage/abstract-operations.html#sec-getiteratordirect) · [Pinned WPT source](https://github.com/web-platform-tests/wpt/blob/a421f2fea5eb7f91e0db399e44b0fb0779ee1603/dom/observable/tentative/observable-from.any.js#L873-L894)
