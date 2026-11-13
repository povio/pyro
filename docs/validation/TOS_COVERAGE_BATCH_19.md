# TOS representative coverage batch 19, 2026-10-17

Batch 19 is all 10 unchanged manifest-routed scenarios of one master-data
locations file not covered by batches 1–18 or the smoke runs: ports. The
selection was recorded and reported to the parent before any run of this file.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `9f100bd` (pinned, not rebuilt) | `lpb19-1017-193725` | 6 / 4 |
| upstream `2ad38773` | `lpb19u-1017-193845` | 10 / 0 |

**Unresolved, timing-sensitive rewrite failures.** All four fail the same
assertion: `getInputOrTextareaByValue` finds no field (`fieldIndex` -1).

| Case | `lpb19` | `lpb19r` | `lpb19t` |
| --- | --- | --- | --- |
| autosaves port name field | failed | failed | failed |
| autosaves port zip field | failed | failed | passed |
| autosaves several port fields in a row | failed | failed | failed |
| autosaves clearing port district | failed | passed | failed |

The repeat (`lpb19r-1017-193956`) and the protocol-traced run
(`lpb19t-1017-194051`, `DEBUG=pw:protocol`) are diagnostic only, not compatibility
evidence. Both used the unchanged file.

**Observed:**
- `updateAutosaveTextField` first looks for `input[name=…]`/`textarea[name=…]` in
  the form. If there is none, it locates the field by its current value, reading
  every input's value once right after the details response.
- In the traced run, the port's `name` input has no `name` attribute. At that
  read, the form's 8 input/textarea values were all `""`, though the form had
  rendered. Earlier reads in the same run show the seeded values, so value
  reading itself works.
- Upstream passes all 10 cases on the same fixture.

**Not established:**
- No mechanism is proven. The read coming before the form is populated is
  consistent with timing sensitivity, as in the
  [batch 16](TOS_COVERAGE_BATCH_16.md) case. Neither a shared cause nor a
  test-only cause is proven.
- No spec or protocol violation is established. No artificial event delay was
  added, and the tests and assertions are unchanged.

The 6 passing cases cover sorting, adding a port, and matchCode, street,
district and city autosave.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–19 have
selected 241 distinct cases (32.5%) in 70 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `fafb77a6fc72032ea4234a3384a57adaadc0e6c01c83f546838fa5c9f56477a3`.
  - It covers all 10 manifest `common` entries for `master-data/locations/ports`.
  - Every run reported "Running 10 tests".
- Binaries. Each was rehashed before its run.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `9f100bd` | `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406` | not rebuilt |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture:
  - The batch run used a fresh, clean `git worktree add --detach`
    (`/private/tmp/pyro-tos-f9f-batch19`).
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
  | `lpb19` | 55.1 s | 17.7 s |
  | `lpb19u` | 57.6 s | 26.0 s |

- No files were cleaned up for this batch.
- Artifacts:
  - `/private/tmp/pyro-tos-batch19`
  - `/private/tmp/pyro-tos-batch19-up`
  - `/private/tmp/pyro-tos-batch19-repeat`
  - `/private/tmp/pyro-tos-batch19-trace`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
