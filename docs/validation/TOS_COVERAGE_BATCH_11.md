# TOS representative coverage batch 11, 2026-10-17

Batch 11 is 14 unchanged manifest-routed scenarios from two files not covered by
batches 1–10 or the smoke runs: checklist items and checklist templates master
data. The selection was recorded and reported to the parent before any run of
these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `82477c9` (pinned, not rebuilt) | `lpb11-1017-183624` | 14 / 0 |

Both files cover the same flows: seeded rows and details, sorting by name and
audit columns, create, autosave update, duplicate-name validation, and archive and
unarchive.

No upstream comparison was run because the rewrite had no failures to compare.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–11 have
selected 140 distinct cases (18.9%) in 43 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `5b134898e5b2069e1c75a321ee302b27b95d975a8d9522768a17389209eeb6b9`.
  - It covers all manifest `common` entries for
    `master-data/logistics/checklist-items` (7) and
    `master-data/logistics/checklist-templates` (7).
  - The run reported "Running 14 tests".
- Binary: the immutable `82477c9` release build (see the
  [batch 7 rerun](TOS_COVERAGE_BATCH_7.md)), SHA-256
  `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/pyro-tos-f9f-batch11`).
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `pyro_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time: Playwright 21.1 s and 59.5 s whole harness. This is not a performance
  comparison.
- Disk cleanup:
  - Before this batch, the parent approved removing only `deps`, `build`,
    `incremental` and `.fingerprint` under `target/release` of three completed
    clones: `main-8a50785`, `b5fix-89ef88f` and `bloburl-92a679b`.
  - The removal was guarded fail-closed by exact-executable process checks and
    symlink checks. All 7 retained binary SHA-256s were unchanged.
  - The audit is in `/private/tmp/pyro-tos-cleanup-plan-1017d`.
- Artifacts: `/private/tmp/pyro-tos-batch11`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
