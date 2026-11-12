# CI recipes

Running the native suite in CI needs Bun, the package, your app's build, and usually your API. The browser comes in the npm package, so there is nothing else to install. This page gives GitHub Actions workflows for macOS and Ubuntu, then covers caching, sharding, JUnit reports, screenshot baselines and how much concurrency to ask for.

## Runners

| Runner | Browser | Fork mode |
|---|---|---|
| `ubuntu-24.04` (x64) | Bundled Linux x64 build | Yes |
| `ubuntu-24.04-arm` (arm64) | Bundled Linux arm64 build | Yes |
| `macos-15` (Apple silicon) | Bundled macOS arm64 build | Yes |
| `macos-15-intel` | Bundled macOS x64 build | No: every page boots its profile |

The Linux builds need glibc 2.39 or newer: Ubuntu 24.04 has it, Ubuntu 22.04 does not. The project's own release pipeline smoke-tests the package on exactly these four runners.

## A complete workflow

This job builds the front end, starts the API in the setup module, serves the build with `--static`, and runs the suite with `pyro e2e`.

```yaml
# .github/workflows/e2e.yml
name: E2E

on:
  pull_request:
  push:
    branches: [main]

jobs:
  e2e:
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v6
      - uses: oven-sh/setup-bun@v2
      - uses: actions/cache@v4
        with:
          path: ~/.bun/install/cache
          key: bun-${{ runner.os }}-${{ runner.arch }}-${{ hashFiles('bun.lock') }}
      - run: bun install --frozen-lockfile
      - run: bun run build                         # your front-end build, e.g. into apps/web/dist
      - name: E2E
        run: bunx pyro e2e e2e --setup e2e/setup.ts --timeout 30000 --junit junit.xml
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: junit
          path: junit.xml
```

The setup module starts what the tests need and hands the browser its flags:

```ts
// e2e/setup.ts
import type { Setup } from '@povio/pyro/e2e';

type Runtime = { origin: string; api: ReturnType<typeof Bun.spawn> };

export default {
  async setup() {
    const api = Bun.spawn(['bun', 'run', 'api:start'], { stdout: 'inherit', stderr: 'inherit' });
    await waitFor('http://localhost:3000/api/health');     // your readiness check
    return { origin: 'http://localhost:3000', api };
  },
  browser: (rt) => ({ args: ['--static', `${rt.origin}=${process.cwd()}/apps/web/dist`] }),
  teardown: (rt) => rt.api.kill(),
} satisfies Setup<Runtime>;

async function waitFor(url: string) {
  for (let i = 0; i < 300; i++) {
    try { if ((await fetch(url)).ok) return; } catch {}
    await Bun.sleep(100);
  }
  throw new Error(`${url} did not come up`);
}
```

Replace `api:start` and the health URL with your own. With `--static`, the API server answers `/api/...` on the same origin while the browser serves the build from disk; no front-end server runs. See [Serving a build: --static](/e2e/static).

`teardown` also runs when the run is interrupted, so a cancelled job does not leave the API running. The browser exits with the runner.

## macOS

The same job runs on `macos-15` by changing `runs-on`. For both:

```yaml
jobs:
  e2e:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, macos-15]
    runs-on: ${{ matrix.os }}
```

