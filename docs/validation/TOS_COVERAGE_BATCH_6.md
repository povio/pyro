# TOS representative coverage batch 6, 2026-10-17

Batch 6 is 13 unchanged manifest-routed scenarios from four files not covered by
batches 1–5: carriers master data, remark templates, the direct-invoice finance
form and the office bank-accounts tab. The selection was recorded before any run
of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `b0f0319` (pinned) | `lpb6-1017-170135` | 12 / 1 |
| upstream `2ad38773` | `lpb6u-1017-170253` | 13 / 0 |
| `92a679b` (PR #54) | `lpb6b-1017-171156` | 13 / 0 |

The single main failure is rewrite-specific: direct-invoice "sends the actual
direct invoice".
- The browser-error fixture recorded `NotSupportedError: Object URLs are not
  implemented`, thrown by `URL.createObjectURL` in TOS
  `file.utils-CIfdemwZ.js:1:657`.
- That helper registers a Blob URL, clicks an `<a download>` with a synthetic
  `MouseEvent('click')`, and revokes the URL later.
- PR #54 implements `createObjectURL`/`revokeObjectURL` registration. It also
  stops `download` links to `blob:`, `data:` or same-origin URLs from navigating
  the page.

**What this evidence does not validate.** With PR #54 the direct-invoice
assertions pass without any actual download, and without `blob:` fetch, XHR or
navigation support. None of those exist in the rewrite, and no CDP download
events are emitted. The test only requires that no browser error is logged and
that the page stays put. This run does not validate any download functionality.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `317396d0748054d4d2bb71fe20eb21464982946e92705a53f17cf818917c180e`.
  - It covers all manifest `common` entries for
    `master-data/business-entities/carriers`,
    `master-data/logistics/remark-templates`, `finances/direct-invoice` and
    `configuration/offices-bank-accounts`.
  - Every run reported "Running 13 tests".
- Binaries. Every hash was verified before execution.

  | Binary | SHA-256 | Notes |
  | --- | --- | --- |
  | main `b0f0319` | `6210596644b56bf0e5865ab5c66eef7374d114215908b7380dba218635b3d8a2` | the immutable binary built for the [batch 5](TOS_COVERAGE_BATCH_5.md) integrated rerun, reused without rebuilding |
  | `92a679b` | `f851f7c85b90aa1299d4d762808e282e84f35e74b1e1a9e2e6c80ca4dc55e221` | fresh `--no-hardlinks` clone release build |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | |

- Fixtures:
  - The main run used a fresh, clean `git worktree add --detach`.
  - The upstream run reused that worktree to save disk space. Its only change was
    the harness-generated `apps/pdfgen/src/data/common/common.models.ts`, left in
    place.
  - The `92a679b` run used its own fresh, clean fixture.
- Setup: one Playwright worker, native PostgreSQL 17 with the `lp_bench` role, and
  a per-run slug and template database. After each cleanup the test role owned
  zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb6` | 67.0 s | 30.5 s |
  | `lpb6u` | 54.0 s | 21.6 s |
  | `lpb6b` | 68.0 s | 27.4 s |

- Disk cleanup: after these runs, the parent authorized removing only `deps`,
  `build`, `incremental` and `.fingerprint` under eight older completed browser
  clones' `target/release`. Retained binary SHA-256s were unchanged; the audit is
  in `/private/tmp/lp-tos-cleanup-1017b`.

Artifacts:
- `/private/tmp/lp-tos-batch6`
- `/private/tmp/lp-tos-batch6-up`
- `/private/tmp/lp-tos-batch6-blob`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
