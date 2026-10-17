# TOS representative coverage batch 5, 2026-10-17

Batch 5 is 13 unchanged manifest-routed scenarios from three files not covered by
batches 1–4: chat notification settings, package-type master data
(CRUD/validation/sort) and role permission editing. The selection was recorded
before any run of these files.

| Binary | Run | Passed / failed |
| --- | --- | --- |
| main `8a50785` | `lpb5-1017-163317` | 9 / 4 |
| upstream `2ad38773` | `lpb5u-1017-163518` | 13 / 0 |
| `89ef88f` (PR #50) | `lpb5f-1017-164538` | 12 / 1 |
| integrated main `b0f0319` (PR #50 + PR #52) | `lpb5m-1017-165911` | 13 / 0 |

All four main failures are rewrite-specific:
- **Window [SameObject] expandos lost after GC (PR #50).** Affected cases:
  notification-settings "keeps browser registration hidden when subscription
  lookup fails" and "scopes message previews to the registered current browser".
  - The init scripts override `navigator.serviceWorker` and `crypto.subtle` with
    `Object.defineProperty`.
  - Those wrappers were only weakly identity-cached, so after collection the
    page saw fresh objects without the overrides. "Set up Push" was then shown, or
    disabled, wrongly.
  - Fix: these wrappers are pinned per realm.
- **Space activation (PR #50).** Affected case: roles-permissions "toggles a
  permission from the Permissions tab…", which timed out at 30 s.
  - Playwright's `keyboard.press('Space')` sends keyDown with text, which never
    clicked the focused switch checkbox.
  - Fix: Space now clicks focused buttons and checkable inputs on keyup, as in
    Blink.
- **CDP orientation override (W3 / lp-cdp).** Affected case: notification-settings
  "automatically saves changes on mobile".
  - `page.setViewportSize` sends `Emulation.setDeviceMetricsOverride` with
    `screenOrientation {angle: 0, type: "landscapePrimary"}` and a portrait
    390x844 screen, and the rewrite rejected that combination.
  - W3 reproduced it. Upstream stores the override for `screen.orientation`. The
    fix belongs to W3's lp-cdp increment, which will include a minimal
    `Viewport.orientation`/`Screen.orientation` change reviewed by W2.

Integrated main `b0f0319` contains PR #50 (with its review repair) and W3's
orientation PR #52. It passes all 13 cases, matching upstream.
- Release binary: SHA-256
  `6210596644b56bf0e5865ab5c66eef7374d114215908b7380dba218635b3d8a2`, built from a
  fresh `--no-hardlinks` clone with its own target directory.
- Fixture: a fresh, clean `/private/tmp/lp-tos-f9f-batch5-main`.
- Run: one Playwright worker and isolated native PostgreSQL. After cleanup the
  test role owned zero databases.
- Time: 28.4 s Playwright and 67.0 s whole harness.
- Artifacts: `/private/tmp/lp-tos-batch5-main`.

## Provenance

- TOS: unchanged `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80`.
- Routing manifest SHA-256 `542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19`.
- `selection.json` SHA-256
  `a44c56734c3415ca2dc6b2e3fad011421c582a2156841297c3d65fc8163fb6fa`.
  - It covers all manifest `common` entries for `chat/notification-settings`,
    `master-data/cargo/package-types` and `configuration/roles-permissions`.
  - Every run reported "Running 13 tests".
- Binaries. Rewrite binaries are release builds from fresh `--no-hardlinks`
  clones with isolated target directories. Every hash was verified before
  execution.

  | Binary | SHA-256 |
  | --- | --- |
  | `8a50785` | `21a75f57bbaaf768614f52aec3d6846cfc0f82ab3da495ef14a0d3d45b46dd34` |
  | `89ef88f` | `946dbc74667abbc1d1609b36251415e0952bd3fd792417d14b3b8e50e79af6c7` |
  | upstream | `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` |

- Fixture: one fresh `git worktree add --detach` (`/private/tmp/lp-tos-f9f-batch5`),
  clean before the main run.
  - Because disk space was low, the upstream and `89ef88f` runs reused this
    worktree.
  - Before those runs, its only change was the harness-generated
    `apps/pdfgen/src/data/common/common.models.ts`, which was left in place.
- Setup: one Playwright worker, native PostgreSQL 17 with the `lp_bench` role, and
  a per-run slug and template database. After each cleanup the test role owned
  zero databases.
- Host: MacBookAir.localdomain, Apple M1, 8 GiB.
- Time, whole harness and not a performance comparison:

  | Run | Wall time | Playwright |
  | --- | ---: | ---: |
  | `lpb5` | 109.7 s | 71.5 s |
  | `lpb5u` | 52.3 s | 19.2 s |
  | `lpb5f` | 58.1 s | 26.0 s |

  The main run's time includes three timeouts.
- Disk: before the `89ef88f` work, free space was low. The parent authorized
  removing only `deps`, `build`, `incremental` and `.fingerprint` under ten older
  completed browser clones' `target/release`. Retained binary SHA-256s were
  unchanged; the record is in `/private/tmp/lp-tos-cleanup-1017`.

Artifacts:
- `/private/tmp/lp-tos-batch5`
- `/private/tmp/lp-tos-batch5-up`
- `/private/tmp/lp-tos-batch5-fix`

Limits: the batch 1 caveats on router bootstrap, synthetic activation and
assertion depth apply. Single runs do not establish flake rates.
