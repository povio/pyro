# Run a large suite in CI with --shard

`pyro run --shard i/n` splits a suite across CI jobs as `playwright test --shard` does. This recipe shows how to use it on GitHub Actions: size the workers of one job first, then, if one machine is not enough, shard across a job matrix. Every job computes the same split from the same checkout.

The workflow file was not run on GitHub for this page.

## What is and is not supported

| You have in Playwright | Under `pyro run` |
| --- | --- |
| `--shard=1/4` | Supported, with Playwright's split. `shard: { current, total }` in the config works too. |
| `--workers` / `-j` | Supported, as a number or a percentage of the CPUs. Default: the config's `workers`, else half the CPUs. |
| `--grep`, `--grep-invert`, tags | Supported. Usable to split by tag. |
| `--retries`, `--max-failures`, `-x` | Supported. |
| Setup projects (`dependencies`, `teardown`) | Supported. Each shard runs the dependencies it needs. |
| `--reporter=html`, `junit`, `json`, blob reports, `merge-reports` | Not supported. Reporters are `list`, `line` and `dot`, printed to the log. |
| Traces | Not supported. The `trace` option is accepted and records nothing. |
| Screenshot on failure (`screenshot`), videos (`video`) | Supported. The files land in `test-results/`, beside the `-actual`, `-expected` and `-diff` images of screenshot mismatches. Video needs ffmpeg on the runner. |
| `npx playwright install --with-deps` | Not needed. The browser is in the npm package. |
| Exit status | 1 when a test failed or a worker crashed, 0 otherwise. 2 for an unknown flag. |

The native runner `pyro e2e` has `--shard` and `--junit` too, but it runs native tests, not Playwright ones. See [CI recipes](/e2e/ci).

## Step 1: size the workers of one job

Each `pyro run` worker is a `bun test` process with its own Pyro browser, and your app server runs on the same machine. Start with the default, then try more, and keep the fastest:

```sh
bunx pyro run --reporter dot             # half the CPUs
bunx pyro run --reporter dot -j 100%     # one worker per CPU
```

On a GitHub runner, `nproc` prints the CPU count. Files are dealt out whole, largest first, so a few very large files limit how far workers help. Split those files, or mark their describes `parallel`, before you add jobs.

A suite that needed eight shards on Chromium often needs fewer here. Measure one job before you copy the old matrix.

## Step 2: shard across jobs

When one machine is not enough, run the same command in N jobs, each with its own part:

```sh
bunx pyro run --shard 1/4
bunx pyro run --shard 2/4
bunx pyro run --shard 3/4
bunx pyro run --shard 4/4
```

How the split works:

- The runner lists the tests of the selected projects, in order, and cuts the list into N parts balanced by **test count**. It does not know how long a test takes.
- A file's tests stay together, in the part where the first of them falls. Tests that are parallel (`fullyParallel`, or a describe with `mode: 'parallel'`) are split one by one, so a fully parallel suite splits more evenly.
- The result depends only on the checkout and the flags, so the jobs agree without talking to each other. Give every job the same `--project`, `--grep` and file filters.
- A project's dependencies are not split: each shard runs the setup projects it needs, in full. See [Sign in once with a setup project's storageState](/recipes/playwright-setup-project-auth).
- A shard with no tests prints `pyro run: shard 4 of 4 has no tests` and exits with status 0, so more jobs than files is safe.

A sharded run starts with a line such as `Running 3 files using 4 workers, shard 2 of 4`. `--list` together with `--shard` shows a part's tests without running them:

```sh
bunx pyro run --shard 2/4 --list
```

If one job is always the slowest, the test counts hide a few slow tests. Move them to a file of their own, or give them a tag and their own job (below).

## The workflow

```yaml
# .github/workflows/e2e.yml
name: E2E

on:
  pull_request:
  push:
    branches: [main]

jobs:
  e2e:
    runs-on: ubuntu-24.04             # the Linux browser needs glibc 2.39+
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3, 4]
    steps:
      - uses: actions/checkout@v4
      - uses: oven-sh/setup-bun@v2
      - uses: actions/cache@v4
        with:
          path: ~/.bun/install/cache
          key: bun-${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('bun.lock') }}
      - run: bun install --frozen-lockfile
      - name: E2E (shard ${{ matrix.shard }} of 4)
        run: bunx pyro run --shard ${{ matrix.shard }}/4 --reporter dot --retries 2 -j 100%
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: test-results-${{ matrix.shard }}
          path: test-results/
          if-no-files-found: ignore
```

What each part is for:

- **`ubuntu-24.04`.** The bundled Linux browsers need glibc 2.39 or newer. `ubuntu-22.04` does not have it.
- **No browser install step.** Remove `npx playwright install --with-deps` and any `~/.cache/ms-playwright` cache.
- **The Bun cache.** The package carries the browsers for all four platforms, so it is large. Caching `~/.bun/install/cache` keyed on `bun.lock` saves downloading it in every job. If you install with npm, cache `~/.npm` keyed on `package-lock.json` instead.
- **Every job runs `webServer`, `globalSetup` and the setup projects itself.** Each job is its own machine, so each starts the app and signs in once.
- **`test-results/`** holds the images of screenshot mismatches and, with `use: { screenshot: 'only-on-failure' }` or `use: { video: 'retain-on-failure' }`, the screenshots and videos of failed tests. There is no HTML report or trace to upload. Video needs ffmpeg: if the runner image has none, add a step with `sudo apt-get install -y ffmpeg`. See [Video](/playwright/runner#video).
- **The pass/fail summary** is in each job's log. There is no JUnit file for GitHub's test summary, and no report merging across jobs, so remove a `merge-reports` job.

Change `4` in two places (the matrix and the command) to change the number of jobs.

## Splitting by tag instead

If a group of tests is slow or needs its own environment, tag it and give it its own job. Tags are part of what `--grep` matches:

```ts
test('says "Saved" when done', { tag: '@slow' }, async ({ page }) => {
  // ...
});
```

```sh
bunx pyro run -g @slow                  # the tagged tests
bunx pyro run --grep-invert @slow       # everything else
```

```text
Running 4 files using 5 workers

  ✓   1 e2e/form.spec.ts › says "Saved" when done (270ms)

  1 passed (0.3s)
```

The output above is from a four-file suite. With a tag split, every job still loads every test file to find its tests.

## Screenshot baselines in CI

Baselines carry the platform in their file name (`-linux`, `-darwin`), and Pyro renders with the operating system's fonts. Write the Linux baselines on the CI image, review them, and commit them. Run CI with `-u none` so a missing baseline fails instead of being written. See [Screenshots and snapshots](/playwright/snapshots#re-baselining-on-migration).

## Next

- Before you switch CI over, run both browsers on the same suite and compare: [Side by side](/recipes/playwright-side-by-side).
- The first red run: [Triage the first failures](/recipes/playwright-triage-first-failures).
- All runner flags: [Config and the test runner](/playwright/runner#sharding).
