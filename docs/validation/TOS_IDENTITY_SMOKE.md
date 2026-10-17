# TOS identity integration smoke, 2026-10-17

The unchanged profile/settings smoke advanced past storage-state restoration and
page creation, then failed both tests. Playwright's injected script requires
MutationObserver at the first toHaveURL assertion. The application module also
reported `ReferenceError: Can't find variable: URLSearchParams` on both routes.
These are distinct WebAPI gaps. No assertions were weakened and no TOS success
or browser performance improvement is claimed.

- Browser commit: `166277a3873060c23ec134f911a7e12dc2108fe9`, release.
- Binary SHA-256: `9c03f233e727ad7d0f42329d60b5b62f71ab871d981fcba9b129e4dbc2533803`.
- TOS commit: `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`, clean before execution.
- Host: MacBookAir.localdomain, same host as the retained upstream baseline.
- Run: `lpua-1017-112050`, zero passed, two failed, zero skipped.
- Playwright phase: 4.979 seconds. Whole harness: 41.911 seconds. These include
  harness/application work and are not browser CPU, RSS or latency benchmarks.
- Test: `e2e/configuration/profile-settings.e2e.ts`, unchanged two tests.
- Setup: existing native PostgreSQL service, isolated run database/template;
  backend test-token localStorage authentication, not UI login; documented direct
  `scripts/run-fe-e2e.mjs` lane with `playwright.routed.config.ts` version gate bypass.

Retained local artifacts on the baseline host:
`/private/tmp/lp-tos-smoke-identity` (sanitized log/report, metadata, toolchain,
pre/post statuses, exact command, DB ownership checks); exact browser checkout
`/private/tmp/lp-tos-browser-identity`; TOS checkout
`/private/tmp/lp-tos-f9f-identity-smoke`. Only the harness-generated PDF model
changed after execution. The sole leftover snapshot template was removed after
checking its run prefix, owner and pre-run absence; the test role then owned zero
DBs. Earlier a7dfe98 and 86ff4ee binaries/reports and the upstream baseline remain
unchanged. The upstream reference is `2ad38773fd674d3c9cea26a263b814ee46f6ba84`;
its retained two-test pass is not a rewrite result.
