# TOS structured-clone integration smoke, 2026-10-17

Both unchanged profile/settings scenarios pass with zero browser errors, skips
or retries. This verifies the employee profile rendering assertions and the
configuration settings panel assertions in the selected file; it is not full
TOS compatibility, an editing/autosave proof, or a performance comparison.

- Browser build: `9963335e5bd8ae1984c9cbbce9aa217f27536cbd`, release.
- Binary SHA-256: `04238836db5fb4600ba14457910ad9a86a60dcf35a6d0b59203f56fdca835931`.
- Tree: `3acb0c59991383cd730d0504a23b85fc4395a6b0`, identical to approved PR27
  head `b8a9c153fa019171d3f482ba14a2eeecba07cf5d` and merged main
  `f537c86ee1c31b49e221cc13ee0d7a41ac96e713`. The history-only merge did not
  replace the pinned build.
- Fixture: unchanged TOS `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Host: MacBookAir.localdomain; run `lpclone-1017-121708`.
- Result: 2 passed, 0 failed, 0 skipped; 3.906 seconds Playwright, 42.111 seconds
  whole harness. Harness timing/RSS are not browser-only performance measurements.
- Native shared PostgreSQL with isolated run databases; unchanged direct-lane
  version gate bypass and backend test-token localStorage authentication.

Sanitized artifacts are retained at `/private/tmp/pyro-tos-smoke-clone`, including
metadata, exact command, binary hash/toolchain, report/log and database ownership
checks. Binary checkout: `/private/tmp/pyro-tos-browser-clone`. Fixture checkout:
`/private/tmp/pyro-tos-f9f-clone-smoke`. Both were clean before execution; afterward
only the fixture's generated PDF model changed. The sole leftover run template
was removed after owner/prefix/preabsence verification; test-role database count
was then zero. All prior pinned failures, including the [1/2 URL run](TOS_URL_SMOKE.md),
and the upstream baseline remain preserved. No assertions/auth/manifest edits.

Command uses the same documented direct lane as the original upstream baseline:

```sh
bun --no-orphans scripts/run-fe-e2e.mjs run --config playwright.routed.config.ts e2e/configuration/profile-settings.e2e.ts
```

The clone binding retains its explicit [conformance limits](../webapi/STRUCTURED_CLONE.md).
