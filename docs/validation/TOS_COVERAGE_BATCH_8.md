# TOS representative coverage batch 8, 2026-10-17

Batch 8 is 13 unchanged manifest-routed scenarios from three files not covered by
batches 1–7 or the smoke runs: accounting currencies, logistics departments and AWB
stock master data. The selection was recorded before any run of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `82477c9` (pinned, not rebuilt) | `lpb8-1017-182750` | 13 / 0 |

The cases cover:
- **currencies:** seeded list rows, sorting by identity and by audit columns, and
  setup-created and setup-updated currencies.
- **departments:** case-insensitive duplicate error, sequential autosave retained
  after reload, archive and unarchive through confirmation, cancelled archive, and
  archive rejection while employees are assigned.
- **AWB stock:** comment autosave, and archive/unarchive through FE controls.

No upstream comparison was run because the rewrite had no failures to compare.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `857b3ac6f5711374f6dc9715b5b860f765c452fb53c4edaebaad365d6265b644`.
  - It covers all manifest `common` entries for
    `master-data/accounting/currencies` (5), `master-data/logistics/departments` (6)
    and `master-data/logistics/awb-stock` (2).
  - The manifest was read from the existing batch 7 fixture at the same TOS commit,
    with its hash verified.
  - The run reported "Running 13 tests".
- Binary: the immutable release build of integrated main `82477c9` from the
  [batch 7 rerun](TOS_COVERAGE_BATCH_7.md), SHA-256
  `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/lp-tos-f9f-batch8`).
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `lp_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - The cleanup script also accepts the run's exact base template name (see the
    batch 7 rerun). After cleanup the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time: Playwright 20.9 s and 59.7 s whole harness. This is not a performance
  comparison.
- Disk cleanup:
  - Before the fixture was created, the parent approved removing only `deps`,
    `build`, `incremental` and `.fingerprint` under the merged PR #54 validation
    targets.
  - All 7 retained binary SHA-256s were unchanged.
  - The audit is in `/private/tmp/lp-tos-cleanup-plan-1017c`. It also records that
    the pre-removal process guard did not abort: its count matched macOS
    `.bundle` system processes. A stricter check found no active processes.
- Artifacts: `/private/tmp/lp-tos-batch8`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
