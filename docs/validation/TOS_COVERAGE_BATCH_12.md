# TOS representative coverage batch 12, 2026-10-17

Batch 12 is 13 unchanged manifest-routed scenarios from two files not covered by
batches 1–11 or the smoke runs: DocuBox templates and integration channels
master data. The selection was recorded and reported to the parent before any run
of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `82477c9` (pinned, not rebuilt) | `lpb12-1017-183856` | 12 / 1 |
| upstream `2ad38773` | `lpb12u-1017-184014` | 13 / 0 |

The single failure is rewrite-specific: docubox-templates "lists seeded DocuBox
templates as HTML rows and opens the nested folder details".
- The test calls `page.keyboard.press("ArrowRight")` to expand a folder.
- On macOS, Playwright 1.59.1 attaches Mac editing `commands` to key events, here
  `["moveRight"]`. On Linux it sends none.
- lp-cdp rejected any non-empty `commands` with "Input editing commands … not
  supported".
- A minimal repro (`/private/tmp/lp-keypress-repro`) shows ArrowRight, ArrowLeft
  and Home rejected on the rewrite but accepted on upstream. Enter and printable
  keys work on both.
- The fix belongs to W3 (lp-cdp) and has been routed there.

After W3's fix, the unchanged docubox-templates file will be rerun. Accepting
`commands` would only stop rejecting Playwright's macOS key events. It would not
claim broad keyboard or editing-command support.

**Separate known WebAPI gap, not in the TOS path.** While building the repro,
`page.setContent` failed on the rewrite with "document.write is not supported",
because Playwright implements `setContent` with `document.write`. The repro
served its page over HTTP instead. This gap is recorded but deferred, and no
implementation is part of this increment.

The other 12 cases pass:
- **integration channels:** canvas-table listing and details, sorting by name,
  last-polled, created and updated dates, diacritics-insensitive search,
  archived filter, autosave, and archive.
- **DocuBox templates:** sorting, archived filter, create with a root folder,
  autosave, and duplicate validation.

## Selection coverage so far

The routing manifest routes 742 `common` cases in 141 files. Batches 1–12 have
selected 153 distinct cases (20.6%) in 45 files.

This counts selected cases only, not passing cases. Batches ran on different
binaries, so their results must not be added together.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `f5ebbffb766d7c00b960b20e67a2b4609fb918a00bf7e487901194afd3a27422`.
  - It covers all manifest `common` entries for
    `master-data/logistics/docubox-templates` (6) and
    `master-data/logistics/integration-channels` (7).
  - Both runs reported "Running 13 tests".
- Binaries. Each was rehashed before its run.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | `82477c9` | `a70a6b5590d555f947d16afbedaeaf62118b27aaebb60235e16832101ecbc8d9` | immutable release build (see the [batch 7 rerun](TOS_COVERAGE_BATCH_7.md)), not rebuilt |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixture:
  - The rewrite run used a fresh, clean `git worktree add --detach`
    (`/private/tmp/lp-tos-f9f-batch12`).
  - The upstream run reused it to save disk space. Its e2e tree was verified
    unchanged; the only change was the harness-generated pdfgen model.
- Setup: one Playwright worker and native PostgreSQL 17 with the `lp_bench` role
  and a per-run slug and template database.
  - Backend test token into localStorage, not UI login.
  - The same direct-lane version-gate bypass as earlier batches.
  - After every exact-name run-owned cleanup, the test role owned zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB, macOS. This matters here: the
  failure depends on Playwright's macOS key mapping.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb12` | 55.9 s | 18.8 s |
  | `lpb12u` | 44.0 s | 11.9 s |

- No files were cleaned up for this batch.
- Artifacts:
  - `/private/tmp/lp-tos-batch12`
  - `/private/tmp/lp-tos-batch12-up`
  - `/private/tmp/lp-keypress-repro`

## Rerun of the docubox-templates file on integrated main `9f100bd`

Integrated main `9f100bd0b7dcbb62a77f70dfcaa823d8dc67717b` contains W3's PR #66
(approved head `54decd9`).
- That fix accepts macOS key `commands` only when they are exactly what Playwright
  derives from the key and the engine already performs that edit, for example
  `moveRight`.
- Other commands are still refused explicitly. These include Home/End, word and
  line moves, and clipboard keys.

| Binary | Run | Cases | Passed / failed |
| --- | --- | --- | --- |
| `9f100bd` | `lpb12k-1017-185734` | docubox-templates, all 6 | 6 / 0 |

- The earlier failure, "lists seeded DocuBox templates as HTML rows and opens the
  nested folder details", now passes.
- This rerun covers only the docubox-templates file. It is separate from the
  12/13 and 13/13 runs above, and no full-batch result is claimed.
- The minimal key repro on `9f100bd`
  (`/private/tmp/lp-keypress-repro/results-9f100bd.txt`) shows ArrowRight,
  ArrowLeft, Enter and `a` accepted, and Home still refused. No support for
  unsupported keys is claimed.

Rerun provenance:
- Source: fresh `--no-hardlinks` clone, detached and clean; tree
  `7c2b1c7c11cfb0a3b58b080743f26518136072f7`; no V8.
- Release binary: SHA-256
  `3da6500e6f7f8f14589875998d2d8b0531206dc535fd62cab033be7550cd2406`, built with
  its own target directory and rehashed before the run.
- Fixture: to save disk space, the rerun reused the batch 12 fixture
  `/private/tmp/lp-tos-f9f-batch12` at `f9fde4aa0d`.
  - Its e2e tree was verified unchanged and the manifest hash matched.
  - The only change was the harness-generated pdfgen model.
- Databases: fresh per-run slug and template. After the exact-name run-owned
  cleanup, the test role owned zero databases.
- Time: Playwright 9.5 s and 37.9 s whole harness. This is not a performance
  comparison.
- Artifacts: `/private/tmp/lp-tos-batch12-keyfix`, including `rerun-subset.json`
  (6 of the 13 selected cases).
- The pinned `82477c9` binary and the earlier batch 12 evidence are unchanged.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
