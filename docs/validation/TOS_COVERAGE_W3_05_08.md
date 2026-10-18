# TOS representative coverage W3-05 to W3-08, 2026-10-18

These are worker 3's batches W3-05 to W3-08. Each covers whole manifest-routed
groups of files from W2's ledger `uncovered_files` (JSON SHA-256 `3d1de5a0…`),
excluding W3-01 to W3-04. Before any run of its files, each selection was sent
with its hash to W2 and the parent, and W2 confirmed there was no overlap. The
selections are in
[`evidence/tos-coverage-w3/selections`](../../evidence/tos-coverage-w3/selections).

These batches ran on a newer main than W3-01 to W3-04, which ran on `6e44bcd`.
Keep the two groups' results separate.

All four batches ran on the W3 host, using Docker PostgreSQL 17. The setup is
described in [W3-01/W3-02](TOS_COVERAGE_W3_01_02.md#pr-82-verification-on-the-w3-host-docker-postgresql-17).
Autovacuum was off on that test-only server; see
[W3-03/W3-04](TOS_COVERAGE_W3_03_04.md#excluded-infrastructure-failure-and-the-autovacuum-setting).

| Batch | Files | Selected cases | main `e56e787` | upstream `2ad38773` |
| --- | --- | ---: | --- | --- |
| W3-05 | `logistics/quotes-overview`, `logistics/quotes-route` | 12 | `w3l05-1018-071339`: 12 passed, 0 failed | `w3l05u-1018-071432`: 12 passed, 0 failed |
| W3-06 | `configuration/employees-overview`, `logistics/quotes-document` | 12 | `w3l06-1018-071521`: 12 passed, 0 failed | `w3l06u-1018-071619`: 12 passed, 0 failed |
| W3-07 | `master-data/business-entities/business-partner`, `logistics/quotes-commercial-parties` | 11 | `w3l07-1018-071739`: 10 passed, 1 failed | `w3l07u-1018-071832`: 10 passed, 1 failed |
| W3-08 | `logistics/quotes-finances`, `logistics/positions-documents` | 13 | `w3l08-1018-071934`: 12 passed, 1 failed | `w3l08u-1018-072040`: 13 passed, 0 failed |

## W3-07: one unresolved failure shared by both binaries

Both binaries fail "Master Data Business Partners FE E2E › lists seeded
business partners as rendered table rows" at the same step:

```
expect(getLastCanvasTable(page)).toBeVisible() failed
Locator: locator('[role="grid"][aria-label="Data table"], .x-canvas-table:not(…)').filter({ visible: true }).last()
Error: element(s) not found
```

The preceding assertion on the row text passes. No Chromium comparison was run
for this batch, and no cause is claimed. The business-partner table is not one
of the components that set `renderer="dom"` at this TOS commit, so the W3-03
uninvoiced-charges explanation is not assumed to apply.

## W3-08: one lightningpanda regression, partly fixed in PR #86

Main fails "Quote Finances FE E2E › updates currencies and exchange rates from
rendered Finances controls"; upstream passes it.

```
press: Protocol error (Input.dispatchKeyEvent): Input editing commands, timestamps, system keys or alternate key metadata is not supported by lightningpanda
  at e2e/fixtures/form.ts:660  await page.keyboard.press("Escape")
```

- **Cause.** On macOS, Playwright 1.59.1 attaches the editing command
  `["cancelOperation"]` to Escape. lp-cdp accepts editing commands only where
  the engine's key default action performs them, and Escape was not listed.
- **Partial fix: draft PR #86** (head `036476888739d35306a5398dac440d9ee9cf9935`)
  accepts Escape's `cancelOperation`. Its build on the W3 host (SHA-256
  `3d2bd5fdc3e8d7c81562800727df6abbbe3d621a22fd080f7e613e24342de8be`) ran W3-08
  twice:

  | Run | Result |
  | --- | --- |
  | `w3l08e-1018-072922` | 12 passed, 1 failed |
  | `w3l08er-1018-073027` | 12 passed, 1 failed |

  - The Escape refusal is gone.
  - The same case now stops earlier, at
    `form.ts:616 page.keyboard.press("ArrowDown")` (`["moveDown"]`).
- **Remaining work.** ArrowUp/ArrowDown first need an engine caret default
  action in lp-webapi. W2 owns that crate and is implementing it; lp-cdp
  allowlist entries follow after that.
- **Not yet verified:** the TOS case passing with both parts. These PR runs are
  candidate evidence, separate from the main results.

## Selection coverage

W2's ledger records 241 selected cases in 70 files, of 742 cases in 141 files.

| Batches | Added cases | In files | Binary |
| --- | ---: | ---: | --- |
| W3-01 to W3-04 | 54 | 9 | main `6e44bcd` |
| W3-05 to W3-08 | 48 | 8 | main `e56e787` |

These are selected cases, not passing cases. Each binary's results stand on
their own and must not be added to another binary's results.

W2 and worker 3 have agreed to split the remaining 53 uncovered files:
- W2 takes 31 files, as W2-20 and later.
- Worker 3 takes 22 `logistics` position, quote-cargo and analytics files.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
  - Manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
  - Before every run, the manifest was rehashed and the fixture's `apps/fe/e2e`
    tree was checked unchanged.
- **Selections:**

  | Batch | SHA-256 | Playwright |
  | --- | --- | --- |
  | W3-05 | `c164ddca002d7f8c81c4eb099aad3e4e07e21090f6761a00123cdc73c994be80` | "Running 12 tests" |
  | W3-06 | `11519ab052d34b1c13abe6ae0f63afcc61bb12402e00a7f5ba535ec7f83089da` | "Running 12 tests" |
  | W3-07 | `cc141d130a453060e23f0084697961d11a989dea92bcb19a9c0393f8bdd8c40a` | "Running 11 tests" |
  | W3-08 | `73e585689f13cb1166f00afb18e87fb742eeda13de222ef64af12aac5a214aa9` | "Running 13 tests" |

- **Binaries:**

  | Binary | SHA-256 | Origin |
  | --- | --- | --- |
  | main `e56e787` | `2507ca09e6c698ebe0bff843c2248d363508fe25abb457f6fd64d9a8cfdd701f` | fresh build on the W3 host |
  | upstream `2ad38773` | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | W2's build, copied read-only |

  - Main `e56e787` includes PR75, PR76, PR80 and PR82. Its tree is `551b01bf`,
    the same tree as W2's tested integration candidate. PR85, merged later, is
    docs-only and leaves the tree unchanged.
  - The main binary came from a fresh `git clone --no-hardlinks` at the exact
    SHA, built in the clone's root so the repository's
    `scripts/macos/link-and-sign.sh` linker applied, with a fresh target,
    `CARGO_INCREMENTAL=0`, `--locked --release` and SDK 26.5. The build compiled
    140 crates.
  - Signing: ad hoc, with the entitlement `com.apple.security.cs.allow-jit`. JIT
    use at runtime was not measured separately.
- **Fixture:** `/private/tmp/lp-w3-lfix-1`, reused from the earlier W3 batches.
  Its e2e tree is unchanged.
- **Database cleanup:** each run created one database and dropped it by exact
  name. The test role then owned 0 databases.
- **Time:** whole harness, not a performance comparison.

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `w3l05` | 51.7 s | 25.6 s |
  | `w3l05u` | 48.6 s | 23.0 s |
  | `w3l06` | 56.7 s | 30.8 s |
  | `w3l06u` | 53.3 s | 27.8 s |
  | `w3l07` | 52.9 s | 27.3 s |
  | `w3l07u` | 60.9 s | 35.4 s |
  | `w3l08` | 64.3 s | 38.6 s |
  | `w3l08u` | 46.7 s | 20.7 s |

- **Artifacts:** on the W3 host in `/private/tmp/lp-w3-l05` … `-l08u`, plus
  `-l08e` and `-l08er`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
