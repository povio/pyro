# TOS representative coverage batch 1, 2026-10-17

Nine unchanged manifest-routed scenarios ran: zero passed, nine failed, zero
skipped/retried. Eight failures expose missing IntersectionObserver; one exposes
the unavailable public Response constructor. No CDP/root network defect is
established by these failures. The earlier [profile/settings 2/2 success](TOS_CLONE_SMOKE.md)
remains valid for its narrower scope.
Later reruns of this selection, with an upstream and Chromium comparison, are in
[TOS_CANVAS_CASE_COMPARISON.md](TOS_CANVAS_CASE_COMPARISON.md).

| File | Selected cases | First observed failure |
| --- | ---: | --- |
| configuration/employees.e2e.ts | 4 | Missing IntersectionObserver in useIntersectionObserver-DosExQ3V.js:1:466; React effect fails before response/header assertions complete. Later waiter page-closed errors occur during teardown. |
| configuration/offices-overview.e2e.ts | 2 | Same missing observer; expected form text is absent. Autosave assertions are not reached. |
| platform/navigation.e2e.ts | 3 | Calendar and module-switch paths fail at missing observer. Platform-root redirect reaches redirect-BWHIdfef.js:1:272, which constructs new Response(null, {status, headers}); the binding throws Illegal constructor. |

The calendar stack points to staticSelect.utils-BfTCi26B-Bp87gcyc.js:1:62913,
also new IntersectionObserver. Built chunks remain in the fixture's
`.tmp/fe-e2e-portless/fe-dist/assets`. These are owned WebAPI implementation gaps;
fixing them does not guarantee later assertions will pass.

- Browser commit: `9963335e5bd8ae1984c9cbbce9aa217f27536cbd`, release; same tree as merged main
  `f537c86ee1c31b49e221cc13ee0d7a41ac96e713`.
- SHA-256: `04238836db5fb4600ba14457910ad9a86a60dcf35a6d0b59203f56fdca835931`.
- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256: `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- Host: MacBookAir.localdomain; run `lpcov1-1017-122150`.
- Time: 177.502 seconds Playwright, 214.139 seconds whole harness. This is not
  browser-only CPU/memory evidence or a same-workload upstream comparison.
- Same documented direct-lane version bypass, backend-token localStorage auth,
  native PostgreSQL service/test role and isolated run databases. No spec,
  helper, auth, manifest or timeout edits. The existing manifest excludes other
  tests in the selected files; the runner confirmed exactly nine selected tests.

Coverage limits matter: openRoute bootstraps an authenticated profile and uses
history.pushState/PopStateEvent for shell navigation, so this is not cold deep-link
coverage. DOM activation dispatches synthetic Pointer/Mouse events through
locator.evaluate, and blur uses DOM.blur; it does not prove native pointer/hit
behavior. Employee tests assert headers and route-backed responses, not exhaustive
row contents. The office tests would assert persisted rendered values after
navigation; the module switch would check the old link absent and table visible.
Those downstream checks did not pass in this run.

Sanitized artifacts: `/private/tmp/pyro-tos-coverage1` (selection.json, metadata,
command, toolchain, report/log, DB pre/post/cleanup). Fresh fixture:
`/private/tmp/pyro-tos-f9f-coverage1`. The prior readonly binary remains at
`/private/tmp/pyro-tos-browser-clone/target/release/pyro`; its hash was
verified before execution. Fixture was clean before execution; afterward only
the harness-generated PDF model changed. The sole leftover template was removed
after exact owner/prefix/preabsence checks; test role then owned zero databases.
All earlier failures, successful smoke artifacts and upstream baseline remain.

```sh
bun --no-orphans scripts/run-fe-e2e.mjs run --config playwright.routed.config.ts e2e/configuration/employees.e2e.ts e2e/configuration/offices-overview.e2e.ts e2e/platform/navigation.e2e.ts
```
