# One server, many workers

A fork-mode server keeps profiles by name for all of its clients. One launcher process can start the server and boot the shared profiles, and every `bun test --parallel` worker connects to it and uses them: profiles boot once per server, not once per worker. This page shows the pattern and the exact rules for names, holds and lifetimes.

If you can use [`pyro e2e`](/e2e/runner) instead, do: it runs every file in one process with one browser and needs none of this. Use a shared server when you must keep `bun test` as the runner, or when several independent processes drive one browser.

## The pattern

The app is the one the [suite runner](/e2e/runner#a-spec-file) example uses (a todo app with a sign-in screen); `../server` exports its `startServer()`.

```ts
// shared/run.ts: starts the app and one fork-mode browser, boots the shared
// profile once, then runs `bun test --parallel` against it.
import { Pyro, button, check, label, text } from '@povio/pyro';
import { startServer } from '../server';

const server = startServer();
const pyro = await Pyro.launch({ args: ['--fork', '--cdp-max-connections', '64'] });
try {
  await pyro.profile('signed-out', { url: `${server.url.origin}/` });
  await pyro.profile('app', {
    base: 'signed-out',
    url: `${server.url.origin}/`,
    setup: [label('Email').fill('ada@example.com'), button('Sign in').click(), check(text('Signed in')).visible()],
  });
  await pyro.warm('app');
  const tests = Bun.spawn(['bun', 'test', '--parallel', 'shared/'], {
    stdio: ['inherit', 'inherit', 'inherit'],
    env: { ...process.env, PYRO_WS: pyro.wsEndpoint, APP_URL: server.url.origin },
  });
  process.exitCode = await tests.exited;
} finally {
  pyro.close();
  server.stop(true);
}
```

```ts
// shared/a.test.ts: each worker connects to the launcher's server.
import { afterAll, beforeAll, test } from 'bun:test';
import { Pyro, check, label } from '@povio/pyro';

let pyro: Pyro;
beforeAll(async () => {
  pyro = await Pyro.connect(process.env.PYRO_WS!);
  // A per-file profile on top of the launcher's 'app': unique name per worker.
  await pyro.profile(`draft-${process.pid}-a`, { base: 'app', url: `${process.env.APP_URL}/`, setup: [label('New todo').fill('a')] });
});
afterAll(() => pyro?.close());

test.concurrent('a: uses the shared signed-in profile', async () => {
  await using page = await pyro.page({ as: 'app' });
  await page.run([check(label('New todo')).visible()]);
});
test.concurrent('a: uses its own derived profile', async () => {
  await using page = await pyro.page({ as: `draft-${process.pid}-a` });
  await page.run([check(label('New todo')).value('a')]);
});
```

`shared/b.test.ts` is the same file with `b` in place of `a`.

```sh
$ bun shared/run.ts
pyro: CDP server listening on ws://127.0.0.1:51783/ (fork mode)
bun test v1.4.3 (c6da4a4d3) 10x PARALLEL

 4 pass
 0 fail
Ran 4 tests across 2 files. [169.00ms]
```

`pyro.wsEndpoint` is the server's CDP WebSocket URL. Hand it to the workers however you like; `PYRO_WS` is the variable the Playwright layer also reads.

## How sharing works

The SDK asks the server for `Pyro.capabilities` on connect. A fork-mode server answers `{ fork: true, sharedProfiles: true }`, and the client then registers profiles **by name** on the server.

| Call | On a shared server |
|---|---|
| `pyro.profile(name, def)` | No profile has the name: it is registered and boots. Another client registered **the same definition** under it: this client adopts it and nothing boots again. **A different definition** while another client holds the name: refused |
| `pyro.adopt(name)` | Holds the profile another client registered, without redeclaring it. Throws `no profile <name> on this server: declare it with pyro.profile()` when there is none |
| `pyro.page({ as: name })` | Uses this client's profile, or adopts the server's profile of that name by itself |
| `base: name` in `pyro.profile()` | Adopts the base by itself, so a worker can derive from a profile only the launcher declared |
| `pyro.releaseProfile(name)` | Drops this client's hold |

"The same definition" compares `url`, the init scripts (including those made from `localStorage`), `viewport`, `setup` and `base`. `pool` is ignored: the template keeps one page per profile.

### Name conflicts

```text
Pyro.registerPreparedPage: profile app is registered with a different definition (release it first)
```

A name means one definition at a time. Give per-worker profiles unique names, for example with the worker's pid and the file name, as in the example. Shared names (`app`) should be declared by one process, usually the launcher, or with identical definitions everywhere.

When nobody holds a name any more, the next registration under it with a new definition takes it over. A client that redeclares its own name with a new definition releases its hold first, so that works whenever no other client holds the name.

## Holds and lifetimes

- A client holds every profile it registered or adopted.
- A hold ends with `releaseProfile(name)`, or when the client's connection closes, including when its process exits without cleaning up.
- The server retires a profile once no client holds it **and** no other profile derives from it. A base stays while a derived profile that someone holds needs it.
- Retiring a derived profile ends its template process and its spares. Pages already open keep running until they close.

`Pyro.preparedProfiles` lists the names with their holder counts, which is handy when debugging:

```ts
const { profiles } = await pyro.cdp.send('Pyro.preparedProfiles');
// e.g. [{ name: 'app', holders: 4, ... }, { name: 'draft-123-a', holders: 1, ... }]
```

## Server lifetime

`Pyro.launch()` passes its own pid to the server (`--parent-pid`, through `PYRO_PARENT_PID`). The server exits when that process exits, however it ends: on macOS it waits on the process with kqueue, on Linux with a pidfd. Test runners that stop workers without running their exit handlers can therefore not leave browsers behind.

`pyro.close()` in the launcher stops the server sooner, with its templates and forked pages: they are one process group.

A server you start yourself (`pyro serve --fork`) runs until you stop it, unless you pass `--parent-pid <PID>`.

## Connection limits

Every page in fork mode is one WebSocket connection, and each client holds one more for browser-level commands. The server's `--cdp-max-connections` (default 16) caps how many are open at once. With several workers each running concurrent tests, raise it:

```ts
Pyro.launch({ args: ['--fork', '--cdp-max-connections', '64'] });
```

Size it to workers × concurrent tests per worker, plus one per worker. `pyro e2e` sets at least 64 by itself.

## Without fork mode

A server without `--fork` reports no shared profiles. Each client's `pyro.profile()` then registers its own pools on its own connections, `pyro.adopt()` throws `adopting a profile needs pyro serve --fork`, and `pyro.page({ as })` for a name this client did not declare throws `no profile <name>: declare it with pyro.profile()`. Several workers can still connect to one plain server, but each boots its own pages.