Keep separate screenshot baselines per OS (see [below](#screenshot-baselines)).

## Caching

- **Bun's install cache** (`~/.bun/install/cache`), keyed on `bun.lock`, as above. The package is large because it holds the browsers for all four platforms; the cache saves downloading it on every job.
- **Your front-end build**, if it is slow, with your build tool's own cache.
- **`timings.json`**: commit it, so every shard computes the same split (below).

There is no browser download step to cache: the binaries are inside the npm package.

## Sharding with timings

`--shard i/n` splits the spec files across jobs by their recorded durations, longest first onto the least loaded shard. Every shard computes the same split from the same `timings.json`.

```yaml
jobs:
  e2e:
    runs-on: ubuntu-24.04
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3, 4]
    steps:
      - uses: actions/checkout@v6
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - run: bun run build
      - run: >
          bunx pyro e2e e2e --setup e2e/setup.ts --timeout 30000
          --shard ${{ matrix.shard }}/4 --timings e2e/timings.json
          --junit junit-${{ matrix.shard }}.xml
      - uses: actions/upload-artifact@v4
        if: always()
        with:
          name: junit-${{ matrix.shard }}
          path: junit-${{ matrix.shard }}.xml
```

Refresh the timings from a full, unsharded run now and then, and commit the file:

```sh
bunx pyro e2e e2e --setup e2e/setup.ts --timings e2e/timings.json --update-timings
```

`--update-timings` merges this run's durations into the file. It does not write when `-t` or `--only` filtered tests or when the run stopped early. Files without a recorded duration count as the average, so new files do not break the split.

## JUnit reports

`--junit <file>` writes one `testsuite` per spec file and one `testcase` per test. Hook failures, files that failed to load and teardown failures appear as test cases of their own. Upload the file as an artifact (as above) and feed it to whatever test-report action or service you use.

The console output is readable in the job log too: one line per test as it finishes, failures in full, then a summary. The exit code is non-zero on any failure.

## Screenshot baselines

Rendering is deterministic for one OS, but text that uses system fonts renders differently on macOS and Linux. Keep a baseline directory per platform:

```ts
import { join } from 'node:path';
const dir = join(import.meta.dir, '__screenshots__', process.platform);   // darwin / linux
await page.run([matchScreenshot('home', { dir })]);
```

Record the Linux baselines on the runner image CI uses. One way is a manually triggered job that runs the visual tests with `-u` and uploads the directory:

```yaml
on:
  workflow_dispatch:

jobs:
  baselines:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v6
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - run: bun run build
      - run: bunx pyro e2e e2e/visual --setup e2e/setup.ts -u
      - uses: actions/upload-artifact@v4
        with:
          name: linux-baselines
          path: e2e/visual/__screenshots__/linux
```

Review the images, then commit them. On a mismatch in a normal run, upload `**/*.actual.png` and `**/*.diff.png` as artifacts to see what changed. See [Visual tests](/e2e/screenshots).

## Concurrency

`-j` defaults to the machine's CPU count. In fork mode every running test is its own process running your app's JavaScript, so the suite is mostly CPU-bound; the right `-j` depends on how long tests wait on your API compared to how long they compute.

- Start with the default and measure wall time at a few values. On a 10-core M1 Max, the 2,158-test ToS suite took 362 s at `-j 12` and 313 s at `-j 16` (PR #289); its tests wait on a real API and Postgres.
- `--max-concurrency 1` keeps the tests of each file sequential while files overlap, for suites whose files share server state.
- The runner raises the server's connection limit to at least `4 × -j + 8` (minimum 64), so you do not need to.
- Shard across jobs when one runner's CPUs are saturated.

On a runner without fork mode (`macos-15-intel`), every page boots the profile, so lower `-j` and expect much longer runs.

## Without the suite runner

With plain `bun test`, launch the server in the test files (`Pyro.launch()`) or share one fork-mode server across `bun test --parallel` workers ([One server, many workers](/e2e/shared-server)). `bun test --reporter=junit --reporter-outfile=junit.xml` writes Bun's own JUnit report. A Playwright-layer suite runs with `pyro run` instead; see [Config and the test runner](/playwright/runner).

## Checklist

- `ubuntu-24.04` or newer (glibc 2.39), or `macos-15`.
- `oven-sh/setup-bun`; no Node needed for the native suite.
- The app's build served with `--static`, the API started in `setup()` and stopped in `teardown()`.
- Server stderr visible in the job log (it is by default): failed profile setups are printed there.
- `timings.json` committed when sharding.
- Per-OS screenshot baselines.
