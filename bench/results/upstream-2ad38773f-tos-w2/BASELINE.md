# Upstream Lightpanda baseline on TOS (Worker 2 host)

Copied from the Worker 2 handoff (`~/pyro-bench/BASELINE-HANDOFF.md`), sanitized: no credentials, tokens or secret contents. Raw logs and Playwright JSON reports stay on the Worker 2 host under `~/pyro-bench/<run>/` (paths below) and are not committed because they contain local environment details.

**Canonical configuration for future rewrite comparisons: B (TOS f9fde4aa0d, unmodified).** Comparisons must use the same TOS revision, the same specs/assertions/authentication, the same host and build conditions, and report both binary hashes. Whole-harness and browser-process timing/RSS are reported separately.

# Upstream Lightpanda baseline handoff - TOS FE E2E Lightpanda lane (Worker 2, this Mac)

Generated 2026-10-17. Host: macOS 26 (Darwin 25.6.0) arm64, 8 cores / 8 GiB, native PostgreSQL 17.11 (Homebrew) at localhost:5432, role `pyro_bench` (CREATEDB, not superuser). Bun 1.4.3 (`/opt/homebrew/bin/bun`, matches repo `packageManager: bun@1.4.3`).

Labels: **[measured]** = observed in this host's logs/commands; **[inferred]** = reasoning, not directly measured.

## Upstream Lightpanda binary (used for both A and B)

- Path: `/Users/softwarefarm/pyro-bench/browser/zig-out/bin/lightpanda` (92,909,656 B, mtime 2026-10-17 10:39)
- `lightpanda version`: `1.1.0-nightly.10716+2ad38773f` [measured]
- sha256: `7f76ccfece22e7130d39c8884321f5dea51f4bb2e195542944c3440a86dc4339` [measured, re-verified 2026-10-17 10:48]
- Source checkout `~/pyro-bench/browser` HEAD `2ad38773fd674d3c9cea26a263b814ee46f6ba84`
- **W3 reported a differing sha256 `cf1545da…` for the same nominal version.** Cross-host byte reproducibility is NOT established. Any performance/compat comparison must use binaries built and run on the same host.
- Note: TOS routing manifest `apps/fe/e2e/suites/lightpanda.json:697` pins `"lightpandaVersion": "1.0.0-nightly.9610+744bf768c"`, and `scripts/run-fe-e2e-routed.mjs:122` rejects other versions. Both A and B call `scripts/run-fe-e2e.mjs` directly with `playwright.routed.config.ts`, which bypasses that version preflight. So these runs use an unvalidated (newer) upstream build on purpose.

## Configuration A - "TOS 37ff64fe9a + native-PG infra patch from a384f409a8 (worktree-only)"

