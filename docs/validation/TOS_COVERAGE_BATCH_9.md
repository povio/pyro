# TOS representative coverage batch 9, 2026-10-17

Batch 9 is 13 unchanged manifest-routed scenarios from four files not covered by
batches 1–8 or the smoke runs: accounting charge types, cargo types, route
templates and the employees archived filter. The selection was recorded and
reported to the parent before any run of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `82477c9` (pinned, not rebuilt) | `lpb9-1017-183034` | 13 / 0 |

The cases cover:
- **charge types and cargo types:** backend label search, sorting by identity,
  dimension, capacity and audit columns, autosave updates, duplicate validation
  from the real backend, and translation autosave.
- **route templates:** name autosave, explicit URL filters overriding saved ones,
  and restoring page filters from localStorage.
- **employees:** the visible archived filter.

No upstream comparison was run because the rewrite had no failures to compare.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `ead7426e014673fc02f5802c62f60c265c0c79dbec0ec0bea17244dc7bfd0e6b`.
  - It covers all manifest `common` entries for
    `master-data/accounting/charge-types` (4), `master-data/cargo/cargo-types` (5),
    `master-data/logistics/route-templates` (3) and
    `configuration/employees-archived-filter` (1).
  - None of these files appear in an earlier selection or validation doc.
  - The run reported "Running 13 tests".
- Binary: the immutable `82477c9` release build (see the
  [batch 7 rerun](TOS_COVERAGE_BATCH_7.md)), SHA-256
  `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/pyro-tos-f9f-batch9`).
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `pyro_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time: Playwright 18.4 s and 54.7 s whole harness. This is not a performance
  comparison.
- No files were cleaned up for this batch.
- Artifacts: `/private/tmp/pyro-tos-batch9`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
