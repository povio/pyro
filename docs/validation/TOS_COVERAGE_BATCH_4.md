# TOS representative coverage batch 4, 2026-10-17

Batch 4 is 13 unchanged manifest-routed scenarios from seven files not covered
by batches 1–3. Its selection was recorded before any run of these files. It
covers:
- the to-dos route;
- projects (read-only user, packing-list position opened in a new tab);
- control-tower shipments;
- the business-partner contacts table;
- the entity chat widget;
- teams master data;
- the positions default filter template.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `4894b3b` | `lpb4-1017-154734` | 7 / 6 |
| upstream `2ad38773` | `lpb4u-1017-154946` | 9 / 4 |
| `c8e80ab` (PR #48, first head) | `lpb4o-1017-161145` | 9 / 4 |
| `11116ea` (PR #48 approved head) | `lpb4p-1017-162309` | 9 / 4 |

**Rewrite-specific failure, fixed in PR #48:** positions-default-filter-template
(both cases).
- The spec's `context.addInitScript` writes `window.localStorage`.
- In the Lightpanda lane (`E2E_FRESH_AUTH_CONTEXT=1`), `context.newPage()` starts
  on a CDP-created `about:blank` with an opaque origin, and init scripts run
  there.
- The rewrite threw SecurityError, and the browser-error fixture recorded the
  pageerror at `about:blank`.
- Upstream (`Window.zig` `bucketForOrigin`) gives every origin key a bucket and
  never throws.
- With the parent's approval, PR #48 gives each opaque-origin document its own
  ephemeral storage. This is a bounded, documented deviation from Chrome. After
  the fix, the rewrite matches upstream at 9/13.

A diagnostic spec added in a separate worktree showed that Chromium also throws
SecurityError for this init script on `about:blank` and reports the pageerror
(`/private/tmp/lp-tos-diag15c`, `diag16c`).
- Caveat: the spec imported Playwright's `test` directly, bypassing the TOS
  browser fixture. Its "Lightpanda" runs (`diag15l`, `diag16l`) therefore also
  ran Chromium and are not rewrite evidence.
- The real spec passes on Chromium (direct config, run `lpchrb4-1017-155525`)
  because that lane reuses a shared authenticated context instead of opening a
  fresh `about:blank` page.

**Shared failures on rewrite, upstream and Chromium.** No root cause is
established for these:
- entity-chat-widget: "recovers after context retries exhaust a persistent
  server error" and "shows a nonterminal unauthorized context error and recovers
  on explicit retry" (10 s timeouts).
- control-tower shipments: "opens a booking's container from the Containers tab"
  (timeout).
- todos: "opens To-Dos from the due-date ordered URL". The canvas or grid table is
  not found, as in the batch 1 platform canvas case.

The Chromium run `lpchrb4` used the direct `playwright.e2e.config.ts` on the four
affected files. It also ran tests from those files outside the manifest
selection; those results are not part of this batch. It failed all four shared
cases listed above and passed both filter-template cases.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `2738e319099f08d62c78026f930edc5f738c27d58d952f5b1ab80ca33b80e337`.
  - It covers all manifest `common` entries for `todos/todos`,
    `projects/projects`, `control-tower/shipments`,
    `master-data/business-entities/business-partner-contacts`,
    `chat/entity-chat-widget`, `master-data/logistics/teams` and
    `logistics/positions-default-filter-template`.
  - Every routed run reported "Running 13 tests".
- Binaries: each rewrite binary is a release build from a fresh `--no-hardlinks`
  clone with its own target directory. Every hash was verified before execution.

  | Binary | SHA-256 |
  | --- | --- |
  | `4894b3b` | `1eeed21fa8cb8ce53152c92928e40ebb0de0fd8997e58b734f81962c396951d9` |
  | `c8e80ab` | `9ffb58fb686042ecaa7f18a6306d9b7ed833b98f950b5afb04a5e5bd2aef451f` |
  | `11116ea` | `3a134b268d18fd70080dcc028e120400fe6f0db77fb83b71a5f24a2cfd4fd9dc` |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` |

- Chromium: Playwright 1.59.1 chromium-headless-shell 147.0.7727.15.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Setup: one Playwright worker, native PostgreSQL 17 with the `lp_bench` test
  role, and a per-run slug and template database. Each run used a fresh clean
  fixture; afterwards only the harness-generated pdfgen model had changed. After
  cleanup the test role owned zero databases.
- Time: 103.0–121.3 s whole harness and 65.4–81.2 s Playwright per routed run.
  This is not browser-only and not a performance comparison.
- The first `c8e80ab` build failed with ENOSPC, filling the disk. The parent then
  removed only three named disposable build directories. The successful build
  and every listed run happened after that.

Artifacts:
- `/private/tmp/lp-tos-batch4{,-up,-opaque,-opaque2}`
- `/private/tmp/lp-tos-chrdft`
- `/private/tmp/lp-tos-diag15c`, `diag15l`, `diag16c`, `diag16l`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
