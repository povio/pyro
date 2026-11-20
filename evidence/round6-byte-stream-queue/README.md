# Readable byte streams and BYOB queue processing

The pinned ordinary selection contains 32 upstream pages across Window, dedicated, shared, and service worker globals. It covers byte streams (general, minimum fill, BYOB request construction, responding after enqueue, template, tee), default-stream tee and Response.clone. The compressed files retain each ordered named row, message and page outcome. The base and candidate measured the same URL selection at four jobs. The local Chrome stable reference comes from the earlier corrected-profile run at the same WPT revision.

| Browser | Named passes | Fully passing pages | Completed pages |
| --- | ---: | ---: | ---: |
| LP base | 852 | 16 | 32 |
| LP candidate | 888 | 16 | 32 |
| Chrome stable | 1,060 | 32 | 32 |

Each global gained nine cases across queue draining into multibyte views, preserving trailing bytes, draining an insufficient closing queue, and cancellation of outstanding BYOB reads. There were no named losses, new or missing names, watchdog kills or browser restarts. The remaining 172 Chrome-only results include pending pull-into descriptor lifetime, request view/buffer layout, byte-stream tee switching to an upstream BYOB reader, and native fetch response bodies. [Stream Standard queue filling](https://streams.spec.whatwg.org/#readable-byte-stream-controller-fill-pull-into-descriptor-from-queue) · [Stream cancellation](https://streams.spec.whatwg.org/#readable-stream-cancel)

Run `python3 compare.py` to check the summary against the stored results. The LP receipts shorten the machine-local executable paths to their basenames. Provenance records original receipt and binary hashes. Reproduce an LP run with the pinned manifest runner.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --manifest-inventory \
  --measure-reviewed-skips --generated-globals "$MANIFEST" --dirs non-css \
  --server wptserve --testdriver --http2 --jobs 4 --tests selection.json \
  --bin "lp=$BIN" --out result.json
```

The standalone `pyro eval subclass.js` check confirms a Uint16Array subclass produces an intrinsic Uint16Array result and a simultaneous DataView preserves its offset, bytes and transfer behavior. The `min.js` check distinguishes Web IDL range conversion errors from requests exceeding view capacity. Both original and candidate outputs are included.
