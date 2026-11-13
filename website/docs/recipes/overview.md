# Recipes

Complete, runnable how-tos for one task each. Every recipe shows all of its files, was run end to end against a Pyro build, and pastes real output. Start from the one closest to what you are building.

::: tip Before the first `@povio/pyro` release
The recipes use the new names (`@povio/pyro`, `pyro run`, `Pyro`). Until the package is published under that name, install it as described in [Before the first `@povio/pyro` release](/guide/installation#try-today).
:::

## Coming from Playwright

You have a Playwright suite and want it to run on Pyro without rewriting tests.

| Recipe | What it covers |
| --- | --- |
| [Sign in once with a setup project's storageState](/recipes/playwright-setup-project-auth) | Why a setup project fails under `pyro run`, and a `globalSetup` that signs in once and shares the state with every test. |
| [Run a large suite in CI without --shard](/recipes/playwright-ci-without-sharding) | Sizing workers, splitting files across CI jobs by size or tag, a GitHub Actions matrix. |
| [Run the same suite on Chromium and Pyro](/recipes/playwright-side-by-side) | One switch for the browser, separate baselines, a script that lists tests whose results differ, and fair timing. |
| [Triage the first red run](/recipes/playwright-triage-first-failures) | Checks before the first run, grouping failures, and the six failures you are most likely to see, with fixes. |

## Fastest E2E tests

You are writing a new suite in the native `page.run([...])` style.

| Recipe | What it covers |
| --- | --- |
| [Project layout for a new E2E suite](/recipes/e2e-project-layout) | Folders, scripts, a setup module that starts the API and seeds, one profile per role, specs, JUnit. |
| [Signing in once](/recipes/e2e-login-once) | Sign in once per role in fork mode, one snapshot per feature with `base`, and keeping tests independent. |
| [API seeding and cleanup](/recipes/e2e-api-seeding-and-cleanup) | Unique data per test, `defer` cleanup, a leak check, and asserting request bodies with `check.request`. |
| [Mocking backend states](/recipes/e2e-mock-backend-states) | Error, empty and slow states with `mock`, `hold` and `release`, so spinners and banners are tested deterministically. |
| [Visual regression in CI](/recipes/e2e-visual-regression-ci) | Baselines per OS, updating them, and uploading the diff images when a check fails. |

## Scraping and automation

You keep signed-in identities on disk and run jobs with them.

| Recipe | What it covers |
| --- | --- |
| [A pool of signed-in identities](/recipes/scraping-identity-pool) | Stored profiles with a proxy and user agent each, least-recently-used assignment, a limit per identity, re-sign-in on expiry, atomic saves. |
| [Fork-mode signed-in jobs](/recipes/scraping-fork-signed-in-jobs) | One booted profile per identity on `pyro serve --fork`; every job a fork that starts signed in. When to choose this over state files. |
| [Data from single-page apps](/recipes/scraping-spa-data) | Why a page can look empty after `goto`, and five ways to get data loaded by `fetch`, plus pagination and infinite scroll. |
| [A batch of URLs to PDF](/recipes/scraping-pdf-batch) | Bounded concurrency, A4 and print media, embedded fonts, and checking each PDF's text. |
| [Moving profiles between machines securely](/recipes/scraping-move-profiles-securely) | Encrypting profiles at rest, using them on another machine, key rotation and revocation. |
