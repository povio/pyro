# TOS representative coverage batch 7, 2026-10-17

Batch 7 is 13 unchanged manifest-routed scenarios from four files not covered by
batches 1–6:
- the direct-invoice credit note flow;
- quote DocuBox (folders, search, folder creation);
- office INTTRA credentials;
- domain error visibility.

The selection was recorded before any run of these files. Unrouted DocuBox
upload/download cases are excluded. Nothing here claims download or `blob:`
resolution support.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `43f9415` | `lpb7-1017-172918` | 9 / 4 |
| upstream `2ad38773` | `lpb7u-1017-173201` | 13 / 0 |

All four rewrite failures are 10 s timeouts. No pageerror or missing API was
reported. Their causes were found with separate diagnostic runs (below).

**INTTRA credentials: request body missing from CDP (W3, lp-cdp).** Affected
cases: "generates Client and Carrier credentials from the details modal" and
"autosaves an INTTRA credential edit through the rendered form".
- The POST/PATCH requests are sent and return 200.
- The tests' unchanged predicates read `request.postDataJSON()`, which is empty.
- Playwright 1.59.1 `crNetworkManager.js` builds the request body only from
  `request.postDataEntries` and ignores `postData` text.
- lp-cdp emits `hasPostData`/`postData` but not `postDataEntries`, on both
  `Network.requestWillBeSent` and `Fetch.requestPaused`.

**Credit note: same-document URL reported late (W1, lp-browser).** Affected cases:
"creates an actual Credit Note from the draft direct-invoice Credit Note" and
"renders the actual direct-invoice Credit Note".
- The test reads the new credit note id from `page.url()` right after the draft
  details response.
- On the rewrite, at that moment JS `location.pathname` already showed the credit
  note, but `page.url()` still showed the source invoice. The test therefore waited
  for an issue POST to the wrong id. On upstream, the URL Playwright reports and
  the page's own URL agree.
- A minimal Playwright repro shows `page.url()` is still stale right after a
  `page.evaluate` that calls `history.pushState` resolves:

  | Browser | `page.url()` |
  | --- | --- |
  | rewrite | old URL, 2/2 rounds |
  | upstream | updated |
  | Chrome headless shell 1217 | updated |

- Cause: `PageNav::same_document` in `crates/lp-browser/src/nav.rs` posts the
  `FrameNavigatedWithinDocument` page event (CDP `Page.navigatedWithinDocument`)
  as a navigation task instead of emitting it synchronously.

Both fixes belong to their owners (W1 and W3) and are routed through the parent.
A compatibility rerun of the four cases needs a freshly built, pinned candidate
that contains both fixes.

## Diagnostic runs (not compatibility evidence)

All diagnostic runs used a separate fixture, `/private/tmp/lp-tos-f9f-batch7-diag`,
with observation-only instrumentation: logs of request method, path, body keys and
status, DOM event markers, and `page.url()`/`location` at the id capture.
Assertions, timeouts, auth and the manifest were unchanged. No tokens or request
body values were logged, and UUIDs were truncated.

| Run | Binary | Specs | Result |
| --- | --- | --- | --- |
| `lpb7d-1017-173725` | `43f9415` | INTTRA + credit note | 4 / 4, same failures |
| `lpb7d2-1017-174552` | `43f9415` | credit note | 2 / 2 |
| `lpb7d3-1017-174808` | `43f9415` | credit note, with capture-time URL log | 2 / 2 |
| `lpb7d3u-1017-174908` | upstream | credit note, with capture-time URL log | 4 / 0 |

Artifacts:
- `/private/tmp/lp-tos-batch7-diag`, `-diag2`, `-diag3`, `-diag3-up`. Each has its
  `instrumentation.patch`.
- Minimal repro: `/private/tmp/lp-pushstate-repro` (`repro.mjs`, `results.txt`,
  `README.md`).

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `30c74f9b986c127e2e17070ad3681c07d048da0156744276c9bca7d33f8990c2`.
  - It covers all manifest `common` entries for `finances/direct-invoice-credit-note`
    (4), `logistics/quotes-docubox` (3), `configuration/offices-inttra-credentials`
    (4) and `master-data/domain-error-visibility` (2).
  - Both compatibility runs reported 13 tests.
- Binaries. Each hash was verified before execution.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `43f9415` | `91a8454368d697aa3621bb1b5c40d71660304bfb9e6f062b796955b7cdd25450` | release build from a fresh detached worktree with its own target directory |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture: the main run used a fresh, clean `/private/tmp/lp-tos-f9f-batch7`. The
  upstream run reused it to save disk space. Its only change was the
  harness-generated pdfgen model.
