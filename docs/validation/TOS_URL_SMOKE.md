# TOS URL integration smoke, 2026-10-17

The unchanged configuration settings test passes. The profile test passes its URL
assertion but still receives title `TOS` instead of `My Profile`. Its browser
console stack points to function T at useFormAutosave-CQW0w-Ga.js:1:2724; the retained
built chunk maps this function to `e => structuredClone(e)`. The current browser
has no structuredClone binding. The previous missing URL exception is gone.

- Browser: `cf5a5ad1d49aa01a18ad1994f703362580103847`, release.
- Binary SHA-256: `576dcbf3e0660eb5d22be822d0e16f55fa4043de522a64487ccad0d79a9a0b18`.
- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Host: MacBookAir.localdomain; run `lpurlobj-1017-120038`.
- Result: one passed, one failed; 14.246 seconds in Playwright, 52.882 seconds
  for the whole harness. No browser performance or full TOS compatibility claim.
- Same native PostgreSQL service/test role with isolated per-run databases,
  backend test-token localStorage authentication, documented direct-lane version
  bypass and unchanged profile-settings.e2e.ts assertions.

Artifacts: `/private/tmp/lp-tos-smoke-url-object`; exact browser worktree
`/private/tmp/lp-tos-browser-url-object`; fixture
`/private/tmp/lp-tos-f9f-url-object-smoke`. The built failing chunk is retained in
its `.tmp/fe-e2e-portless/fe-dist/assets` directory. Both worktrees were clean
before execution; the fixture afterward changed only its generated PDF model.
The sole leftover template was removed after exact owner/prefix/preabsence
checks; the test role then owned zero databases. Earlier runs are preserved.
