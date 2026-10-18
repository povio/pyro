# TOS representative coverage W3-01 and W3-02, 2026-10-18

W3-01 and W3-02 begin worker 3's batch namespace. They are separate from W2's
numbered batches and do not overlap them. Each batch is the whole set of
manifest-routed cases in two files taken from `uncovered_files` in W2's ledger.
The ledger was read from branch `evidence/tos-coverage-ledger` @ `e3abe510`, and
its JSON has SHA-256
`3d1de5a08cf4add614432f30ea316d97472ffe42ea73d00a7e775655aef09016`. Each selection
was recorded and sent, with its hash, to W2 and the parent before any run of its
files. The selections are in
[`evidence/tos-coverage-w3/selections`](../../evidence/tos-coverage-w3/selections).

| Batch | Files | Cases | main `6e44bcd` | upstream `2ad38773` |
| --- | --- | ---: | --- | --- |
| W3-01 | `control-tower/navigation`, `finances/navigation` | 11 | `w3b01-1018-053947`: 11 / 0 | `w3b01u-1018-054051`: 11 / 0 |
| W3-02 | `master-data/uncovered-navigation`, `email-logs` | 15 | `w3b02-1018-054229`: 14 / 1 | `w3b02u-1018-054321`: 15 / 0 |

Results are shown as passed / failed.

**Regression in W3-02: `email-logs` "restores text drafts on Back and Forward
after duplicate blur commits".** The rewrite fails at the test's first
`page.goBack()`:

```
page.goBack: Protocol error (Page.getNavigationHistory): Page.getNavigationHistory is not supported by lightningpanda
```

- Upstream passes the case on the same fixture.
- Playwright implements `goBack` and `goForward` with `Page.getNavigationHistory`
  and `Page.navigateToHistoryEntry`. On `6e44bcd`, lp-cdp rejected both as
  unsupported. lp-browser also fired no `popstate` on traversal.
- This failure is caused by a missing protocol method; it is not a timing or
  test problem.
- The fix is in draft PR #82 (head `3f9b73d`). That PR is not in `6e44bcd`, so this
  table is unchanged by it. The TOS re-run of W3-02 on the PR head will be
  recorded separately.

All other 25 selected cases pass on both binaries. They cover:
- direct-route rendering of Control Tower, Finances and office-settings pages;
- unauthorized-route redirects that make no forbidden API request;
- email log listing, filtering, date ranges, bookmarks and attribution.

## Selection coverage

The routing manifest routes 742 `common` cases in 141 files. W2's ledger records
241 cases in 70 files as selected by batches 1–19. W3-01 and W3-02 add 26 cases
in 4 more files.

These are selected cases, not passing cases. Each binary's results stand on
their own and must not be added to another binary's results.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
  - Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
    Before every run, the manifest was rehashed and `git diff` showed the
    fixture's `apps/fe/e2e` tree unchanged.
- **Selections:**

  | Batch | `selection.json` SHA-256 | Recorded (UTC) | Playwright |
  | --- | --- | --- | --- |
  | W3-01 | `881ce9f0dc11236c846c8bd194689aafb8eeddde0c0ab72cb3700c5157dec58f` | 05:35:29 | "Running 11 tests" |
  | W3-02 | `d5ee93174663075a5498229643361bd6074fc9f4de5307e776ba7cd1b95e04e1` | 05:42:11 | "Running 15 tests" |

- **Binaries:**

  | Binary | SHA-256 | Build |
  | --- | --- | --- |
  | main `6e44bcd` | `0ce9777440471710fd2ce6adf3320d00353871430a903d68f90f9d29377104dc` | see below |
  | upstream `2ad38773` | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` | W2's existing build, reused read-only |

  - The `6e44bcd` binary came from a fresh `git clone --no-hardlinks` at the
    exact SHA, built with a fresh `CARGO_TARGET_DIR`, `CARGO_INCREMENTAL=0`,
    `cargo build --locked --release` and SDK 26.5. The build compiled 140
    crates.
  - `6e44bcd` does not contain PR75 or PR76.
  - Signing: ad hoc, linker-signed, with no entitlements (no `allow-jit`).
    JIT behaviour was not measured separately.
- **Fixture:** one fresh `git worktree add --detach` at the TOS commit
  (`/private/tmp/lp-w3-fixture-1`), then `bun install --frozen-lockfile`.
  - W3-01 ran on it first.
  - W3-02 and both upstream runs reused it. This reuse is disclosed; the e2e
    tree and manifest were checked as above.
- **Setup:** the same harness and conventions as W2's batches (`lp-tos-mkrun.sh`).
  - `bun --no-orphans scripts/run-fe-e2e.mjs run --config playwright.routed.config.ts <files>`,
    with the documented direct-lane version-gate bypass.
  - One Playwright worker, `E2E_ROUTING_ENGINE=lightpanda`.
  - Native PostgreSQL 17 with the `lp_bench` role and a per-run slug and
    template database.
  - Backend test token into localStorage, not UI login. Authentication and
    assertions are unchanged.
- **Database cleanup:**
  - Before each run, the role owned no databases.
  - Each run created exactly one database, `fe_e2e_<slug>_template_bun_e2e_8f4d6e232f35`.
  - Cleanup dropped that database by exact name only. Afterwards the role owned
    zero databases again.
  - The shared service, other roles and other workers' databases were not
    touched.
- **Host:** W2's TOS host (MacBookAir.localdomain, Apple M1, 8 GiB, macOS 26.6),
  reached through BB remote terminals. Each run held the host's heavy-step
  lock.
- **Time:** whole harness, not a performance comparison.

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `w3b01` | 52.7 s | 15.7 s |
  | `w3b01u` | 41.8 s | 9.4 s |
  | `w3b02` | 49.6 s | 17.8 s |
  | `w3b02u` | 37.6 s | 12.6 s |

- **Artifacts:** on the TOS host in `/private/tmp/lp-w3-b01`, `-b01u`, `-b02`
  and `-b02u`; copies are with worker 3. Each holds `coverage.log`, the
  Playwright JSON, metadata and the database lists.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
