# TOS representative coverage batch 3 and main integration, 2026-10-17

Batch 3 is 14 unchanged manifest-routed scenarios. Its selection was recorded
before any run of these files. It covers quote and position forms (copy, convert,
close/reopen), finance listing/search/details/delete, column configuration, and
canvas-table search/sort.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| `916267f` | `lpb3-1017-141709` | 8 / 6 |
| main `e9123f7` | `lpb3m-1017-142312` | 8 / 6 |
| `da71ea7` (DOMParser) | `lpb3dp-1017-143750` | 10 / 4 |
| `ee9dbde` (PR #45 head: DOMParser + virtual `click()`) | `lpb3c-1017-144937` | 11 / 3 |
| `a2afd29` (PR #46 pre-review head, on main `c29d754`) | `lpb3x-1017-145758` | 12 / 2 |
| `361ee22` (PR #46 head, review fixes) | `lpb3y-1017-153940` | 12 / 2 |
| upstream `2ad38773` | `lpb3u-1017-142535` | 12 / 2 |

Rewrite-specific failures and the fixes for them:
- **DOMParser (fixed in PR #45).** invoices "opens searched invoice details" and
  "keeps invoice search after closing the details modal" failed.
  - tiptap's `TextEditor` chunk calls `new window.DOMParser().parseFromString(...)`
    (`TextEditor-BzPvjmkd.js:11:62394`), which logged a console.error and then
    timed out.
  - There was no DOMParser binding.
- **Virtual click (fixed in PR #45).** invoices "deletes a draft Direct Invoice"
  failed because the "Delete Draft" React Aria `onPress` ignored `element.click()`.
  - A separate diagnostic spec found the button enabled, but the click event had
    `detail` 1.
  - React Aria's `usePress` treats a click as virtual only if `detail` is 0 and
    `pointerType` is empty.
  - Browsers and upstream fire `click()` with `detail` 0 and `pointerId` -1.
- **XPath (PR #46).** quotes-copy failed because its Playwright `xpath=`
  locator (`ancestor::div[contains(@class, 'h-[2.625rem]')][1]`) threw
  "Can't find variable: XPathResult". There was no `document.evaluate`.

Shared failure: "positions-close" (both cases) fails on rewrite, upstream and
Chromium.
- The Chromium run used the direct config on the unchanged file: run
  `lpchrcl-1017-145233`, 0/2, with the same "count expected 1, received 0"
  predicate.
- No rewrite-specific cause is established, and the root cause is not proven.
- At TOS `f9fde4aa0d`, the "Close Position" action is rendered only inside the
  settings Popover. It moved there in TOS `c4a7167a0b` (2026-10-05, "Move position
  actions into settings menu and unify confirmation modals").
- The spec, last changed 2026-09-24, expects a directly visible button, so test
  drift is likely. The seed grants `office-position-closing-admin`.

## Integrated main `e9123f7`

These runs checked main after PR #37 (lazy interface objects). The release binary
is SHA-256 `e13ddf3fa792f50d13ab0f7bfea8ffb452f4908d09d04ffe322054f833ab1085`,
built from a fresh `--no-hardlinks` clone with its own target directory.

| Selection | Run | Passed / failed |
| --- | --- | --- |
| profile-settings smoke (2) | `lpsmm-1017-142806` | 2 / 0 |
| batch 1 (9) | `lpcovm-1017-142859` | 8 / 1 (shared canvas case, [comparison](TOS_CANVAS_CASE_COMPARISON.md)) |
| [batch 2](TOS_COVERAGE_BATCH_2.md) (12) | `lpb2m-1017-142137` | 12 / 0 |
| batch 3 (14) | `lpb3m-1017-142312` | 8 / 6 (as `916267f`) |

No failure referenced interface objects or prototypes.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- Batch 3 `selection.json` SHA-256
  `947a0fc423b3d58582c7b85ed48b8a296398d781a1b77f5c95f780013b8c684f`.
  - It covers all manifest `common` entries for seven files: `logistics/quotes`,
    `quotes-copy`, `quotes-convert-to-position`, `positions-close`,
    `finances/invoices`, `finances/outgoing-invoices` and
    `master-data/locations/countries`.
  - It was recorded before the first run. Every routed run reported "Running 14
    tests".
- Binary SHA-256 values, all verified before execution; every rewrite binary is a
  release build from a fresh clone:

  | Binary | SHA-256 |
  | --- | --- |
  | `916267f` | `7bb33527f4a8fe9598c4b91aa0eac52f6ba80f477af01e791e55a81ba8f566a5` |
  | `e9123f7` | `e13ddf3fa792f50d13ab0f7bfea8ffb452f4908d09d04ffe322054f833ab1085` |
  | `da71ea7` | `de5a1539df832247ce8f8c7703057c2651e1138f0cab49228481e64b85e317e8` |
  | `ee9dbde` | `2adc21918226420919bf069d4071c9bf8cb5d553ff93528f05641563045295c9` |
  | `a2afd29` | `f431aa5f53fbed8c8b4e1bd987c610b1fb111a51284afb3763ebb377ede8ab3c` |
  | `361ee22` | `d3afc0fb18c060de5b47d12d341f4924dd1c32ea8a48af57846a4c7492bf44ff` |
  | upstream `2ad38773` | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` |

- Chromium: Playwright 1.59.1 chromium-headless-shell 147.0.7727.15. It ran on the
  direct `playwright.e2e.config.ts` lane, with cold `page.goto` and native clicks.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Setup: one Playwright worker, native PostgreSQL 17 with the `lp_bench` role, and
  a per-run slug and template database. Every run used a fresh `git worktree add
  --detach` fixture, clean before execution. Afterwards only the harness-generated
  `apps/pdfgen/src/data/common/common.models.ts` had changed, and it was left in
  place.
- Database cleanup: after each run the test role owned zero databases.
  - In `lpb3x` the harness's own `DROP DATABASE … WITH (FORCE)` failed with
    "permission denied to terminate process", leaving the run database, its
    template and its snapshot template.
  - No sessions remained on them.
  - Exactly those three names were dropped after checking that each was owned by
    `lp_bench`, carried the run prefix and was absent before the run
    (`cleanup2.py`, `db-cleanup2.txt`).
- Whole-harness wall time is 101.9–120.8 s per batch 3 run, and Playwright time is
  64.8–78.1 s. These cover the whole harness, are not browser-only, and are not a
  performance comparison.

Artifacts:
- `/private/tmp/lp-tos-batch3{,-main,-dp,-click,-xpath,-xpath2,-up}`
- `/private/tmp/lp-tos-chrclose`
- `/private/tmp/lp-tos-smoke-main`
- `/private/tmp/lp-tos-coverage-main`
- `/private/tmp/lp-tos-batch2-main`

Diagnostic runs (`/private/tmp/lp-tos-diag12` to `diag14`) used an added diagnostic
spec in a separate worktree and are not part of these measurements.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. This is 14 of 742 routed scenarios, and single runs do not
establish flake rates.
