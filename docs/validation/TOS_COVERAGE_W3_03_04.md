# TOS representative coverage W3-03 and W3-04, 2026-10-18

These are worker 3's third and fourth batches. Each covers the whole
manifest-routed groups of files from W2's ledger `uncovered_files`
(JSON SHA-256 `3d1de5a0…`), excluding W3-01 and W3-02. Both selections were
recorded and sent, with their hashes, to W2 and the parent before any run of
their files. The selections are in
[`evidence/tos-coverage-w3/selections`](../../evidence/tos-coverage-w3/selections).

Both batches ran on the worker 3 host, using Docker PostgreSQL 17. The setup is
described in [W3-01/W3-02](TOS_COVERAGE_W3_01_02.md#pr-82-verification-on-the-w3-host-docker-postgresql-17).

| Batch | Files | Cases | main `6e44bcd` | upstream `2ad38773` |
| --- | --- | ---: | --- | --- |
| W3-03 | `control-tower/calendar`, `finances/uninvoiced-charges` | 13 | `w3l03-1018-063518`: 4 / 9 | `w3l03u2-1018-064043`: 4 / 9 |
| W3-04 | `platform/overview-table-config`, `configuration/roles-overview`, `master-data/office/roles-overview` | 15 | `w3l04-1018-070622`: 15 / 0 | `w3l04u-1018-070722`: 14 / 1 |

Results are shown as passed / failed.

## W3-03: unresolved failures shared by every engine on this host

The same 9 cases fail on main, on upstream, and in a diagnostic Chromium run.
They are:

- all 7 `control-tower/calendar` cases;
- the 2 header-sorting cases of `finances/uninvoiced-charges`.

| Run | Engine | Selected cases: passed / failed |
| --- | --- | --- |
| `w3l03-1018-063518` | main `6e44bcd` | 4 / 9 |
| `w3l03b-1018-065542` | main `6e44bcd`, repeat (autovacuum off, see below) | 4 / 9 |
| `w3l03u2-1018-064043` | upstream `2ad38773` | 4 / 9 |
| `w3l03c-1018-070233` | Chromium (Playwright 1.59.1 headless shell 1217), `playwright.e2e.config.ts`, diagnostic | 4 / 9 |

The Chromium run also ran 2 cases that the manifest routes to Chromium, not
Lightpanda: "filters route event types…" failed, and "shows the selected-charge
summary…" passed. Both are excluded from the comparison.

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

**Not established:**
- Whether these cases also fail on W2's host. They have not been run there.
- Whether the cause is in the test, the application at this TOS commit, or this
  host's setup. No backend request log is available. The manifest's Lightpanda
  verification predates this TOS commit (it was refreshed on 2026-09-24 against
  `63530b58ea`).
- No cause is claimed.

These failures are not lightningpanda regressions: upstream and Chromium fail
the same cases.

### Excluded infrastructure failure

The first upstream attempt, `w3l03u-1018-063907`, never ran a test. Template
preparation stopped when the runner's
`SELECT pg_terminate_backend(pid) … WHERE datname = '<template>'` failed with
"permission denied to terminate process".

- The server log shows that the other backend on the template was an
  autovacuum worker ("terminating autovacuum process due to administrator
  command"). In PostgreSQL 17 a non-superuser cannot signal an autovacuum
  worker; PostgreSQL 18 adds `pg_signal_autovacuum_worker` for this.
- Autovacuum on this test-only server is therefore off
  (`ALTER SYSTEM SET autovacuum = off`) for all later runs, from 06:40 UTC.
- `w3l03` ran before the change and `w3l03b` after it, with the same result.
- The failed attempt's two run-owned databases were removed by exact-name
  cleanup.

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
W3-01 to W3-04 add 54 cases in 9 more files.

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
