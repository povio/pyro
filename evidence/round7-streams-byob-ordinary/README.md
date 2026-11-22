# ReadableStream BYOB, ordinary valid inputs

Parent 538339506 (`origin/testharness/round7`). The source is in commit 9754bcecc, which changes only `crates/pyro-webapi/src/streams.js` (sha256 49ff1bec…c9d7; the parent's file is b759e438…75a3). WPT checkout a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

The byte controller now keeps the spec's pull-into descriptors. Before this change, `byobRequest` viewed a private copy and pending reads lived only on the reader. The change covers:

- partial fills and `read({ min })`
- `byobRequest` views over the pending read's own buffer at offset + bytesFilled
- `respond()` and `respondWithNewView()` with transfer
- `autoAllocateChunkSize` descriptors
- release steps that keep the first pull-into as readerType `none` for the next reader
- pending BYOB reads at close, which wait for `respond(0)`; byte tee does that respond on its closed branches

Public `enqueue()` now transfers the chunk's buffer instead of copying it, as the spec and Chrome do.

## Scope (frozen allowlist)

`allowlist.json` has 27 names: 19 from `general.any.js`, 7 from `read-min.any.js` and 1 from `tee.any.js`. Each is a gain on ordinary valid typed arrays or DataViews.

`check.cjs` registers only allowlisted tests, so other tests on the same pages never run. Four gains on these pages are withheld because they use `transferArrayBufferView` (structuredClone transfer):

- `read(view), then respondWithNewView() with a transferred ArrayBuffer`
- `respondWithNewView() with a transferred non-zero-length view (in the readable state)`
- `read({ min }), then respondWithNewView() with a transferred ArrayBuffer`

These pages were not run at all: `bad-buffers-and-views`, `enqueue-with-detached-buffer`, `non-transferable-buffers` and `patched-global` under readable-byte-streams, plus crashtests. The CI gate and independent QA must cover them for regressions.

## Results (actual scoped gains)

The production factory runs with WeakMap slots and upstream testharness.js, with 3 runs per revision on each runtime (bun 1.4.4 / JSC, node v24.16.0 / V8):

| | parent 538339506 | candidate 9754bcecc |
|---|---|---|
| bun | 0/27 per run (0/81) | 27/27 per run (81/81) |
| node | 0/27 per run (0/81) | 27/27 per run (81/81) |

Receipts: `{parent,candidate}-scoped-{bun,node}.jsonl`.

## Context, not claims

`full-page-context.txt` is a whole-page bun run of mixed pages:

| page | parent | candidate |
|---|---|---|
| general | 80/101 | 101/101 |
| read-min | 15/24 | 23/24 |
| tee | 29/40 | 30/40 |

No page lost a pass. The remaining read-min failure is `tee() with read({ min })`: byte tee is still default-reader based.

The historical counts in `bench/wpt/full/testharness/streams.json` come from an older browser run, not from this measurement: read-min 12/24, tee 29 of 40 with 11 failing, and no general row. Do not subtract them from these numbers.

## Controls

`controls.txt` covers 58 stream pages: readable-streams (default streams, including the prior object-input and queue-total gains), writable-streams, transform-streams, piping, queuing-strategies, and the byte pages templated, construct-byob-request and respond-after-enqueue. Parent and candidate pass counts are identical on every page, with zero losses.

## Reproduce

From the repository root, with `<wpt>` set to the checkout above:

```
bun evidence/round7-streams-byob-ordinary/check.cjs crates/pyro-webapi/src/streams.js <wpt> streams/readable-byte-streams/general.any.js evidence/round7-streams-byob-ordinary/allowlist.json
```

For the parent, use `git show 538339506:crates/pyro-webapi/src/streams.js` as the first argument. Leave out the allowlist argument to run a whole page.

Browser confirmation (Window and worker globals in a warm Pyro build) has not been run here. It is coordinated separately on the allowlisted names only. There is no release and no main merge.
