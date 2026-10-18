# Triage of shared TOS failures in worker 3 batches, 2026-10-18

This note follows up the failures that both lightningpanda and upstream had in
batches W3-07, W3-10, W3-12, W3-17, W3-18 and W3-19. W3-03 is covered in
[W3-03/W3-04](TOS_COVERAGE_W3_03_04.md#follow-up-triage-added-after-merge).

The triage was read-only. Nothing in TOS changed: source, tests, authentication,
assertions and the manifest are as before. A failure shared by lightningpanda and
upstream does not prove the browser had no part in it. Each case below is
classified only as far as the evidence goes, and unknowns are left explicit.

## Results

| Batch | Case(s) | Classification | Evidence |
| --- | --- | --- | --- |
| W3-17 | `positions-analytics`, 13 cases | **TOS data drift, confirmed** | Template database query, source, and a Chromium diagnostic run |
| W3-19 | `positions-overview`, both "INTTRA service type" cases | **TOS UI-text drift, confirmed** from source | Source |
| W3-07 | `business-partner` "lists seeded business partners as rendered table rows" | **TOS renderer drift, confirmed** from source; no run | Source |
| W3-18 | `positions-finances` "lists finance rows inside the Position details Finances tab" | **Backend deadlock during API fixture setup** | Server log |
| W3-10 | master position "marks a Position as Master Position", and quotes-cargo "adds and deletes a Package" | **Fails on all three engines; cause unresolved** | Chromium diagnostic run |
| W3-12 | route "updates the Route tab incoterm" and "…transport mode" | **Fails only in the Lightpanda lanes; Chromium passes; cause unresolved** | Chromium diagnostic run |

### W3-17: analytics month buckets (TOS data drift)

- **What the test expects.** It expects periods around November 2026. The seed
  (`apps/be/scripts/e2e/seed-fe-e2e.ts`, about lines 5904–6150) writes
  `serviceDate` 2026-11-01 … 11-07 for seven positions.
- **What the template database holds.** In the retained template
  `fe_e2e_w3_f9fde4aa_lfix1_template`, queried read-only, `Position.service_date`
  is NULL for those seeded positions (`E2E-POS-LIST-001`, `-SORT-A`, `-SORT-M`,
  `-SORT-Z`, `-MÜNCHEN-200`, `-ROUTE-300`, `-ROAD-400`). Their `created_at`
  values are 2026-11-01 … 11-07. The column is
  `timestamp without time zone`, and the server time zone is `Etc/UTC`.
- **Why it is NULL.** TOS migration `20260926090000_database_position_cache`
  (commit `7b53f0d893`, 2026-09-28, which is after the test's last edit on
  2026-09-24) maintains `service_date` through the position-cache flush:
  `service_date = COALESCE(default_route.service_date, estimated_service_date)`.
  The seeded positions have neither, so the seed's explicit `serviceDate` is
  replaced with NULL.
- **How the analytics query groups.** Analytics groups by
  `date_trunc(period, COALESCE(p.service_date, fallback))`
  (`position-core.repository.ts` around line 1493). The fallback is
  `getAnalyticsFallbackServiceDate()`, which is tomorrow's UTC date from the
  wall clock (around line 1821).
- **Result.** With runs on 2026-10-18, the seven positions fall into
  "Oct 2026". That is exactly what all three engines render: "Oct 2026" with 7
  positions, and no "Nov 2026".
- **Chromium diagnostic run** `w3l17c-1018-110449`: on the 18 selected cases,
  5 passed and 13 failed, the same 13 cases. Two extra cases, which the
  manifest routes to Chromium, also ran and are excluded.
- **Consequence.** The outcome depends on the wall clock; it is not
  browser-dependent.

### W3-19: INTTRA service type (TOS UI-text drift)

Both cases wait for the placeholder "Select INTTRA Service Type". TOS commit
`ccc8e360ce` (2026-10-12, "Add loading and discharge ICD/CY routing", after the
test's last edit) renamed it to "Select Carrier Service Type" in
`translation.json`; the old string no longer exists in `apps/fe/src`. Upstream
fails the same two cases, plus "updates air import customs status", which
lightningpanda passes.

### W3-07: business-partner canvas table (TOS renderer drift)

- **What the test expects.** It calls `getLastCanvasTable`. That locator matches
  only the canvas grid (`[role="grid"][aria-label="Data table"]` or
  `.x-canvas-table`).
- **What the app renders now.** `BusinessPartnersTable` uses `InfiniteTable`
  without a `renderer` prop. TOS commit `79035d3e45` (2026-10-02, after the
  test's last edit on 2026-09-24) changed `InfiniteTable`'s default renderer
  from `"canvas"` to `"dom"`. This is the same commit identified for the W3-03
  uninvoiced-charges sorting failures.
- **Status.** Confirmed from source. No diagnostic run was made.

### W3-18: backend deadlock during API fixture setup

The setup helper sends parallel `POST /offices/<id>/positions/<id>/account/items`
requests, one per item type, for the same position (`Promise.all`). These go
through Playwright's API request context, not a page.

The backend logged `error: deadlock detected` when committing the transaction,
and the fixture got `500 uncaught-exception`. This happened on both lightningpanda
and upstream, and no browser is involved in that request. The new position-cache
triggers (deferred constraint trigger at commit) are a plausible contributor, but
no cause is claimed. It was not run on Chromium.

### W3-10: two cases fail on every engine

- **Chromium diagnostic run** `w3l10c-1018-111115`: on the 13 selected cases,
  11 passed and the same 2 failed:
  - `getByText('Master Position')` is not found;
  - the Quote Cargo package card locator is not found.
- **Extras.** Nine cases that the manifest routes to Chromium also ran; 2 of
  them failed. They are not part of the selection.
- **Status.** The cause is not established.

### W3-12: two cases fail only in the Lightpanda lanes

- **Chromium diagnostic run** `w3l12c-1018-111322`: all 15 selected cases pass,
  as do 12 extras. Both lightningpanda and upstream fail the same two cases.
- **Failure modes.**
  - Incoterm: neither a visible "CPT" value nor a select trigger with "CPT" is
    found in the transshipment row (`prepareRouteInlineOption`).
  - Transport mode: `scrollIntoViewIfNeeded` on the visible "Transshipment Port"
    row waits until teardown.
- **Status.** These cases are specific to the Lightpanda lane: browser behaviour,
  layout visibility, or the lane's DOM-activation adaptations. They are not a
  rewrite-only regression. The cause is not established.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`, manifest
  `542d479b…`, fixture `/private/tmp/lp-w3-lfix-1`.
- **Selections:** the same files as the original batches, so the same hashes
  (see `evidence/tos-coverage-w3/selections`).
- **Diagnostic runs:** Chromium lane, `playwright.e2e.config.ts`, Playwright
  1.59.1 headless shell 1217. They are not coverage evidence, and selected and
  extra cases are counted separately.
  - Each run used the harness's retained template and
    `LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB=64`, which the Chromium lane ignores.
  - Each run created one database and dropped it by exact name, leaving 0 run
    databases.
- **Retained template ledger:**
  - The template is `fe_e2e_w3_f9fde4aa_lfix1_template`, owned by the test role.
    It was created by run `w3l19h-1018-093618` and reused since; the stamp hash
    is `0ebac3a9acb4`.
  - It is the only database the test role currently owns.
  - It is retained until W3 TOS work ends, and then dropped by exact name.
  - The template database query was read-only
    (`default_transaction_read_only`).
- **Artifacts:** on the W3 host in `/private/tmp/lp-w3-l17c`, `-l10c` and
  `-l12c`, plus the original batch run directories.
