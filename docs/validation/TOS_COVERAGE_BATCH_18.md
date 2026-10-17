# TOS representative coverage batch 18, 2026-10-17

Batch 18 is all 12 unchanged manifest-routed scenarios of one master-data
locations file not covered by batches 1–17 or the smoke runs: depots. The
selection was recorded and reported to the parent before any run of this file.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb18-1017-193257` | 12 / 0 |

The cases cover:
- **sorting:** by identity, address and audit columns;
- **autosave:** each of matchCode, name, shortName, additionalInformation, street,
  zip and district;
- **clearing:** shortName, additionalInformation and district;
- **several fields edited in a row.**

No upstream comparison was run because the rewrite had no failures to compare.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–18 have
selected 231 distinct cases (31.1%) in 69 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `e85da7e13d2af11eed8b11dac5250d8485bc271e9467cd179d1225f14a750b38`.
  - It covers all 12 manifest `common` entries for
    `master-data/locations/depots`.
  - The run reported "Running 12 tests".
- Binary: the immutable `9f100bd` release build, SHA-256
  `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406`. It was
  rehashed before the run and not rebuilt.
- Fixture: a fresh, clean `git worktree add --detach` (`/private/tmp/lp-tos-f9f-batch18`).
  The manifest hash was verified after install.
- Setup: frozen install, one Playwright worker, and native PostgreSQL 17 with the
  `lp_bench` role and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After the exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time: Playwright 16.7 s and 54.5 s whole harness. This is not a performance
  comparison.
- No files were cleaned up for this batch.
- Artifacts: `/private/tmp/lp-tos-batch18`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