- Auth and setup:
  - backend test token into localStorage, not UI login;
  - the same direct-lane version-gate bypass as earlier batches;
  - one Playwright worker and native PostgreSQL 17 with a per-run slug and
    template.
  - After every cleanup, including the diagnostic runs, the test role owned zero
    databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb7` | 106.0 s | 67.7 s |
  | `lpb7u` | 63.5 s | 26.2 s |

  The rewrite's time includes four 10 s timeouts.
- History: the main and upstream runs and diagnostic 1 were made by the authorized
  fallback W2 writer (handoff `target/handoffs/SECOND-ASTRA-20261017.md`).
  Diagnostics 2 and 3 and the minimal repro were made after writer ownership
  returned.

## Rerun of the failing files on integrated main `82477c9`

Integrated main `82477c9ed06c88f4baf3e9d19c84b0759c93db47` contains PR #56 (CDP
`postDataEntries`) and PRs #58/#59 (synchronous same-document commit, and command
replies queued behind the events they produce).

The rerun used the two affected files from the recorded selection, all cases of
each: `direct-invoice-credit-note` (4) and `offices-inttra-credentials` (4), 8 of
the 13 cases. The other 5 were not rerun. These 6/8 results are separate from the
9/13 run above; no fresh 13-case run was made, and no combined 11/13 result is
claimed.

| Binary | Run | Cases | Passed / failed |
| --- | --- | --- | --- |
| `82477c9` | `lpb7f-1017-181417` | 8 | 6 / 2 |

- **INTTRA: 4/4.** Both earlier failures pass.
- **Credit note: 2/4.** "creates an actual Credit Note from the draft
  direct-invoice Credit Note" and "renders the actual direct-invoice Credit Note"
  still time out waiting for the issue response. They are recorded as observed,
  timing-dependent failures.

**Observed ordering.** These observations come from diagnostic runs and protocol
traces, all labelled below.
- The test reads the new credit-note id from `page.url()` as soon as
  `waitForResponse` matches any invoice-detail GET. The app calls `pushState` to
  the credit note only after it has consumed that refetch.
- On the rewrite, the Playwright protocol trace shows this order:
  1. `Network.responseReceived` and `loadingFinished` for the source invoice's
     detail GET, the last of four refetches;
  2. then `Page.navigatedWithinDocument`.

  The test captured the source id.
- On the Chromium lane, the trace shows `Page.navigatedWithinDocument` arriving
  before `Network.responseReceived` for that detail GET. The test captured the
  credit-note id and passed 4/4.
- On upstream, the four refetches completed in request order, with the detail
  second of four rather than last. The test captured the credit-note id and passed.
- A controlled repro of the same shape (wait for a response, then read
  `page.url()`, while the page calls `pushState` after reading that body) returns
  the previous URL on the rewrite, upstream and Chrome alike. The test's own
  timing decides the outcome.

**Inferred, not established.** Chrome probably wins the race because of DevTools
transport latency across processes. Upstream probably wins it because of its
response completion order. No spec or protocol violation by the rewrite has been
established. No network-event delay heuristic was added, and the tests and
assertions are unchanged.

**The real defect is fixed.** The minimal repro (`page.url()` right after a
`page.evaluate` that calls `history.pushState` resolves) returned the stale URL on
`43f9415`. On `82477c9` it returns the updated URL in 2/2 rounds, matching upstream
and Chrome (`/private/tmp/lp-pushstate-repro/results-82477c9.txt`).

### Rerun provenance

- Source: fresh `--no-hardlinks` clone, detached and clean; tree
  `762ba4ca218c73172ffda6a8383aba6a4ebcd6be`.
- Release binary: SHA-256
  `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9`, built with
  its own target directory.
- Fixture: to save disk space, the rerun reused the original
  `/private/tmp/lp-tos-f9f-batch7` at `f9fde4aa0d`. Its e2e tree was verified
  unchanged; the only change was the harness-generated pdfgen model.
- Databases:
  - The harness left the run's base template database in addition to the snapshot.
  - The original `cleanup.py` refused it, because its assertion only allowed
    snapshot names, so nothing was dropped.
  - A second script, `cleanup2.py`, also allowed that exact template name. Both
    databases carried this run's slug, were owned by the test role and were absent
    before the run. They were dropped, and the role then owned zero databases.
  - Both scripts are kept in the artifact directory.
- Time: Playwright 41.7 s and 72.0 s whole harness. This is not a performance
  measurement.
- Artifacts: `/private/tmp/lp-tos-batch7-fixed`, including `SUMMARY.md`.

### Rerun diagnostics (not compatibility evidence)

All of these used fresh run-owned databases, and none are left. Protocol traces
have tokens and auth headers redacted and are kept locally only.

| Run | Fixture | Binary | Purpose | Result |
| --- | --- | --- | --- | --- |
| `lpb7d4-1017-181616` | instrumented diagnostic | `82477c9` | capture-time URL | 2 / 2 |
| `lpb7d5-1017-182029` | instrumented diagnostic | `82477c9` | `DEBUG=pw:protocol` trace | 2 / 2 |
| `lpchrb7-1017-181831` | unchanged | Chromium lane | comparison | 4 / 0 |
| `lpchrb7d-1017-181922` | instrumented diagnostic | Chromium lane | capture-time URL | 4 / 0 |
| `lpchrb7e-1017-182220` | instrumented diagnostic | Chromium lane | protocol trace | 4 / 0 |

- The Chromium-lane runs use the direct `playwright.e2e.config.ts`. Unlike the
  Lightpanda lane, this lane reuses a shared authenticated context.
- Artifacts:
  - `/private/tmp/lp-tos-batch7-{diag4,diag5,chr,chr-diag,chr-diag2}`
  - `/private/tmp/lp-pushstate-repro`: the original `repro-v1.mjs` and
    `results.txt`; `repro.mjs` with the added response-then-pushState shape and
    `results-v2.txt`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
