# TOS representative coverage batch 14, 2026-10-17

Batch 14 is 13 unchanged manifest-routed scenarios from seven master-data
business-entity files not covered by batches 1–13 or the smoke runs: office,
cargo agents (list, basic, accounts, documents), and business partner overview
and basic. The selection was recorded and reported to the parent before any run
of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb14-1017-190709` | 12 / 1 |
| upstream `2ad38773` | `lpb14u-1017-190848` | 12 / 1 |

**Shared failure on rewrite and upstream:** office "lists seeded offices as
rendered table rows".
- Both browsers fail the same assertion: the row's `innerText` never contains
  "Business Partner". The received text is identical on both. The ID, VAT,
  address, debtor/creditor, remark and audit columns render; the office-types
  value does not.
- The test then also expects the `types` header to be at least 320 px wide.
- On the Chromium lane (direct `playwright.e2e.config.ts`, run
  `lpchrb14-1017-191013`), this case passes.
  - That run also executed the other office tests not routed by the manifest; they
    are not part of this batch.
  - Unlike the Lightpanda lane, the Chromium lane reuses a shared authenticated
    context.
- It is recorded as a shared Lightpanda-lane baseline failure, not a
  rewrite regression. No root cause is established. It may involve layout-width
  measurement of the types cell, but that has not been verified.

The other 12 cases pass:
- **office:** create, archived filter, and debtor/creditor ID filters.
- **cargo agents:** list, create, the Basic tab, adding and deleting an account
  line, and the empty Documents state.
- **business partner:** the Overview drawer with identifiers and address, a
  different B/L address with identifier autosave, Overview lock/unlock, and the
  Basic tab render.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–14 have
selected 179 distinct cases (24.1%) in 56 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `c2786b4c134c0fe68a32bdbcb36d6b3f7bdd5c77762f12e040653db0e0aafda3`.
  - It covers all manifest `common` entries for the seven files under
    `master-data/business-entities/`: `office` (4), `cargo-agents` (2),
    `cargo-agents-basic` (1), `cargo-agent-accounts` (1),
    `cargo-agent-documents` (1), `business-partner-overview` (3) and
    `business-partner-basic` (1).
  - Both routed runs reported "Running 13 tests".
- Binaries. Each was rehashed before its run.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `9f100bd` | `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406` | not rebuilt |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture:
  - The rewrite run used a fresh, clean `git worktree add --detach`
    (`/private/tmp/lp-tos-f9f-batch14`).
  - The upstream and Chromium runs reused it to save disk space. Its e2e tree was
    verified unchanged; the only change was the harness-generated pdfgen model.
- Setup: one Playwright worker and native PostgreSQL 17 with the `lp_bench` role
  and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After every exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb14` | 81.5 s | 39.6 s |
  | `lpb14u` | 72.6 s | 40.2 s |

  Both include the 10 s polling timeouts.
- No files were cleaned up for this batch.
- Artifacts:
  - `/private/tmp/lp-tos-batch14`
  - `/private/tmp/lp-tos-batch14-up`
  - `/private/tmp/lp-tos-batch14-chr`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
