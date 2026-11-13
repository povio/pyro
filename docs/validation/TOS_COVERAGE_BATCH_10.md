# TOS representative coverage batch 10, 2026-10-17

Batch 10 is 13 unchanged manifest-routed scenarios from two files not covered by
batches 1–9 or the smoke runs: airlines and partner-networks master data. The
selection was recorded and reported to the parent before any run of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `82477c9` (pinned, not rebuilt) | `lpb10-1017-183306` | 13 / 0 |

The cases cover:
- **airlines:** seeded rows, details, sorting by identity and audit columns,
  create, autosave update, archive and unarchive.
- **partner networks:** seeded rows, details, sorting by name and audit columns,
  archived-status filter, create, and autosave update.

No upstream comparison was run because the rewrite had no failures to compare.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Its 116 `nativeOnly`
cases are out of scope. Batches 1–10 have selected 126 distinct `common` cases
(17.0%) in 41 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together. Smoke runs such as
profile-settings are not counted.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `c3aac06f2cd45a532ccda7e994f1cc21316d485b0e03de9884b3ac6c994a3472`.
  - It covers all manifest `common` entries for
    `master-data/business-entities/airlines` (7) and
    `master-data/business-entities/partner-networks` (6).
  - The run reported "Running 13 tests".
- Binary: the immutable `82477c9` release build (see the
  [batch 7 rerun](TOS_COVERAGE_BATCH_7.md)), SHA-256
  `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/pyro-tos-f9f-batch10`).
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `pyro_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time: Playwright 19.2 s and 59.5 s whole harness. This is not a performance
  comparison.
- No files were cleaned up for this batch.
- Artifacts: `/private/tmp/pyro-tos-batch10`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
