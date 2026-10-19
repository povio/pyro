# Pointer-default focus preserves the press point

Frozen engine fb3b12dc failed two unchanged split-route cases: `8f696ff70238b8ee51c6bcd01748dd65e43070c831c6d50ce4854ff179321ccd` (Positions) and `daee6424923bdcfea7d83e0ac804b9e5749bab38d3b25c05d6c2a0f5f54757a9` (Quotes). The pointer hit a visible span within a partly clipped button. Default mousedown focus revealed the button ancestor, moving the span before release; the resulting click targeted another common ancestor instead of activating the tab.

The native repair passes the existing preventScroll option through the focus transition for pointer-default focus only. Explicit script focus still uses its own reveal policy; canceled mousedown prevents default focus and reentrant focus requests retain their cancellation behavior. Hit testing, coordinates, application source and test assertions are unchanged.

## Exact candidate and validation

- Source `7973176bd07ae3807b85a34d8bf6316ebafdd7e3`, tree `b1944c8a155ea0114f7c79c2cbf6269fb8b6ad8b`, based on main/PR136 `9f21abd5ee7e161f5de138080461c3b07ce9903a`.
- Signed locked release binary SHA256 `098a98f994f1d9735591fa9c25b86bc7d92aec8900023406cb0041640431ebc9`; compiler/SDK/cwd/command, strict signature and allow-jit evidence retained. Polling traps enabled, shared/private caches 64 MiB, no diagnostic environment.
- New clipped-button regression fails before the repair because scroll offsets change during press, passes after. Both new regressions pass, including fully visible control, completed click, canceled mousedown and reentrant explicit focus reveal. All 149 DOM binding tests pass; strict WebAPI release/all-targets clippy, fmt and no-V8 checks pass.
- Broader WebAPI release attempt: 208 passed, one existing XHR upload-order assertion failed because it observed two identical final download-progress notifications rather than one. The unchanged test then passed alone and all 22 XHR tests passed together. Both the initial failure and follow-ups are retained; this is not an uninterrupted full-suite pass and no XHR production/test change is included here.
- W3's earlier paired reduced control is retained from diagnostic-only `14729e147d4fcbbe73f7b3cc3cdda8a9678fea1b`: Chrome clicked both controls, frozen rewrite missed the clipped control. On this candidate the same probe (only the local Playwright import path adapted) clicks both controls, with ancestor offsets unchanged. These are functional results across different worker hosts, not performance comparisons.

## Unchanged application follow-up

Both complete route files passed **13/13 canonical common cases**, zero skips/retries/flaky results, against exact TOS `e6cf6953f456a82df30e037620afb314a99abc85`. Both source files are byte-identical to original `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`; all selected identities and source hashes were verified. Two native-only cases remain outside this selection. Backend-token localStorage auth, direct-lane version gate bypass, retained same-date template and fresh run-owned clones follow the established setup. The pre-run database set was restored; zero run databases remained.

The run began 2026-10-19T04:47:16.822Z; Playwright duration 26.291s, whole harness 29.276s. These are functional timings, not a browser-speed claim. Full original frozen742 evidence remains 729 passed/13 failed; these targeted results do not replace it or establish a new aggregate pass.

The separate Quote autocomplete pair is addressed by W3's narrow TOS readiness delta, with paired whole-file 5/5 evidence at `c8f127b92b0a76dcf56b90be97768dd2c630e262`, `evidence/final-followup-w3/quote-followup/`. That test change is not part of this native PR. Final common-candidate integration and a fresh complete742 run remain required.
