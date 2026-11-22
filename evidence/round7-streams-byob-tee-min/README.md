# ReadableStream byte tee: BYOB reads and read({ min })

Parent aba03192f (`origin/testharness/round7` tip aba03192f). The change is only in `crates/pyro-webapi/src/streams.js` (parent sha256 49ff1bec…, candidate ea85fe1a…). WPT checkout a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

Byte `tee()` now follows ReadableByteStreamTee instead of a default reader with cloned chunks:

- a branch pull with a pending `byobRequest` reads the source with a BYOB reader into that request's view (min 1), then `respondWithNewView()`s the branch and enqueues a clone into the other branch
- a pull without one reads with a default reader; the source reader switches between the two (release, reacquire, error forwarding only from the current reader)
- `readAgainForBranch1/2` replace the single read-again flag
- close completes pending BYOB reads on both branches (respondWithNewView of the empty view on the BYOB branch, respond(0) on the other)

Default-stream tee and `cloneBody` (Response.clone) on default streams are unchanged. `cloneBody` on a byte body takes the byte path, as before (the clone callback was already ignored for bytes).

## Scope (frozen allowlist)

`allowlist.json`: 11 names, 1 from `read-min.any.js` (`tee() with read({ min }) from branch1 and read() from branch2`, the residual of round7-streams-byob-ordinary) and 10 from `tee.any.js`. All use ordinary valid typed arrays; none use transferred or detached buffers or error paths.

## Results (scoped, 3 runs per revision per runtime)

| | parent | candidate |
|---|---|---|
| bun 1.4.4 | 0/33 | 33/33 |
| node v24.16.0 | 0/33 | 33/33 |

Receipts: `{parent,candidate}-scoped-{bun,node}.jsonl`.

## Controls

- `controls.txt`: the 58 prior control pages plus the byte pages general, read-min and tee (64 pages, bun). Zero losses on every page; only read-min (23 → 24/24) and tee (30 → 40/40) change.
- `blob-controls.cjs`: 12 ordinary checks of `fromBytes` (Blob.stream()) and `cloneBody` (Response.clone()) with default, BYOB and BYOB-min reads, tee of both kinds, cancel, chunk independence, multi-chunk min across chunks, default cloneBody callback. 12/12 parent and candidate on bun and node.

Not run (as instructed): bad-buffers-and-views, enqueue-with-detached-buffer, non-transferable-buffers, patched-global under readable-byte-streams, crashtests, browser globals. CI gate and independent QA cover those.

## Reproduce

```
bun evidence/round7-streams-byob-tee-min/check.cjs crates/pyro-webapi/src/streams.js <wpt> streams/readable-byte-streams/tee.any.js evidence/round7-streams-byob-tee-min/allowlist.json
bun evidence/round7-streams-byob-tee-min/blob-controls.cjs crates/pyro-webapi/src/streams.js
```
