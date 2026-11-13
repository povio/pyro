# TOS representative coverage batch 17, 2026-10-17

Batch 17 is 14 unchanged manifest-routed scenarios from four master-data files not
covered by batches 1–16 or the smoke runs:
- accounting dunning systems;
- business-partner documents, employee roles and employees.

The selection was recorded and reported to the parent before any run of these
files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb17-1017-193035` | 14 / 0 |

The cases cover:
- **dunning systems:** canvas-table listing, details, sorting by name, created
  date and default flag, diacritics and normalized-text search, create, autosave,
  duplicate-name and duplicate-default validation, and archive and unarchive.
- **business partner:** the empty Documents state, the empty Employee Roles table,
  and the Employees create-drawer labels.

No upstream comparison was run because the rewrite had no failures to compare.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–17 have
selected 219 distinct cases (29.5%) in 68 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `a73dd44030a9992186b24c3cd8174d4d62912cd28919a42f60ea46e1866096f9`.
  - It covers all manifest `common` entries for
    `master-data/accounting/dunning-systems` (11) and
    `master-data/business-entities/business-partner-documents` (1),
    `business-partner-employee-roles` (1) and `business-partner-employees` (1).
  - The run reported "Running 14 tests".
- Binary: the immutable `9f100bd` release build, SHA-256
  `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/pyro-tos-f9f-batch17`).
  The manifest hash was verified after install.
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `pyro_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time: Playwright 20.0 s and 56.4 s whole harness. This is not a performance
  comparison.
- No files were cleaned up for this batch.
- Artifacts: `/private/tmp/pyro-tos-batch17`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
