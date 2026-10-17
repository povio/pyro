# TOS query-parameter integration smoke, 2026-10-17

Both unchanged profile/settings tests now pass their initial toHaveURL assertion.
They still fail: the profile title remains `TOS` instead of `My Profile`, and the
settings content is absent. Both routes report `ReferenceError: Can't find
variable: URL` from Vite's preload helper. A generic employee-user fetch error is
also logged; this report does not assign it a network/protocol root cause.
MutationObserver and URLSearchParams exceptions from the preceding run are gone.

- Browser: `26c496c7b636470255d87ee59205beefe5062593`, release; later viewport
  protocol and navigation repairs were not silently added to this candidate.
- Binary SHA-256: `a3ed2a365d46ef4012792d434177b542729a1623281ff0f8614332c47f91ca1c`.
- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Host: MacBookAir.localdomain; run `lpurl-1017-114311`.
- Result: zero passed, two failed; 24.689 seconds in Playwright, 62.657 seconds
  for the whole harness. This is functional evidence, not a browser performance
  measurement or full TOS compatibility claim.
- Same native PostgreSQL role/service with isolated per-run databases, backend
  test-token localStorage authentication, direct-lane documented version bypass,
  and unchanged `profile-settings.e2e.ts` assertions.

Retained artifacts: `/private/tmp/lp-tos-smoke-url` (sanitized log/report,
metadata, toolchain and DB ownership/preabsence checks), exact browser checkout
`/private/tmp/lp-tos-browser-url`, and fixture `/private/tmp/lp-tos-f9f-url-smoke`.
The fixture was clean before execution; only the harness-generated PDF model
changed afterward. Its sole leftover snapshot template was removed after exact
ownership/prefix/preabsence checks; the test role then owned zero databases.
All earlier pinned binaries, artifacts and upstream baseline are preserved.
