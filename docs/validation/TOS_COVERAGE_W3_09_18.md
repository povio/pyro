# TOS representative coverage W3-09 to W3-18, 2026-10-18

These are worker 3's batches W3-09 to W3-18, all drawn from worker 3's share of
the W2/W3 split of the ledger's remaining `uncovered_files` (ledger JSON
SHA-256 `3d1de5a0…`). Each batch is whole manifest-routed files. Before any
run of its files, each selection was sent with its hash to W2 and the parent.
The selections are in
[`evidence/tos-coverage-w3/selections`](../../evidence/tos-coverage-w3/selections).

All runs were on the W3 host, using Docker PostgreSQL 17 with autovacuum off
(see [W3-01/W3-02](TOS_COVERAGE_W3_01_02.md#pr-82-verification-on-the-w3-host-docker-postgresql-17)
and [W3-03/W3-04](TOS_COVERAGE_W3_03_04.md#excluded-infrastructure-failure-and-the-autovacuum-setting)).

Two main binaries were used:

- W3-09 and W3-10 ran on main `e56e787`, the same binary as
  [W3-05 to W3-08](TOS_COVERAGE_W3_05_08.md).
- W3-11 to W3-18 ran on main `e8cb7ef`, which adds PR86 (Escape), PR88 (caret)
  and PR91 (ArrowUp/ArrowDown).

Keep results from different binaries separate.

| Batch | Files (`e2e/logistics/…`) | Selected cases | main | upstream `2ad38773` |
| --- | --- | ---: | --- | --- |
| W3-09 | `position-export-templates`, `vat-presets-finances`, `analytics`, `positions/sea-courier-agent` | 13 | `e56e787` `w3l09-1018-073928`: 13 passed, 0 failed | `w3l09u-1018-074011`: 13 passed, 0 failed |
| W3-10 | `quotes-cargo`, `positions-master-sub-positions`, `positions-overview-autosave` | 13 | `e56e787` `w3l10-1018-074055`: 11 passed, 2 failed | `w3l10u-1018-074235`: 11 passed, 2 failed |
| W3-11 | `positions-air-transport-order`, `-incoming-invoice-credit-note`, `-outgoing-invoice-payments`, `-register-incoming-pro-forma`, `-route-autosave` | 13 | `e8cb7ef` `w3l11-1018-080626`: 13 passed, 0 failed | `w3l11u-1018-080738`: 13 passed, 0 failed |
| W3-12 | `positions-route`, `positions-route-tracking` | 15 | `e8cb7ef` `w3l12-1018-080842`: 13 passed, 2 failed | `w3l12u-1018-081025`: 13 passed, 2 failed |
| W3-13 | `positions-house-bl-document`, `positions-route-subpositions` | 16 | `e8cb7ef` `w3l13-1018-081241`: 16 passed, 0 failed | `w3l13u-1018-081357`: 16 passed, 0 failed |
| W3-14 | `positions-shipping-instructions-document` | 10 | `e8cb7ef` `w3l14-1018-081457`: 10 passed, 0 failed | `w3l14u-1018-081613`: 10 passed, 0 failed |
| W3-15 | `positions-todos` | 9 | `e8cb7ef` `w3l15-1018-081911`: 9 passed, 0 failed | `w3l15u-1018-081953`: 9 passed, 0 failed |
| W3-16 | `positions` | 13 | `e8cb7ef` `w3l16-1018-082032`: 13 passed, 0 failed | `w3l16u-1018-082111`: 13 passed, 0 failed |
| W3-17 | `positions-analytics` | 18 | `e8cb7ef` `w3l17-1018-082210`: 5 passed, 13 failed | `w3l17u-1018-082454`: 5 passed, 13 failed |
| W3-18 | `positions-finances` | 21 | `e8cb7ef` `w3l18-1018-082926`: 20 passed, 1 failed | `w3l18u-1018-083122`: 20 passed, 1 failed |

There is no rewrite-only failure in these batches. Every failure fails on both
binaries, with the same case titles; only durations differ. No cause is
claimed for any of them. They are unresolved shared failures.

- **W3-10**
  - `positions-master-sub-positions` › "marks a Position as Master Position
    from the FE": a `toBeVisible` locator is not found, and a pending
    `waitForResponse` ends at teardown.
  - `quotes-cargo` › "adds and deletes a Package from the FE": a
    `toBeVisible` locator is not found.
- **W3-12** (`positions-route`)
  - "updates the Route tab incoterm" fails `toBeGreaterThan`.
  - "updates the Route tab transport mode" waits in `scrollIntoViewIfNeeded`
    until teardown.
- **W3-17** (`positions-analytics`): all 13 failures are `toContain`
  assertions on rendered period labels.
  - The expected labels are around November 2026 (for example "01 Nov 2026",
    "Nov 2026", "W44 2026").
  - The rendered rows show other periods (for example "Jan 2026", "Oct 2026",
    "Apr 2031").
  - Three cases ran for about 0 ms. Their durations are recorded as reported.
  - The runs took place on 2026-10-18.
- **W3-18** (`positions-finances`) › "lists finance rows inside the Position
  details Finances tab": a fixture API call returned
  `500 {"code":"uncaught-exception"}` (`e2e/fixtures/api.ts:65`) on both
  binaries.

## W3-08 on the combined Escape and ArrowUp/ArrowDown candidate

This section adds the TOS verification reported in PR #91 (merged), which
[W3-05 to W3-08](TOS_COVERAGE_W3_05_08.md#w3-08-one-pyro-regression-partly-fixed-in-pr-86)
describes as not yet verified.

The candidate was `78d67405e9071c7a0eb8437d571e47a7ae1e40b6`, tree `38c8f94c`:
main `a398e2d`, which includes PR86, plus PR88 `98110d0` and the PR91 pyro-cdp
entries. Its fresh in-clone signed build has SHA-256
`d648896b381d85f25ae79cebae75689c2b10a15bb070b310c9f0ef916b85288f`, ad hoc
signed with `allow-jit`.

W3-08 was run unchanged:

| Run | Result |
| --- | --- |
| `w3l08a-1018-075638` | 13 passed, 0 failed |
| `w3l08ar-1018-075745` | 13 passed, 0 failed |

Quote Finances "updates currencies and exchange rates from rendered Finances
controls" passed in both runs. That is the W3-08 regression on `e56e787`.

## Selection coverage

W2's ledger records 241 selected cases in 70 files, of 742 cases in 141 files.

| Batches | Added cases | In files |
| --- | ---: | ---: |
| W3-01 to W3-08 | 102 | 17 |
| W3-09 to W3-18 | 141 | 21 |

At the time of these runs, worker 3's share had one file left,
`positions-overview`, with 32 cases. That file was subsequently completed; see
the [W3-19 recovery and failure reconciliation](TOS_W3_FAILURE_LEDGER.md).
These are selected cases, not passing cases. Each binary's results stand on its
own and must not be added to another binary's results.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
  - Manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
  - Before every run, the manifest was rehashed and the e2e tree of fixture
    `/private/tmp/pyro-w3-lfix-1` (reused) was checked unchanged.
- **Selections:**

  | Batch | SHA-256 |
  | --- | --- |
  | W3-09 | `395277a79b07c5d96afddefc81e4f3a1963ed52424f209b550e837e77cb393d1` |
  | W3-10 | `d51bc7e6b04ff82a2bbacad4a5de478197764f02d07f76e837c26dd8430a1301` |
  | W3-11 | `6c59e93854e3fc91f450ec531597db24cccd3a960fd3e4580bc39f38a982839d` |
  | W3-12 | `66d3e9ceb7be942b61fd787fc5a91efd059ca9a986cc61f060576817c256ec08` |
  | W3-13 | `3ebc41f89254a62c898bc02f2c9c1c68a1495ea8de349101df50189bca592df2` |
  | W3-14 | `b1b10cb5d7d2c5db89d4191d9c04d451496ee1413b6bbeb6aee6d75d2df2e13c` |
  | W3-15 | `554a7b1eb81e1062034cbf752b59e682ec6db1ebcde0fb715e0bf3798210d09e` |
  | W3-16 | `a5081d8da3b07f5e9de53f45b18effa1fb4cd4b75018b6f75c04c629dfca23e1` |
  | W3-17 | `b373df9b94fa363230593228cc7b28f43c3e41b3f5f7012b7ae76619bff2ec11` |
  | W3-18 | `50605e1da2beba213527573c171fae3e8ecdcb05e0184a819d9cb992301dcac5` |

  Every run reported "Running N tests", where N is the batch's case count.
- **Binaries:**

  | Binary | SHA-256 |
  | --- | --- |
  | main `e56e787` | `2507ca09e6c698ebe0bff843c2248d363508fe25abb457f6fd64d9a8cfdd701f` |
  | main `e8cb7ef` (tree `1828893a`) | `93d3669c69aff4550361baf7da071ca5b92dbfa5d6a303abfa2aa91962602c83` |
  | upstream `2ad38773` | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` |

  - The two main binaries are fresh in-clone builds with `CARGO_INCREMENTAL=0`,
    `--locked --release` and SDK 26.5. They are ad hoc signed with
    `allow-jit`; JIT use at runtime was not measured separately.
  - The upstream binary is W2's build, copied read-only.
- **Database cleanup:** each run created one database and dropped it by exact
  name. The test role then owned 0 databases.
- **Recovery note:** the W3-17 and W3-18 runs finished while worker 3 was
  paused for quota. A temporary W3 takeover recovered their results from the
  run artifacts, which include metadata and database lists. The runs were not
  repeated.
- **Time:** whole harness, not a performance comparison.

  | Run | Wall time |
  | --- | ---: |
  | `w3l09` | 42.5 s |
  | `w3l09u` | 42.9 s |
  | `w3l10` | 98.7 s |
  | `w3l10u` | 114.8 s |
  | `w3l11` | 70.4 s |
  | `w3l11u` | 63.5 s |
  | `w3l12` | 101.8 s |
  | `w3l12u` | 102.6 s |
  | `w3l13` | 75.7 s |
  | `w3l13u` | 59.0 s |
  | `w3l14` | 74.4 s |
  | `w3l14u` | 58.4 s |
  | `w3l15` | 40.8 s |
  | `w3l15u` | 37.9 s |
  | `w3l16` | 37.6 s |
  | `w3l16u` | 36.3 s |
  | `w3l17` | 162.9 s |
  | `w3l17u` | 270.6 s |
  | `w3l18` | 114.8 s |
  | `w3l18u` | 88.8 s |

- **Artifacts:** on the W3 host in `/private/tmp/pyro-w3-l09` … `-l18u`, plus
  `-l08a` and `-l08ar`.

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
