# Suite runner: pyro e2e

`pyro e2e` runs a whole suite in one process. Tests from every spec file go into one queue that runs up to `--concurrency` of them at a time, so cases of different files interleave. They all share one fork-mode browser, which the runner launches on first use and always closes. Every profile therefore boots once per run, and every test forks its page from it.

Use it instead of `bun test --parallel` for large suites. `bun test` runs the files of a worker one after another, so overlapping tests of different files takes worker processes, and each worker launches its own browser and boots its own profiles.

It needs [Bun](https://bun.sh) on `PATH`: the CLI hands `e2e` over to Bun even when started from Node.

## A spec file

Specs import `describe`, `test`, the hooks and `expect` from `@povio/pyro/e2e` instead of `bun:test`, and get the browser from `browser()`:

```ts
// e2e/todos.test.ts
import { beforeAll, browser, defer, describe, expect, test } from '@povio/pyro/e2e';
import { button, check, label, testId, text } from '@povio/pyro';
import { app, signedInProfile } from './app';

describe('todos', () => {
  beforeAll(async () => {
    await signedInProfile();                       // shared by every file, registered once
    const pyro = await browser();
    await pyro.profile('todos', { base: 'app', url: `${app()}/`, setup: [check(label('New todo')).visible()] });
    defer(() => pyro.releaseProfile('todos'));       // after this describe's last test
  });

  test('adds a todo', async () => {
    await using page = await (await browser()).page({ as: 'todos' });
    const r = await page.run([
      label('New todo').fill('Buy milk'),
      button('Add').click(),
      check.request('POST', '/api/items').status(201),
      text(/^Added /).read('status'),
    ]);
    expect(r.values.status).toBe('Added Buy milk');
  });

  test('deletes a seeded todo', async () => {
    const item = await (await fetch(`${app()}/api/seed`, { method: 'POST', body: JSON.stringify({ title: 'Walk' }) })).json();
    await using page = await (await browser()).page({ as: 'todos', route: '/' });
    await page.run([
      button('Delete Walk').click(),
      button('Delete', { exact: true }).click(),
      check(testId(`item-${item.id}`)).count(0),
    ]);
  });
});
```

A profile that many files share is registered once per run. Module state is shared by every file (one process), so memoize the promise:

```ts
// e2e/app.ts
import { browser, setupResult } from '@povio/pyro/e2e';
import { button, check, label, text } from '@povio/pyro';

export const app = () => setupResult<{ app: string }>().app;

let signedIn: Promise<void> | undefined;
export function signedInProfile() {
  return (signedIn ??= (async () => {
    const pyro = await browser();
    await pyro.profile('signed-out', { url: `${app()}/` });
    await pyro.profile('app', {
      base: 'signed-out',
      url: `${app()}/`,
      setup: [label('Email').fill('ada@example.com'), button('Sign in').click(), check(text('Signed in')).visible()],
    });
  })());
}
```

```sh
$ bunx pyro e2e e2e --setup e2e/setup.ts -j 4 --timeout 10000 --junit junit.xml
pyro: CDP server listening on ws://127.0.0.1:49843/ (fork mode)
✓ e2e/settings.test.ts > opens settings [320.10ms]
✓ e2e/todos.test.ts > todos > adds a todo [159.22ms]
✓ e2e/todos.test.ts > todos > deletes a seeded todo [166.88ms]
✓ e2e/todos.test.ts > todos > runs alone [29.62ms]

 4 pass
Ran 4 tests across 2 files. [415.78ms]
```

The first tests of a run include the profile boots they waited for.

## Options

```text
pyro e2e [options] [path | dir | glob | filter ...]
```

| Option | Default | Meaning |
|---|---|---|
| `-j`, `--concurrency <n>` | CPU count | Tests running at once across all files |
| `--max-concurrency <n>` | no limit | Tests of one file running at once (as in `bun test`) |
| `--timeout <ms>` | `5000` | Default timeout of tests and hooks |
| `-t`, `--test-name-pattern <re>` | | Only tests whose name (describe names and test name joined by spaces) matches the regular expression |
| `--only` | | Only tests and describes marked `.only` |
| `--shard <i>/<n>` | | Run shard `i` of `n`; files are split by `--timings` |
| `--timings <file>` | | Per-file durations, `{"version": 1, "files": {"<path>": ms}}` |
| `--update-timings` | | Write this run's per-file durations to `--timings` |
| `--setup <module>` | | A setup module (see below) |
| `--preload <module>` | | Loaded before the specs; its top-level hooks apply to every test. Repeatable |
| `--browser-arg <arg>` | | Extra `pyro serve` argument. Repeatable |
| `--reporter junit` | console | Also write a JUnit report, to `--reporter-outfile` |
| `--reporter-outfile <file>` | | Where the JUnit report goes |
| `--junit <file>` | | Both at once |
| `-u`, `--update-snapshots` | | Write screenshot baselines instead of comparing |
| `--bail [n]` | | Stop after `n` failures (1 without a number) |
| `-h`, `--help` | | Print the usage |

Options that take a value also accept `--option=value`. An unknown option or a bad value prints the usage and exits with code 2.

### Paths

Arguments are files, directories, globs or filters:

- A **directory** is searched for files matching `*.test.*`, `*.spec.*`, `*_test.*` or `*_spec.*` with a `js`, `ts`, `jsx`, `tsx`, `mjs`, `cjs`, `mts` or `cts` extension, skipping `node_modules` and dot directories.
- A **file** runs as given, whatever its name.
- A **glob** (an argument with `*`, `?`, `[` or `{`) runs every file it matches. Use one for other naming schemes: `pyro e2e 'e2e/**/*.e2e.ts'`.
- Anything else is a **filter**: only discovered files whose path contains it run, as with `bun test`.

Without paths the runner searches the working directory. No spec files found is an error (exit code 1).

## The setup module

`--setup <module>` names a module that prepares the run. Export the functions one by one, or as a default export (type it with `satisfies Setup<T>`):

```ts
// e2e/setup.ts
import type { Setup } from '@povio/pyro/e2e';
import { startServer } from '../server';

type Runtime = { app: string; stop: () => void };

export default {
  async setup() {
    const server = startServer();
    return { app: server.url.origin, stop: () => server.stop(true) };
  },
  browser: () => ({ args: [] }),
  teardown(rt) {
    rt.stop();
  },
} satisfies Setup<Runtime>;
```

| Export | Called | Contract |
|---|---|---|
| `setup()` | Once, before any spec file loads | Start servers, seed data. Its (awaited) return value is the run's setup result |
| `browser(result)` | When the first test calls `browser()` | Returns `{ args?: string[]; bin?: string }`: extra `pyro serve` arguments (`--static`, `--proxy`, ...) and the browser binary (default `$PYRO_BIN`, else the bundled one) |
| `teardown(result)` | Once, after the run, also when it is interrupted | Stop what `setup` started. A failure counts as a hook failure |

Specs read the result with `setupResult<T>()`, even while they load. Without `--setup`, `setupResult()` throws.

The runner launches the browser with `--fork` when the binary supports it (it checks `pyro version` for `fork: true`), plus `--cdp-max-connections` set to the larger of 64 and `4 × concurrency + 8` unless you pass that flag yourself. Arguments from `browser(result)` come first, then `--browser-arg` values.

## Spec API

Everything below is exported from `@povio/pyro/e2e`. Signatures follow `bun:test`; `expect` is Bun's own.

| Export | Meaning |
|---|---|
| `describe(name, fn)` | A group. `fn` must register its tests synchronously |
| `describe.serial`, `.skip`, `.todo`, `.only`, `.if(cond)`, `.skipIf(cond)` | Modifiers, as below |
| `test(name, fn, timeout?)`, `it` | A test. `timeout` is a number or `{ timeout }` |
| `test.serial` | Runs alone in its describe (see [Serial tests](#serial-tests)) |
| `test.concurrent` | Same as `test`: every test is concurrent unless it is serial |
| `test.skip`, `test.todo(name, fn?)`, `test.only`, `test.if(cond)`, `test.skipIf(cond)` | As in `bun:test`; `.only` takes effect with `--only` |
| `beforeAll`, `afterAll`, `beforeEach`, `afterEach` | Hooks of the describe (or file) they are declared in |
| `setDefaultTimeout(ms)` | Default timeout of tests and hooks without their own |
| `browser()` | The run's browser (a `Pyro`), launched on first use |
| `defer(fn)` | Cleanup after what is running now |
| `setupResult<T>()` | What the setup module's `setup()` returned |
| `testInfo()` | `{ file, name, titlePath }` of the running test, or `undefined` |
| `expect` | Bun's `expect` |

Hooks and tests can be async, or take a `done` callback. `beforeEach` hooks run outer first, `afterEach` inner first. Registering a test from inside a test or hook throws.

### browser()

```ts
const pyro = await browser();
await using page = await pyro.page({ as: 'todos' });
```

The first call launches `pyro serve --fork` with the setup module's options. Every later call returns the same instance. The runner closes it when the run ends, also on `SIGINT`, `SIGTERM` and `SIGHUP`. If the runner is killed outright, the browser's `--parent-pid` watch ends it.

Pages a test opens through `browser()` are closed when the test ends, including a test that timed out, so a hung test does not keep its forked process. Still write `await using`: it closes the page as soon as the test body is done.

### defer()

`defer(fn)` runs `fn` once what is running now is done:

| Called from | `fn` runs |
|---|---|
| A test, or its `beforeEach`/`afterEach` | After that test |
| A describe's (or file's) `beforeAll`/`afterAll`, or while it registers | After that describe's last test and its `afterAll` hooks |

Later calls run first. The usual use is retiring a per-describe profile:

```ts
beforeAll(async () => {
  const pyro = await browser();
  await pyro.profile('orders', { base: 'app', url, setup });
  defer(() => pyro.releaseProfile('orders'));
});
```

## Scheduling

- All tests of all files share one queue; up to `-j` run at once. Tests start in file order (longest file first when `--timings` is given) and declaration order.
- A describe's `beforeAll` runs before its first test starts. Its other tests wait for it without taking a slot. Its `afterAll` runs after its last test.
- `--max-concurrency n` caps the tests of one file running at once. `-j 8 --max-concurrency 1` runs like `bun test --parallel=8 --max-concurrency=1`, with one browser instead of eight.
- A test that fails a `beforeAll` of its scopes fails with `HookError: beforeAll of describe '...' failed: ...`.

### Serial tests

`test.serial` runs alone in its describe: after every test declared before it has finished, and before any test declared after it starts. Tests of other describes and files keep running beside it. `describe.serial` makes every test in it serial, so they run one at a time in order.

Use serial tests for steps that depend on each other's server state. Prefer independent tests that each seed what they need.

### Timeouts

A test's timeout covers its `beforeEach` hooks, the test, its `afterEach` hooks and its deferred cleanups. A test that times out fails with `TimeoutError: test timed out after N ms`, and its pages are closed. Hooks without a timeout of their own use the default. Step timeouts inside `page.run` are separate (5 s per step by default); see [Steps and checks](/e2e/steps#timeouts).

## Failures and exit code

Each test prints a line as it finishes, with failures in full. The summary lists every failure, then counts of passed, failed, skipped, todo, filtered out and not run tests.

The exit code is 1 when any test failed, a hook failed, a file failed to load, or queued tests never ran (after `--bail` or an interruption); 0 otherwise. An unhandled promise rejection or an uncaught exception during the run counts as a hook failure instead of ending the process. A spec that fails to load is reported and the other files still run.

`--bail [n]` stops starting tests after `n` failures. Running tests finish, open describes run their `afterAll` hooks and deferred cleanups, then the browser closes and teardown runs.

A first `SIGINT` (or `SIGTERM`, `SIGHUP`) stops the run the same way and prints `stopping (the browser closes, teardown runs)`. A second one exits at once.

## Sharding and timings

```sh
# record durations once (on a full run)
bunx pyro e2e e2e --setup e2e/setup.ts --timings e2e/timings.json --update-timings

# split across 4 machines
bunx pyro e2e e2e --setup e2e/setup.ts --timings e2e/timings.json --shard 1/4
```

`--shard i/n` sorts the files by their recorded duration, longest first, and gives each to the least loaded shard. Files without a recorded duration count as the average. Every shard computes the same split from the same file, so commit `timings.json`. A shard with no files prints `No spec files in shard i/n.` and exits 0.

`--update-timings` writes each file's busy time (its tests and hooks) merged into the existing file, sorted longest first. It does not write when `-t` or `--only` filtered tests, or when the run stopped early. With `--timings` and no `--shard`, files still start longest first.

## JUnit

`--junit junit.xml` writes one `testsuite` per file and one `testcase` per test, with skipped and todo tests marked `<skipped/>`. Hook failures, load failures and teardown failures appear as test cases of their own, so CI shows them.

## Screenshots

`-u` sets update mode for `matchScreenshot` steps (the same as `PYRO_UPDATE_SNAPSHOTS=1`): missing and mismatching baselines are written instead of compared. See [Visual tests](/e2e/screenshots).

## Environment

The runner sets `NODE_ENV=test` unless it is already set. `$PYRO_BIN` selects the browser binary when the setup module does not.

## Migrating from bun test --parallel

1. Replace `bun:test` imports in spec files with `@povio/pyro/e2e`. The functions take the same arguments.
2. Replace your own `Pyro.launch()` or `connect()` with `await browser()`. Do not call `pyro.close()`: the runner owns the browser.
3. Move global setup (servers, seeding, environment) into a setup module: `setup()` returns what specs need, `browser(result)` returns the server flags (`--static`, `--local-storage`, ...), `teardown(result)` stops it all.
4. Register shared profiles once (a memoized promise in a shared module), per-file or per-describe profiles in that describe's `beforeAll` with `defer(() => pyro.releaseProfile(name))`.
5. Remove per-file globals that assumed a process per file: every file shares one module graph.
6. Run with the same concurrency first (`-j 8 --max-concurrency 1` for `bun test --parallel=8 --max-concurrency=1`), then raise `-j`.

Porting ToS (2,158 cases) took the import changes and about 20 lines of harness code (PR #289). At equal concurrency on 62 files, `bun test` took 59.0 s and 59.3 s, `pyro e2e` 51.9 s and 58.9 s; on the full suite, `pyro e2e -j 16` with one browser took 313 s against 380 s for `bun test --parallel=8` with eight.

The runner has no watch mode, no coverage option and no HTML report.
