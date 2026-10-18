# TOS representative coverage W3-03 and W3-04, 2026-10-18

These are worker 3's third and fourth batches. Each covers the whole
manifest-routed groups of files from W2's ledger `uncovered_files`
(JSON SHA-256 `3d1de5a0…`), excluding W3-01 and W3-02. Both selections were
recorded and sent, with their hashes, to W2 and the parent before any run of
their files. The selections are in
[`evidence/tos-coverage-w3/selections`](../../evidence/tos-coverage-w3/selections).

Both batches ran on the worker 3 host, using Docker PostgreSQL 17. The setup is
described in [W3-01/W3-02](TOS_COVERAGE_W3_01_02.md#pr-82-verification-on-the-w3-host-docker-postgresql-17).

| Batch | Files | Selected cases | main `6e44bcd` | upstream `2ad38773` |
| --- | --- | ---: | --- | --- |
| W3-03 | `control-tower/calendar`, `finances/uninvoiced-charges` | 13 | `w3l03-1018-063518`: 4 passed, 9 failed | `w3l03u2-1018-064043`: 4 passed, 9 failed |
| W3-04 | `platform/overview-table-config`, `configuration/roles-overview`, `master-data/office/roles-overview` | 15 | `w3l04-1018-070622`: 15 passed, 0 failed | `w3l04u-1018-070722`: 14 passed, 1 failed |

## W3-03: unresolved failures shared by every engine on this host

The same 9 cases fail on main, on upstream, and in a diagnostic Chromium run.
They are:

- all 7 `control-tower/calendar` cases;
- the 2 header-sorting cases of `finances/uninvoiced-charges`.

| Run | Engine | The 13 selected cases |
| --- | --- | --- |
| `w3l03-1018-063518` | main `6e44bcd` | 4 passed, 9 failed |
| `w3l03b-1018-065542` | main `6e44bcd`, repeat (autovacuum off, see below) | 4 passed, 9 failed |
| `w3l03u2-1018-064043` | upstream `2ad38773` | 4 passed, 9 failed |

**Chromium diagnostic comparison** (kept separate; not a coverage result):

- Run `w3l03c-1018-070233` used Playwright 1.59.1's headless Chromium shell
  (build 1217) with the direct `playwright.e2e.config.ts`. It ran 15 tests.
- On the 13 selected cases it got 4 passed and 9 failed: the same 9 cases.
- The other 2 tests are cases the manifest routes to Chromium, not Lightpanda.
  "Filters route event types…" failed and "shows the selected-charge summary…"
  passed. Both are excluded from every comparison above.

**Observed:**
- The calendar `beforeEach` waits for a `GET /calendar` response with
  `type=Container&search=CTCU` and status 200. That response is not observed
  within the 10 s test timeout on any engine
  (`forResponse: Target page, context or browser has been closed` at teardown).
- Both uninvoiced-charges sorting cases time out after opening the route. On
  every engine, the `Sort column` button of the first header to sort
  (`Sender`, or `Amount`) is never found: `toBeVisible` in the Lightpanda
  lanes, the `click` wait in Chromium.
- The other 4 uninvoiced-charges cases pass on every engine: listing and total,
  direction switch, filter/column configuration, and export.

**Not established at the time of the runs:**
- Whether these cases also fail on W2's host. They have not been run there.
- Whether the cause is in the test, the application at this TOS commit, or this
  host's setup. No backend request log was available. The manifest's Lightpanda
  verification predates this TOS commit (it was refreshed on 2026-09-24 against
  `63530b58ea`).
- The [follow-up triage](#follow-up-triage-added-after-merge) below narrows this
  down.

These failures are not lightningpanda regressions: upstream and Chromium fail
the same cases.

### Follow-up triage (added after merge)

W1 triaged the failures read-only from the TOS source, and worker 3 then made
one diagnostic run. Run `w3l03ct-1018-073301` repeated W3-03 unchanged in the
Chromium lane, with tracing enabled by the Playwright CLI flag `--trace on`,
which the runner forwards. No TOS file was changed. It gave the same result: on
the 13 selected cases, 4 passed and 9 failed.

The 15 traces stay on the W3 host and are not published, because they contain
authentication headers and localStorage. `traces.SHA256SUMS` has SHA-256
`0893ff7463e1db17756e4986cad19860f67caed5a84846ce3923880bcaab03df`.

- **Calendar (7 cases):** the type=Booking request is confirmed by trace,
  consistent with TOS `dd8659750e`.
  - In every calendar trace, the only `/calendar` API request is
    `type=Booking&search=CTCU`, answered 200. There is no `type=Container`
    request, which is what the test's `beforeEach` waits for.
  - In the TOS source at `f9fde4aa0d`, `CalendarView.tsx:269` requests
    `type: CalendarTypeEnum.Booking`. That line comes from `dd8659750e`
    (2026-09-29), after the test was last edited (2026-09-25).
- **Uninvoiced sorting (2 cases):** there is no grid named "Data table" at
  click time (trace snapshots), consistent with renderer change `79035d3e45`.
  The rendered table structure is not established.
  - The DOM snapshots after the route opens and the charges response returns
    200 contain no `role="grid"`, no `aria-label="Data table"`, no
    `TABLE`/`TH` element and no header text.
  - In the TOS source, `UninvoicedChargesTable.tsx:430` sets `renderer="dom"`
    (from `79035d3e45`, 2026-10-02). Only `CanvasTableRenderer` carries
    `role="grid"` and `aria-label="Data table"`. The test was last edited on
    2026-09-24.

### Excluded infrastructure failure and the autovacuum setting

The first upstream attempt, `w3l03u-1018-063907`, is excluded from all results.
It never ran a test: template preparation stopped when the runner's
`SELECT pg_terminate_backend(pid) … WHERE datname = '<template>'` failed with
"permission denied to terminate process".

- **Cause.** The server log shows that the other backend on the template was an
  autovacuum worker ("terminating autovacuum process due to administrator
  command"). In PostgreSQL 17 a non-superuser role cannot signal an autovacuum
  worker; PostgreSQL 18 adds `pg_signal_autovacuum_worker` for this.
- **The fix and its scope.** Autovacuum was turned off only on worker 3's
  dedicated Docker test server: container `lp-w3-pg17` on the W3 host, holding
  only disposable test databases. The command was run as the container's
  bootstrap superuser:

  ```
  docker exec -u postgres lp-w3-pg17 psql -X -c "ALTER SYSTEM SET autovacuum = off" -c "SELECT pg_reload_conf()"
  ```

  It took effect at 06:40 UTC, and `SHOW autovacuum` reports `off`.
- **Persistence.** The setting is stored in `postgresql.auto.conf` inside the
  container's data volume (`lp-w3-pg17-data`), so it persists across restarts.
  It is undone with `ALTER SYSTEM RESET autovacuum` and a reload.
- **What was not changed.** W2's native PostgreSQL and any other shared or
  native service. The TOS source and runner are also unchanged.
- **Which runs it affects.** `w3l03` ran before the change and `w3l03b` after
  it, with the same result. All later runs (`w3l03u2`, `w3l03c`, `w3l04`,
  `w3l04u`) ran with autovacuum off.
- **Cleanup.** The excluded attempt's two run-owned databases were removed by
  exact-name cleanup.

## W3-04: one upstream-only failure

Main passes all 15 cases. Upstream fails "Configuration Roles Overview tab ›
deletes a role from the overview tab and removes it from the rendered list".

```
Error: Expected browser errors were not observed:
- deleted role details refetch: observed 0/1
```

This is upstream-only in a single run. Main does not fail it, so it is not a
rewrite regression. No upstream cause is claimed.

## Selection coverage

W2's ledger records 241 selected cases in 70 files, of 742 cases in 141 files.
W3-01 to W3-04 add 54 cases in 9 more files. These four batches ran on main
`6e44bcd`. Later W3 batches use a newer main, and their results are kept
separate.

These are selected cases, not passing cases. Each binary's results stand on
their own and must not be added to another binary's results.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
  - Manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
  - Before every run, the manifest was rehashed and the fixture's `apps/fe/e2e`
    tree was checked unchanged.
- **Selections:**

  | Batch | SHA-256 | Playwright |
  | --- | --- | --- |
  | W3-03 | `64bd3f7379be19a0bec6c0a14ec230f3074dcc2a71e27e3f17c15a0d3a442543` | "Running 13 tests"; "Running 15" in the Chromium lane |
  | W3-04 | `ff49bf247f076185cf0bb0c52a537e4f43b3358b7e3c50426c27f2dfb9a71c3d` | "Running 15 tests" |

- **Binaries:**

  | Binary | SHA-256 | Origin |
  | --- | --- | --- |
  | main `6e44bcd` | `b1d041fbd8a85b47c045556a478e486a16cba456f67df64a0566304d83c04923` | fresh clone and target on the W3 host |
  | upstream `2ad38773` | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | W2's existing build, copied read-only |

  - The `6e44bcd` binary was built with `CARGO_INCREMENTAL=0`,
    `--locked --release` and SDK 26.5. It is ad hoc signed with no
    entitlements.
  - The upstream binary is the same file W2 uses: it was transferred as a
    compressed copy, verified chunk by chunk and in full, and the hash
    matches. It is ad hoc signed and reports
    `1.1.0-nightly.10716+2ad38773f`.
- **Fixture:** one fresh worktree at the TOS commit
  (`/private/tmp/lp-w3-lfix-1`), then `bun install --frozen-lockfile`. All runs
  reused it.
- **Setup:** the W3 host setup.
  - Docker PostgreSQL 17.11 runs in Colima, with a dedicated `LOGIN CREATEDB`
    non-superuser test role.
  - Authentication is the backend test token in localStorage. Assertions are
    unchanged.
  - Template preparation falls back to a Prisma reset (828 migrations), then
    seeds, as on W2's host.
- **Database cleanup:** each run's owned databases were dropped by exact name.
  The test role then owned 0 databases.
- **Time:** whole harness, not a performance comparison.

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `w3l03` | 135.5 s | 1.8 min |
  | `w3l03b` | 136.1 s | 1.8 min |
  | `w3l03u2` | 179.8 s | 2.6 min |
  | `w3l03c` | 149.2 s | 2.1 min |
  | `w3l04` | 58.8 s | 33.4 s |
  | `w3l04u` | 55.5 s | 29.9 s |

- **Artifacts:** on the W3 host in `/private/tmp/lp-w3-l03`, `-l03u`, `-l03u2`,
  `-l03b`, `-l03c`, `-l04` and `-l04u`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
