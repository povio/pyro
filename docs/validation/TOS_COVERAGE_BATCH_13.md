# TOS representative coverage batch 13, 2026-10-17

Batch 13 is 13 unchanged manifest-routed scenarios from four master-data office
files not covered by batches 1–12 or the smoke runs: office accounts, basic,
contacts and overview. The selection was recorded and reported to the parent
before any run of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb13-1017-190323` | 13 / 0 |

This is the first batch on the `9f100bd` binary. Batches 8–12 used `82477c9`, and
the two sets of results are kept separate.

The cases cover:
- **Accounts tab:** seeded rows, adding a minimal line, debtor/creditor autosave,
  and delete.
- **Contacts tab:** add, update and delete.
- **Basic tab:** values, VAT update and EORI update.
- **Overview:** details modal, field autosave, and lock/unlock.

No upstream comparison was run because the rewrite had no failures to compare.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–13 have
selected 166 distinct cases (22.4%) in 49 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `865a045251d1e1fc79d506e1be5193a078c18045f0babbc9cacae708cd41bb71`.
  - It covers all manifest `common` entries for
    `master-data/business-entities/office-accounts` (4), `office-basic` (3),
    `office-contacts` (3) and `office-overview` (3).
  - The run reported "Running 13 tests".
- Binary: the immutable integrated-main `9f100bd` release build from the
  [batch 12 docubox rerun](TOS_COVERAGE_BATCH_12.md), SHA-256
  `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/lp-tos-f9f-batch13`).
  The manifest hash was verified after install.
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `lp_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time: Playwright 26.9 s and 63.2 s whole harness. This is not a performance
  comparison.
- Disk cleanup:
  - Before this batch, the parent approved option A1 only: removing `deps`,
    `build`, `incremental` and `.fingerprint` under `target/release` of the
    superseded pinned clones `main-b0f0319` and `main-43f9415`.
  - Exactly 8 paths were removed, about 1.3 GiB.
  - The removal used fail-closed exact-executable process checks and symlink
    checks, and all 8 retained binary SHA-256s were unchanged.
  - The audit is in `/private/tmp/lp-tos-cleanup-plan-1017e`.
- Artifacts: `/private/tmp/lp-tos-batch13`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
