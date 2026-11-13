# TOS representative coverage batch 2, 2026-10-17

Twelve unchanged manifest-routed scenarios ran on the pinned rewrite binary:
twelve passed, zero failed, zero skipped, retried or flaky. The selection was
recorded before any run of these files. It covers configuration and master-data
CRUD, form autosave, search, sort, filter and routing history, so it does not
repeat the [batch 1](TOS_COVERAGE_BATCH_1.md) files.

| File | Cases | Scenarios |
| --- | ---: | --- |
| configuration/roles.e2e.ts | 4 | create global role and verify listing; filter by context and owner; list seeded roles with columns; search by name |
| configuration/offices.e2e.ts | 4 | create office and verify list HTML; list rows; search from visible input; sort by name header |
| master-data/cargo/hs-codes.e2e.ts | 3 | add and archive; sort by identity and audit columns; autosave update |
| master-data/navigation-history.e2e.ts | 1 | routing history tracks affected master-data details |

Selection: all manifest `common` entries for these four files, written to
`selection.json` before execution.
- `selection.json` SHA-256 `c612f67bc36b05a49ab4ebd90009a45c56ff0e350351ddea36d60821f752ae62`.
- The runner reported "Running 12 tests using 1 worker". The manifest excludes
  any other tests in these files.

## Provenance

- Run: `lpb2-1017-141444`.
- Browser: `916267f391ca7b8f2fa85528e58aa03d1e32994a`, release.
  - It has no `crates/`, `Cargo.toml` or `Cargo.lock` difference from main at
    `8e53da5`.
  - Binary `/private/tmp/pyro-tos-browser-fd-916267f/target/release/pyro`,
    SHA-256 `7bb33527f4a8fe9598c4b91aa0eac52f6ba80f477af01e791e55a81ba8f566a5`,
    verified before execution. This is the same binary as
    [the 9-case comparison](TOS_CANVAS_CASE_COMPARISON.md).
- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Same documented direct-lane version bypass, backend-token localStorage auth, one
  Playwright worker and native PostgreSQL 17 with the `pyro_bench` test role. The
  run used its own slug and template database.
- After cleanup the test role owned zero databases. The command is
  `bun --no-orphans scripts/run-fe-e2e.mjs run --config playwright.routed.config.ts`
  with the four files.
- Time: 22.5 s Playwright, 59.5 s whole harness.
- Peak RSS of 1.86 GB from `time -l` covers the whole harness: Nest backend,
  bundler and Playwright as well as the browser. It is not browser-only CPU or
  memory evidence, and it is not an upstream comparison.

The fixture was a fresh `git worktree add --detach`, clean before execution
(`tos-pre-status.txt` is empty). After the run only the harness-generated
`apps/pdfgen/src/data/common/common.models.ts` changed, as in earlier runs. It
was left in place.

Sanitized artifacts (selection, metadata, command, scrubbed log and JSON report,
DB pre/post/cleanup) are in `/private/tmp/pyro-tos-batch2`. The fixture is
`/private/tmp/pyro-tos-f9f-batch2`.

Limits:
- The batch 1 caveats apply: the router bootstrap via pushState/PopStateEvent is
  not cold deep-link coverage, and activation dispatches synthetic DOM events,
  not native pointer input.
- This is 12 of 742 routed scenarios, not TOS compatibility. A single passing run
  does not establish flake rates.
