# TOS representative coverage batch 16, 2026-10-17

Batch 16 is 14 unchanged manifest-routed scenarios from two master-data files not
covered by batches 1–15 or the smoke runs: accounting dunning levels and
business-partner accounts. The selection was recorded and reported to the parent
before any run of these files.

| Binary | Run | Cases | Passed / failed |
| --- | --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb16-1017-192408` | 14 | 13 / 1 |
| upstream `2ad38773` | `lpb16u-1017-192520` | 14 | 14 / 0 |

**Unresolved, timing-sensitive rewrite failure:** business-partner-accounts
"deletes an account line from the Accounts tab table". Upstream passes this case.
- After the details and bookkeeping-mappings responses arrive, the test reads
  every visible row input's value once, through `getInputOrTextareaByValue`
  inside `rowWithText`.
- That helper calls `expect(fieldIndex).toBeGreaterThanOrEqual(0)` directly. A
  failure there aborts the surrounding `expect.poll` rather than retrying it.
- On the rewrite, the seeded `E2E-BP-ACC-DELETE-DEBTOR-…` value was not among the
  row inputs at that moment, and the test failed in under 10 s.
- Additional runs of the unchanged file on the same binary (diagnostic only, not
  compatibility evidence):

  | Run | Tracing | Result |
  | --- | --- | --- |
  | `lpb16r-1017-192636` | none | failed again (4/5) |
  | `lpb16t-1017-192734` | `DEBUG=pw:protocol` | passed 5/5 |

  The pass under `DEBUG=pw:protocol` supports timing sensitivity. It does not
  prove a specific mechanism; client slowdown is not established as the cause.
- The shape resembles the batch 7 credit-note cases, which upstream also passed:
  the test reads the page right after a network response. A shared cause is not
  proven, and neither is a test-only cause.
- No spec or protocol violation is established. No artificial event delay was
  added, and the tests and assertions are unchanged. It is recorded as an
  unresolved, timing-sensitive rewrite failure.

The other 13 cases pass:
- **dunning levels:** list, details, sorting by identity, amounts, related system,
  audit and days overdue, the create drawer, fee display, duplicate isolation, and
  archive and unarchive.
- **business-partner accounts:** seeded mappings, the Accounts tab visual, adding
  a line, and debtor/creditor autosave with the factoring checkbox.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–16 have
selected 205 distinct cases (27.6%) in 64 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `df59d44362010b7142a3e0891ac91d9e42fa92956b8e8287157c39db7d5b6f0d`.
  - It covers all manifest `common` entries for
    `master-data/accounting/dunning-levels` (9) and
    `master-data/business-entities/business-partner-accounts` (5).
  - Both routed batch runs reported "Running 14 tests".
- Binaries. Each was rehashed before its run.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `9f100bd` | `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406` | not rebuilt |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture:
  - The batch run used a fresh, clean `git worktree add --detach`
    (`/private/tmp/pyro-tos-f9f-batch16`).
  - The upstream, repeat and trace runs reused it. Its e2e tree was verified
    unchanged; the only change was the harness-generated pdfgen model.
- Setup: one Playwright worker and native PostgreSQL 17 with the `pyro_bench` role
  and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After every exact-name run-owned cleanup, the test role owned zero databases.
  - The trace log was scrubbed of tokens and auth headers and is kept locally.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb16` | 59.9 s | 23.0 s |
  | `lpb16u` | 47.5 s | 15.1 s |

- Disk cleanup: before this batch, the parent approved removing only
  `target/debug/incremental` of the primary checkout. That freed about 5.6 GiB.
  Retained binary and handoff hashes were unchanged; the audit is in
  `/private/tmp/pyro-tos-cleanup-plan-1017g`.
- Artifacts:
  - `/private/tmp/pyro-tos-batch16`
  - `/private/tmp/pyro-tos-batch16-up`
  - `/private/tmp/pyro-tos-batch16-repeat`
  - `/private/tmp/pyro-tos-batch16-trace`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