- Source commit: `37ff64fe9ab00505d5527f8b1eb356a7dac38069` (Merge PR #1472)
- Worktree: `~/pyro-bench/tos-lpbase-20261017102215`, run slug `lpbase-20261017102215`, artifacts `~/pyro-bench/lpbase-20261017102215/`
- Patch: `~/pyro-bench/lpbase-20261017102215/native-pg-a384f409a8.patch`
  - sha256 `94175bbb0664140c7d387ed8303ceb3041e070ada915eb92ef334fde49a03546` [measured]
  - `git show` of `a384f409a89a21826020b0dcf261b220fa3c2f91` ("fix(e2e): reuse native PostgreSQL on development workers"). It touches 2 files: `apps/be/scripts/e2e/database-snapshot.mjs` and `scripts/run-fe-e2e.mjs`.
  - `git diff` of those 2 files in the A worktree matches the patch body byte-for-byte. Only the 6-line commit header differs [measured].
- `git diff --stat` in the A worktree (current) [measured]:
  ```
   apps/be/scripts/e2e/database-snapshot.mjs    |  7 +++++--
   apps/pdfgen/src/data/common/common.models.ts | 14 +-------------
   scripts/run-fe-e2e.mjs                       |  7 +++++--
   3 files changed, 11 insertions(+), 17 deletions(-)
  ```
  The `apps/pdfgen/.../common.models.ts` change is not part of the patch. It is produced by the harness build/codegen step: the identical file became modified during the clean B run [measured in B]. Attributing A's change to the same cause is [inferred].
- Smoke results (from the previous run, recorded in the logs):
  - `e2e/auth/login.e2e.ts`: **No tests found** (Playwright exit 1), 40.14 s real, max RSS 1,689,485,312 B (`smoke.log`)
  - `e2e/configuration/profile-settings.e2e.ts`: **2 passed** (3.5 s Playwright), 26.69 s real, max RSS 1,750,646,784 B (`smoke-profile-settings.log`, report `lightpanda-profile-settings.json`)
  - Lightpanda-process RSS was not sampled for A.

## Configuration B - "TOS f9fde4aa0d unmodified"

- Commit `f9fde4aa0dce7d1d7a1ffebffe3306399973cd80` (Merge PR #1666, 2026-10-15). It was already present locally, so no fetch was needed. `a384f409a8` is an ancestor. Between a384f409a8 and f9f the native-PG files differ only by 3 cosmetic lines (Docker image 17→18-alpine default, `pg_isready -h 127.0.0.1` in Docker mode, an error message). The native path is the same.
- Run slug `lpbase-f9f-20261017104836`. Worktree `~/pyro-bench/tos-lpbase-f9f-20261017104836` (detached at f9f). Artifacts are in `~/pyro-bench/lpbase-f9f-20261017104836/`.
- `bun install --frozen-lockfile`: exit 0, 1499 packages (`bun-install.log`)
- `git status --porcelain` before run: **empty (clean)** (`git-status-pre.txt`). After run: ` M apps/pdfgen/src/data/common/common.models.ts` (harness codegen, `git-status-post.txt`).
- Env vars confirmed at this commit (`scripts/run-fe-e2e.mjs:45-53,202,843,913,1794`, `apps/fe/playwright.routed.config.ts:6,20`, `apps/fe/e2e/fixtures/lightpanda.ts:8`): `FE_E2E_POSTGRES_MODE`, `FE_E2E_POSTGRES_PORT`, `POSTGRES_USER`, `POSTGRES_PASSWORD`, `FE_E2E_RUN_SLUG`, `FE_E2E_REUSE_TEMPLATE_DATABASE`, `E2E_TEMPLATE_DATABASE`, `E2E_DATABASE_SNAPSHOT_RESTORE_WITH_DOCKER`, `E2E_ROUTING_ENGINE`, `E2E_ROUTING_REPORT`, `LIGHTPANDA_EXECUTABLE`, `E2E_PLAYWRIGHT_WORKERS`. No changes were needed.

### B results [measured]

| Metric | Value |
|---|---|
| Tests | **2 passed**, 0 skipped/unexpected/flaky (report `lightpanda.json`, sha256 `606e0481…6dfd`) |
| Playwright duration | 4.0 s (report stats 4009 ms). Tests: 604 ms and 489 ms |
| Harness wall time (`/usr/bin/time -l`) | **41.74 s** real (27.15 user, 6.63 sys). Includes a runtime build cache miss and a template DB restore of 11.8 s |
| Whole-harness max RSS (`/usr/bin/time -l`) | **1,855,766,528 B**. Peak memory footprint 993,907,968 B |
| Lightpanda process RSS (ps sampling, 200 ms) | 1 process (pid 2856), 4 samples over about 1.05 s: 27,840 → 108,688 → 126,160 → **154,784 KiB (151.2 MiB) peak observed** |
| Exit | 0 |

RSS caveat: the Lightpanda process lived for only about 1 s. The samples were still rising when it exited, so 151.2 MiB is a **lower bound** on true peak RSS [inferred]. Whole-harness RSS (bun, Nest BE, Vite build, Playwright) is a different quantity and cannot be compared with browser RSS.

A vs B wall-time difference (26.69 s vs 41.74 s) [inferred]: mostly harness cache state (B had a fresh worktree, a runtime build cache miss and a template DB restore). It is not a browser difference. Playwright time was 3.5 s vs 4.0 s.

### Exact commands (B), password redacted

```sh
git -C /Users/softwarefarm/Documents/GitHub/tos-monorepo cat-file -t f9fde4aa0dce7d1d7a1ffebffe3306399973cd80   # commit
RUN=lpbase-f9f-20261017104836
git -C /Users/softwarefarm/Documents/GitHub/tos-monorepo worktree add --detach ~/pyro-bench/tos-$RUN f9fde4aa0dce7d1d7a1ffebffe3306399973cd80
cd ~/pyro-bench/tos-$RUN && perl -e 'alarm 900; exec @ARGV' bun install --frozen-lockfile
git -C ~/pyro-bench/tos-$RUN status --porcelain   # empty
# runner: ~/pyro-bench/$RUN/tools/run-smoke.sh $RUN (starts the RSS sampler, then, in the worktree root):
FE_E2E_POSTGRES_MODE=native FE_E2E_POSTGRES_PORT=5432 POSTGRES_USER=pyro_bench \
POSTGRES_PASSWORD='<redacted; read from ~/pyro-bench/pg_lp_bench.pass>' \
FE_E2E_RUN_SLUG=$RUN E2E_TEMPLATE_DATABASE=fe_e2e_${RUN//-/_}_template \
FE_E2E_REUSE_TEMPLATE_DATABASE=0 E2E_DATABASE_SNAPSHOT_RESTORE_WITH_DOCKER=0 \
E2E_ROUTING_ENGINE=lightpanda E2E_ROUTING_REPORT=$HOME/pyro-bench/$RUN/lightpanda.json \
LIGHTPANDA_EXECUTABLE=$HOME/pyro-bench/browser/zig-out/bin/lightpanda E2E_PLAYWRIGHT_WORKERS=1 \
perl -e 'alarm 1800; exec @ARGV' /usr/bin/time -l bun --no-orphans scripts/run-fe-e2e.mjs run \
  --config playwright.routed.config.ts e2e/configuration/profile-settings.e2e.ts
# RSS sampler: ~/pyro-bench/$RUN/tools/rss-sampler.sh "^$HOME/pyro-bench/browser/zig-out/bin/lightpanda serve" <out> <stopfile>
#   loop: pgrep -f <pattern> -> ps -o rss= -p <pid> every 0.2 s
```
Configuration A used the same command style on the 37ff64fe9a worktree with `RUN=lpbase-20261017102215` and `E2E_ROUTING_REPORT` pointing at its run dir.

## Authentication (B, f9f) - API token injected into storage state, no UI login

1. `profile-settings.e2e.ts:14-16`: `test.beforeAll` calls `startWorkerSession(browser)` (`fixtures/worker-session.ts:29-49`). There is no office or user isolation, so the session uses the seed office and the default user `fe-e2e.master-data@example.com` (`fixtures/env.ts:18`, `scripts/fe-e2e-users.mts:18-19`).
2. `getAuthTokens` (`fixtures/auth.runtime.ts:207-267`) checks for `E2E_AUTH_TOKENS`, which the runner does not set. It then makes an HTTP GET to `${apiUrl}/auth/provider/local/magic-link/test-token?email=…` (`:237-248`). That BE endpoint is only allowed when `STAGE=test` (`apps/be/src/common/authn/providers/local/magic-links/rest/local-authn-magic-link.rest.controller.ts:71-77`). On 404/403 it falls back to a `bun --filter be magic-link` CLI and token exchange (`:249-252`, `:184-202`). The token must have `userType === "employee"` (`:287-291`).
3. `createAuthenticatedContext` (`auth.runtime.ts:525-529`): the routed config sets `E2E_FRESH_AUTH_CONTEXT=1` for lightpanda (`playwright.routed.config.ts:9-11`), so `createFreshAuthenticatedContext` (`:387-482`) runs. It calls `browser.newContext({ storageState: { origins: [{ origin: feUrl, localStorage: [...] }] } })` with `auth-custom-jwt-accessToken/-accessTokenExpiresAt/-refreshToken/-userType`, `routing-history-store` and `user-store` (`:374-385`, `:407-415`). It also adds an init script that re-applies those localStorage entries and removes other keys (`:424-450`), plus fixed-date and deterministic-style/`Math.random` init scripts.
4. The Lightpanda browser is the worker-scoped fixture `createLightpandaBrowser` (`fixtures/test.ts:47-63`, `fixtures/lightpanda.ts`). It spawns `lightpanda serve --host 127.0.0.1 --port <free> --log-level error --experimental-features cors --load-resources stylesheet --load-resources iframe` and then `chromium.connectOverCDP`. Each context gets one process and only one page at a time.
5. Navigation: `openRoute` (`fixtures/navigation.ts:5-25`). `/configuration/profile` and `/configuration/settings` are excluded from `needsShell`, so both tests use a plain `page.goto`.
6. Implicit check: `E2E_DOM_ACTIVATION=1` (lightpanda) makes `_e2eRuntime` run `assertNoUnexpectedBrowserErrors()` after each test (`fixtures/test.ts:84-87`).

## Assertions (`apps/fe/e2e/configuration/profile-settings.e2e.ts` at f9f; describe-level timeout 30 s, `mode: "parallel"`, line 10)

Test 1, `renders the employee profile with its editable settings` (line 22). It opens `${feUrl}/configuration/profile` (line 26), then:
- L28 `await expect(page).toHaveURL(/\/configuration\/profile\/?$/);`
- L29 `await expect(page).toHaveTitle("My Profile");`
- L30 `await expect(page.getByRole("heading", { name: "My Profile" })).toBeVisible();`
- L31 `await expect(page.getByRole("textbox", { name: "First Name" })).toBeVisible();`
- L32 `await expect(page.getByRole("textbox", { name: "Last Name" })).toBeVisible();`
- L33 `await expect(page.getByRole("textbox", { name: "Email" })).toBeDisabled();`
- L34 `await expect(page.getByText("Invoice signature", { exact: true })).toBeVisible();`

Test 2, `renders the configuration settings panel` (line 40). It opens `${feUrl}/configuration/settings` (line 44), then:
- L46 `await expect(page).toHaveURL(/\/configuration\/settings$/);`
- L47 `await expect(page.getByText("Settings", { exact: true }).filter({ visible: true }).first()).toBeVisible();`
- L48 `await expect(page.getByRole("heading", { name: "MarineTraffic" })).toBeVisible();`
- L49 `await expect(page.getByRole("button", { name: "Save settings" }).first()).toBeDisabled();`

Neither test performs a mutation or form submit. Both are render/role/state assertions only.

## Cleanup manifest (B)

Before the run, `pyro_bench` owned 0 databases [measured].

Databases created during the run (from the log):
| Database | Disposition |
|---|---|
| `fe_e2e_lpbase_f9f_20261017104836_template` | dropped by harness (`FE_E2E_REUSE_TEMPLATE_DATABASE=0`) |
| `fe_e2e_lpbase_f9f_2026101710483_1rw_0` | dropped by harness cleanup |
| `fe_e2e_lpbase_f9f_2026101710483_1rw_0_standby` | never created ("does not exist, skipping") |
| `fe_e2e_lpbase_f9f_20261017104836_template_bun_e2e_8f4d6e232f35` (owner pyro_bench, `datistemplate=t`, `datallowconn=f`) | **dropped by me**: `ALTER DATABASE … IS_TEMPLATE false ALLOW_CONNECTIONS true; DROP DATABASE … WITH (FORCE);` exit 0 (`db-cleanup.log`) |

After cleanup, `pyro_bench` owns 0 databases (`db-post-cleanup.txt`). No other databases or roles were touched.

## Resources left in place

- Role `pyro_bench` and secret `~/pyro-bench/pg_lp_bench.pass` (unchanged)
- Worktree A `~/pyro-bench/tos-lpbase-20261017102215` and its artifacts `~/pyro-bench/lpbase-20261017102215/`
- Worktree B `~/pyro-bench/tos-lpbase-f9f-20261017104836` (registered in the tos-monorepo worktree list; only harness-generated `common.models.ts` is modified) and artifacts `~/pyro-bench/lpbase-f9f-20261017104836/`: `smoke-profile-settings.log` (sha256 `2d42f801…9f4b`), `lightpanda.json`, `lightpanda-rss-samples.txt`, `lightpanda-rss-peak.txt`, `bun-install.log`, `git-status-pre.txt`, `git-status-post.txt`, `db-pre-cleanup.txt`, `db-cleanup.log`, `db-post-cleanup.txt`, `smoke.start`/`smoke.end`, `tools/run-smoke.sh`, `tools/rss-sampler.sh`
- Harness temp/build outputs under each worktree's `.tmp/`
- Lightpanda binary `~/pyro-bench/browser/zig-out/bin/lightpanda` (unchanged)
