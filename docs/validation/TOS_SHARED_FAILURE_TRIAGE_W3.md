# Triage of shared TOS failures in worker 3 batches, 2026-10-18

This note follows up the failures that both pyro and upstream had in
batches W3-07, W3-10, W3-12, W3-17, W3-18 and W3-19. W3-03 is covered in
[W3-03/W3-04](TOS_COVERAGE_W3_03_04.md#follow-up-triage-added-after-merge).

The triage was read-only. Nothing in TOS changed: source, tests, authentication,
assertions and the manifest are as before. A failure shared by pyro and
upstream does not prove the browser had no part in it. Each case below is
classified only as far as the evidence goes, and unknowns are left explicit.

## Results

Three categories of confirmed TOS drift account for 16 failing cases: W3-17 (13
cases), W3-19 (2) and W3-07 (1). The W3-07 finding is from source only. The
remaining 5 cases (W3-18: 1, W3-10: 2, W3-12: 2) are not shown to be TOS drift:
Their broader causes remain unresolved. W3-12 has a reduced synthetic-geometry
mismatch consistent with the rewrite-only page observations (see below); it has
not been shown to explain every failure in either Lightpanda lane. Unresolved
does not mean the cause is outside TOS.

| Batch | Case(s) | Classification | Evidence |
| --- | --- | --- | --- |
| W3-17 | `positions-analytics`, 13 cases | **TOS data drift, confirmed** | Template database query, source, and a Chromium diagnostic run |
| W3-19 | `positions-overview`, both "INTTRA service type" cases | **TOS UI-text drift, confirmed** from source | Source |
| W3-07 | `business-partner` "lists seeded business partners as rendered table rows" | **TOS renderer drift, confirmed** from source; no run | Source |
| W3-18 | `positions-finances` "lists finance rows inside the Position details Finances tab" | **Backend deadlock during API fixture setup** | Server log |
| W3-10 | master position "marks a Position as Master Position", and quotes-cargo "adds and deletes a Package" | **Fails on all three engines; cause unresolved** | Chromium diagnostic run |
| W3-12 | route "updates the Route tab incoterm" and "…transport mode" | **Fails in both Lightpanda lanes; Chromium passes. Rewrite-only page observations are consistent with a reduced synthetic-geometry mismatch; full failure causes remain unresolved** | Chromium diagnostic run, rewrite-only probe runs, static repro |

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
pyro passes.

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
and the fixture got `500 uncaught-exception`. This happened on both pyro
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
  as do 12 extras. Both pyro and upstream fail the same two cases.
- **Failure modes.**
  - Incoterm: neither a visible "CPT" value nor a select trigger with "CPT" is
    found in the transshipment row (`prepareRouteInlineOption`).
  - Transport mode: `scrollIntoViewIfNeeded` on the visible "Transshipment Port"
    row waits until teardown.

#### Rewrite-only narrowing (added after merge)

The probes below ran only on the rewrite, main `2641569`. Upstream fails the same
two cases in the batch runs, but it was not probed on the page, so nothing here
is claimed about why upstream fails. The static repro at the end did run on both.

- **Probe runs.** Both used the fresh signed build of main `2641569` (SHA-256
  `b7937265…`), unchanged TOS, and a diagnostic CDP recording proxy. Before each
  `mousePressed`, the proxy injects one `Runtime.evaluate`; its reply is recorded
  and not forwarded. They are not coverage evidence.
  - `w3l12h-1018-112330`: `positions-route.e2e.ts` alone (10 cases), 8 passed and
    the same 2 failed. The probe recorded `document.elementFromPoint` at the click
    point and the select-trigger rectangles.
  - `w3l12s-1018-112717`: the "Route tab inline field updates" subset (7 cases),
    5 passed and the same 2 failed. The probe recorded a bounded, scrubbed
    snapshot: the hit element's chain, the trigger's ancestor chain with rects
    and selected computed styles, and the row markup.
  - A third attempt with Playwright tracing on (`w3l12d`) is excluded: tracing
    in the Lightpanda lane adds a `CSSGroupingRule` page error from Playwright's
    snapshotter, which fails cases on its own.
- **How the test clicks.** Its helper `clickVisibleCenter` calls
  `page.mouse.click` at the centre of the trigger's own bounding box.
- **What the page has at the click.** For incoterm, the "CPT" trigger
  (`BUTTON[data-type=select-trigger]`) reports (207, 0, 5, 5).
  - Its parent is a `DIV` with the class `pointer-events-none`, so the button is
    not hit-testable in the reduced case on all three engines. The full TOS
    page's computed pointer-events value was not retained by this snapshot.
  - The focusable select `DIV[tabindex=0]` that should receive the click reports
    (207, −15, 5, 15). That covers y −15…0, not the click point (209.5, 2.5).
  - `elementFromPoint` there returns the enclosing `TR` (56, −115, 1480, 286). So
    the click never reaches the select, and the option list never appears.
  - For transport mode, the same structure gives `BODY`.
- **Static repro** ([`evidence/tos-coverage-w3/w3-12-hit-test`](../../evidence/tos-coverage-w3/w3-12-hit-test),
  script and the three engines' outputs). It uses the
  same shape: a 26 px table cell, a focusable wrapper with a click listener, a
  `pointer-events: none` element, and the button. It clicks the button's centre.

  | Variant | Chrome 147 | rewrite `2641569` | upstream `2ad38773` |
  | --- | --- | --- | --- |
  | `pointer-events: none`, inline style | hit inside the wrapper; click reaches it | hit `BODY`; click misses | hit `BODY`; click misses |
  | `pointer-events: none`, stylesheet class | hit inside the wrapper; click reaches it | hit `BODY`; click misses | hit `BODY`; click misses |
  | Control: cell `overflow: hidden`, no `pointer-events: none` | hit the button; click reaches it | hit the button; click reaches it | hit the button; click reaches it |

  - In Chrome the wrapper and the button share one box (11, 13, 43, 21).
  - In both Lightpanda engines, the reduced example's nested divs and button
    get 5×5 boxes stacked in document order. The stylesheet-class variant's
    wrapper is (0, 60, 5, 5) and its button (0, 75, 5, 5), so the wrapper does
    not enclose the button. The cell itself retains its explicit 26 px height.
- **Classification.**
  - The reduced target is reachable under Chrome geometry. Separately, the
    Chromium diagnostic run passes the selected TOS cases.
  - In the reduced case, both Lightpanda engines honour inherited
    `pointer-events: none`, but the wrapper geometry does not cover the button
    centre. This reproduces a synthetic-layout difference independently of TOS;
    it does not establish that hit testing should ignore geometry or clipping.
  - The failing static variants do not set overflow clipping. The separate
    overflow control does not exclude clipping, stacking or scroll effects in
    the full TOS page; those are not established by the truncated snapshots.
  - The rewrite-only TOS observations support this as a candidate explanation
    for the missed clicks. A native repair and unchanged-case rerun are still
    needed to establish their contribution to each failure. Upstream's full-page
    failure mechanism was not probed and remains unresolved.
  - The repro has been sent to W2, who owns layout and hit-testing (pyro-dom and
    pyro-webapi). There is no production change here.
- **Limits.**
  - Two simpler repros did not reproduce the miss on any engine. They had no
    `pointer-events: none`, and they stay on the W3 host in
    `/private/tmp/pyro-w3-probes`:
    - `hit-test-repro.mjs`: overflow-hidden cells and nested flex;
    - `hit-test-min.mjs`: six nested divs in a 26 px cell.
  - The probe adds one evaluation and one local WebSocket hop per press.
  - The snapshot's style filter drops `none` values, so `pointer-events: none`
    was read from the class list and confirmed by the static repro. It does not
    appear in the recorded styles.
  - The snapshots stay on the W3 host, and only their hashes are given below.
    Before saving, UUIDs, `value` attributes, ids and ARIA references were
    removed, and text was replaced with `x` except the option labels.

## Provenance

- **TOS:** unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`, manifest
  `542d479b…`, fixture `/private/tmp/pyro-w3-lfix-1`.
- **Selections:** the same files as the original batches, so the same hashes
  (see `evidence/tos-coverage-w3/selections`).
- **Chromium diagnostic runs** (`w3l17c`, `w3l10c`, `w3l12c`): Chromium lane,
  `playwright.e2e.config.ts`, Playwright 1.59.1 headless shell 1217. They are
  not coverage evidence, and selected and extra cases are counted separately.
  This scope covers only these three runs.
- **Lightpanda probe runs** (`w3l12h`, `w3l12s`): routed config, rewrite main
  `2641569`, SHA-256
  `b79372659084464730fc1a64589d8a7c82c0137e0afe21f34d37548fcef3e4ff`. These are
  not coverage evidence.
  - Recording wrapper `pyro-record-wrapper.mjs`: `f0c15757…`.
  - Proxies: `cdp-hittest-proxy.mjs` `81f2b411…` and `cdp-snapshot-proxy.mjs`
    `e2b6cf7a…`.
  - Snapshot SHA-256 values:

    | Snapshot | SHA-256 |
    | --- | --- |
    | `incoterm-click.json` | `a5b39dce…` |
    | `incoterm-click.tr.html` | `b4557898…` |
    | `transport-mode-click.json` | `f730598f…` |
    | `transport-mode-click.tr.html` | `0739cef7…` |

- **Static repro:** run with Playwright 1.59.1 against Chrome 147.0.7727.15
  (headless shell), the rewrite binary above, and upstream `2ad38773`
  (`7f76ccfe…`).
- **TOS harness diagnostic runs:**
  - Each used the harness's retained template and
    `PYRO_SHARED_ASSET_CACHE_MB=64`, which the Chromium lane ignores.
  - Each created one database and dropped it by exact name, leaving 0 run
    databases. The static data-URL probes use no backend or database.
- **Retained template ledger:**
  - The template is `fe_e2e_w3_f9fde4aa_lfix1_template`, owned by the test role.
    It was created by run `w3l19h-1018-093618` and reused since; the stamp hash
    is `0ebac3a9acb4`.
  - It is the only database the test role currently owns.
  - It is retained until W3 TOS work ends, and then dropped by exact name.
  - The template database query was read-only
    (`default_transaction_read_only`).
- **Artifacts:** on the W3 host in `/private/tmp/pyro-w3-l17c`, `-l10c`, `-l12c`,
  `-l12h`, `-l12s` and `-l12d` (excluded), and `/private/tmp/pyro-w3-probes`, plus
  the original batch run directories.
