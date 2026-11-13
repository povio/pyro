# TOS representative coverage batch 15, 2026-10-17

Batch 15 is 12 unchanged manifest-routed scenarios from six master-data files not
covered by batches 1–14 or the smoke runs:
- commercial party templates and their duplicates;
- shipping events;
- teams persistent filters;
- the business-partner similar-partners banner;
- VAT rule applicability.

The selection was recorded and reported to the parent before any run of these
files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb15-1017-191435` | 11 / 1 |
| upstream `2ad38773` | `lpb15u-1017-191630` | 11 / 1 |

**Baseline failure on every lane:** shipping-events "sorts shipping events by
visible sortable columns", a 30 s timeout.
- On both the rewrite and upstream, the browser-error fixture recorded the same
  backend `HTTP response status 500` for
  `GET /offices/<id>/position-tracking-shipment-snapshots?…&order=-shipmentId&…`.
- On the Chromium lane (direct `playwright.e2e.config.ts`, run
  `lpchrb15-1017-191813`, comparison only), this case also times out waiting for
  the response.
  - That run also executed two shipping-events tests not routed by the manifest.
    They are excluded.
- It is recorded as a baseline failure shared by all lanes. The 500 on
  `-shipmentId` sorting points at the backend or this environment rather than a
  browser, but no root cause is established and nothing is routed.

The other 11 cases pass:
- **commercial party templates:** duplicate and document party row order,
  applying missing occurrences to positions and quotes without duplicating
  Customer, rejecting Customer in templates, and template name autosave.
- **similar-partners banner:** shown on Overview and the Business Partner tab,
  kept on the other tabs, hidden when there are none, and opening a similar
  partner.
- **teams:** opening an action drawer does not navigate back to the list.
- **VAT rule applicability:** regional bookkeeping edits.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–15 have
selected 191 distinct cases (25.7%) in 62 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `5964b90aa1649022b1c39b638c322e41fd9d24085a5ec483db909ffea1497ee6`.
  - It covers all manifest `common` entries for
    `master-data/logistics/commercial-party-template-duplicates` (4),
    `commercial-party-templates` (1), `shipping-events` (1) and
    `teams-persistent-filters` (1);
    `master-data/business-entities/business-partner-similar-banner` (4); and
    `master-data/accounting/vat-rule-applicability` (1).
  - Both routed runs reported "Running 12 tests".
- Binaries. Each was rehashed before its run.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `9f100bd` | `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406` | not rebuilt |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture:
  - The rewrite run used a fresh, clean `git worktree add --detach`
    (`/private/tmp/pyro-tos-f9f-batch15`).
  - The upstream and Chromium runs reused it to save disk space. Its e2e tree was
    verified unchanged; the only change was the harness-generated pdfgen model.
- Setup: one Playwright worker and native PostgreSQL 17 with the `pyro_bench` role
  and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After every exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb15` | 99.4 s | 58.9 s |
  | `lpb15u` | 85.4 s | 52.5 s |

  Both include the 30 s timeout.
- No files were cleaned up for this batch.
- Artifacts:
  - `/private/tmp/pyro-tos-batch15`
  - `/private/tmp/pyro-tos-batch15-up`
  - `/private/tmp/pyro-tos-batch15-chr`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
