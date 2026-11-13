// `pyro run` on a TOS-shaped config: discovery of `.e2e.ts` files per
// project, each file with its project's `use`, parallel tests spread over worker
// processes (own module state), skips reported as skips, the exit code. Then
// Playwright's project dependencies, global setup's FullConfig, sharding,
// hooks, worker fixtures and testInfo, each on its own config under cli-fixture/.
import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const dir = join(import.meta.dir, 'cli-fixture');
const cli = join(import.meta.dir, '../src/cli.ts');
// Not spawnSync: the tests' own servers answer while the run goes on.
const runIn = async (fixture: string, env: Record<string, string>, ...args: string[]) => {
  const cwd = join(dir, fixture);
  const p = Bun.spawn(['bun', cli, 'run', '-c', join(cwd, 'playwright.config.ts'), ...args], { cwd, env: { ...process.env, NO_COLOR: '1', ...env }, stdout: 'pipe', stderr: 'pipe' });
  const [out, err, code] = await Promise.all([new Response(p.stdout).text(), new Response(p.stderr).text(), p.exited]);
  return { code, out: out + err };
};
const run = (...args: string[]) => runIn('', {}, ...args);
/** The tests a run passed, as `[project] › file › title`. */
const passedIn = (out: string) => [...out.matchAll(/✓ +\d+ (.+?) \(\d+ms\)/g)].map((m) => m[1]);

test('runs every project and reports passes, failures and skips', async () => {
  const { code, out } = await run();
  expect(out).toContain('Running 3 files using 2 workers');
  expect(out).toContain('✓');
  expect(out).toMatch(/✘ +\d+ e2e\/root\.e2e\.ts › fails/);
  expect(out).toMatch(/- +\d+ e2e\/root\.e2e\.ts › skips at run time \(not today\)/);
  expect(out).toMatch(/- +\d+ e2e\/root\.e2e\.ts › skipped when declared/);
  expect(out).toContain('uses its own project');
  expect(out).toContain('1 failed');
  expect(out).toContain('2 skipped');
  expect(out).toContain('6 passed');
  expect(out).toContain('expect(received).toBe(expected)');
  expect(code).toBe(1);
}, 60_000);

test('spreads parallel tests over the workers, each with its own module state', async () => {
  const { out } = await run('parallel', '-j', '2');
  expect(out).toContain('4 passed');
  expect(out).toMatch(/\[w0\] ran p\d in worker 0/);
  expect(out).toMatch(/\[w1\] ran p\d in worker 1/);
}, 60_000);

test('filters by file and grep, lists without running', async () => {
  expect((await run('sub')).out).toContain('1 passed');
  const grep = await run('-g', 'uses its');
  expect(grep.out).toContain('2 passed');
  expect(grep.code).toBe(0);
  const list = await run('--list');
  expect(list.out).toContain('Total: 9 tests in 3 files');
}, 60_000);

describe('project dependencies', () => {
  // The sign-in server: /login sets the cookie the storage state keeps, /whoami echoes the cookie.
  let server: ReturnType<typeof Bun.serve>;
  let state: string;
  let pings = 0;
  beforeAll(() => {
    server = Bun.serve({
      port: 0,
      fetch(req) {
        const { pathname } = new URL(req.url);
        if (pathname === '/ping') pings++;
        if (pathname === '/login') return new Response('ok', { headers: { 'set-cookie': 'session=signed-in; Path=/' } });
        if (pathname === '/whoami') return new Response(req.headers.get('cookie') ?? '', { headers: { 'content-type': 'text/plain' } });
        return new Response('pong');
      },
    });
    state = join(mkdtempSync(join(tmpdir(), 'pyro-deps-')), '.auth', 'user.json');
  });
  afterAll(() => {
    server.stop(true);
    rmSync(join(state, '../..'), { recursive: true, force: true });
  });
  const deps = (env: Record<string, string>, ...args: string[]) => runIn('deps', { DEPS_BASE_URL: `http://127.0.0.1:${server.port}`, DEPS_STATE: state, ...env }, ...args);

  test('the setup project runs first, its dependents start from its storage state, the teardown project runs last', async () => {
    const { code, out } = await deps({});
    expect(code).toBe(0);
    expect(passedIn(out)).toEqual([
      '[setup] › auth.setup.ts › signs in',
      expect.stringMatching(/^\[app\] › app\.e2e\.ts › /),
      expect.stringMatching(/^\[app\] › app\.e2e\.ts › /),
      '[cleanup] › cleanup.teardown.ts › removes the storage state',
    ]);
    expect(out).toContain('TEARDOWN finds the state present');
    expect(out).toContain('4 passed');
  }, 60_000);

  test('global setup gets the FullConfig: projects with their resolved use', async () => {
    pings = 0;
    const { out } = await deps({}, '--list');
    // --list runs no global setup.
    expect(pings).toBe(0);
    expect(out).toContain('Total: 4 tests in 3 files');
    expect((await deps({})).out).toContain(`GLOBAL SETUP ping 200, projects setup,cleanup,app, app depends on setup, app storageState true, workers 2, shard null`);
    expect(pings).toBe(1);
  }, 60_000);

  test('a failing setup project: its dependents do not run, the teardown project still does', async () => {
    const { code, out } = await deps({ DEPS_FAIL_SETUP: '1' });
    expect(code).toBe(1);
    expect(out).toMatch(/✘ +\d+ \[setup\] › auth\.setup\.ts › signs in/);
    expect(out).toContain('Error: sign-in failed');
    expect(out).not.toContain('[app] ›');
    expect(passedIn(out)).toEqual(['[cleanup] › cleanup.teardown.ts › removes the storage state']);
    expect(out).toContain('TEARDOWN finds the state missing');
    expect(out).toContain('1 failed');
    expect(out).toContain('2 did not run');
  }, 60_000);

  test('--project takes the dependencies along, --no-deps leaves them out', async () => {
    const app = await deps({}, '--project', 'app', '--list');
    expect(app.out).toContain('[setup] › auth.setup.ts › signs in');
    expect(app.out).toContain('[cleanup] › cleanup.teardown.ts › removes the storage state');
    const alone = await deps({}, '--project', 'APP', '--no-deps', '--list');
    expect(alone.out).toContain('Total: 2 tests in 1 file');
    expect((await deps({}, '--project', 'nope')).out).toContain('no project named "nope"');
  }, 60_000);
});

describe('--shard', () => {
  const tests = ['a.e2e.ts › a0', 'a.e2e.ts › a1', 'a.e2e.ts › a2', 'a.e2e.ts › a3', 'a.e2e.ts › a4', 'b.e2e.ts › b0', 'b.e2e.ts › b1', 'c.e2e.ts › c0'].map((t) => `[tests] › ${t}`);
  const shards = (...args: string[]) =>
    Promise.all(
      [1, 2].map(async (i) => {
        const { code, out } = await runIn('shard', {}, '--shard', `${i}/2`, ...args);
        expect(code).toBe(0);
        expect(out).toContain(`shard ${i} of 2`);
        return passedIn(out);
      }),
    );
  const fileOf = (t: string) => t.split(' › ')[1];

  test('the shards run every test once, a file in one shard; the dependency runs in each', async () => {
    const [one, two] = await shards();
    for (const s of [one, two]) expect(s.filter((t) => t.startsWith('[setup]'))).toEqual(['[setup] › prepare.setup.ts › prepares']);
    const ran = [...one, ...two].filter((t) => t.startsWith('[tests]'));
    expect(ran.sort()).toEqual(tests);
    // Balanced by tests, as Playwright: a (5 tests) | b, c (3).
    expect([...new Set(one.filter((t) => t.startsWith('[tests]')).map(fileOf))]).toEqual(['a.e2e.ts']);
    expect(new Set(two.filter((t) => t.startsWith('[tests]')).map(fileOf))).toEqual(new Set(['b.e2e.ts', 'c.e2e.ts']));
  }, 60_000);

  test('fully parallel: single tests split, a file can span shards', async () => {
    const [one, two] = await shards('--fully-parallel');
    const ran = [...one, ...two].filter((t) => t.startsWith('[tests]'));
    expect(ran.sort()).toEqual(tests);
    // 8 tests: a0-a3 | a4, b0, b1, c0.
    expect(one.filter((t) => t.startsWith('[tests]')).sort()).toEqual(tests.slice(0, 4));
    expect(two).toContain('[setup] › prepare.setup.ts › prepares');
  }, 60_000);

  test('the config can set the shard; --list lists the shard', async () => {
    const { out } = await runIn('shard', { SHARD_IN_CONFIG: '1' });
    expect(out).toContain('shard 2 of 2');
    expect(passedIn(out).filter((t) => t.startsWith('[tests]')).sort()).toEqual(tests.slice(5));
    const list = (await runIn('shard', {}, '--shard', '3/3', '--fully-parallel', '--list')).out;
    expect(list).toContain('[tests] › b.e2e.ts › b1');
    expect(list).not.toContain('b0');
    expect(list).toContain('Total: 3 tests in 3 files');
    const bad = await runIn('shard', {}, '--shard', '3/2');
    expect(bad.code).toBe(2);
    expect(bad.out).toContain('expected "current/total"');
  }, 60_000);
});

describe('hooks, worker fixtures and testInfo', () => {
  let out = '';
  let code: number | null = null;
  beforeAll(async () => {
    ({ out, code } = await runIn('hooks', {}));
  }, 60_000);
  const lines = () => out.split('\n').map((l) => l.replace(/^ +\[w\d+\] /, '').trim());

  test('a worker fixture is set up once per worker, across files, and torn down after the files', async () => {
    expect(lines().filter((l) => l === 'WORKER FIXTURE SETUP')).toHaveLength(1);
    expect(out).toContain('A sees worker fixture 1');
    expect(out).toContain('B sees worker fixture 1');
    expect(lines().filter((l) => l === 'WORKER FIXTURE TEARDOWN')).toHaveLength(1);
    expect(out.indexOf('WORKER FIXTURE TEARDOWN')).toBeGreaterThan(out.indexOf('AFTERALL same browser'));
  }, 60_000);

  test("a file's afterAll sees the browser its tests used", async () => {
    expect(out).toContain('AFTERALL same browser true, stats {"pooled":0,"fresh":1}, worker fixture 1');
  }, 60_000);

  test('a beforeAll error fails the first test of its describe; the others do not run; the run fails', async () => {
    expect(out).toMatch(/✘ +\d+ before-all\.e2e\.ts › a beforeAll that asks for a page › first/);
    expect(out).toContain('"context" and "page" fixtures are not supported in "beforeAll"');
    expect(out).toMatch(/- +\d+ before-all\.e2e\.ts › a beforeAll that asks for a page › second \(a beforeAll hook of its describe failed\)/);
    expect(out).toContain('AFTERALL runs after the failed beforeAll');
    // Test fixtures other than the page are a beforeAll's own, as in Playwright.
    expect(out).toMatch(/✓ +\d+ before-all\.e2e\.ts › a beforeAll with a test fixture › gets its own instance/);
    expect(out).toMatch(/✓ +\d+ before-all\.e2e\.ts › a test outside the describes runs/);
    expect(out).toContain('4 failed');
    expect(out).toContain('1 did not run');
    expect(out).toContain('6 passed');
    expect(code).toBe(1);
  }, 60_000);

  test('an afterAll error fails the last test of its describe; a worker fixture teardown error fails the run', () => {
    expect(out).toMatch(/✘ +\d+ errors\.e2e\.ts › an afterAll that throws › passes, and is the last of its describe/);
    expect(out).toContain('Error: afterAll failed');
    expect(out).toContain('Error outside any test: worker fixture "tornDownBadly" teardown');
    expect(out).toContain('Error: worker fixture teardown failed');
    expect(out).toContain('1 error was not part of any test');
  }, 60_000);

  test('afterEach hooks and fixture teardown see the status and errors of the test', async () => {
    expect(out).toContain('AFTEREACH fails: failed, expected passed, 1 error(s), expect(received).toBe(expected)');
    expect(out).toContain('FIXTURE TEARDOWN fails: failed');
    expect(out).toContain('AFTEREACH times out: timedOut, expected passed, 1 error(s), Test timeout of 200ms exceeded.');
    expect(out).toContain('FIXTURE TEARDOWN times out: timedOut');
    expect(out).toContain('AFTEREACH fails as expected: failed, expected failed, 1 error(s), expect(received).toBe(expected)');
    expect(out).toContain('AFTEREACH passes: passed, expected passed, 0 error(s)');
    expect(out).toMatch(/✘ +\d+ status\.e2e\.ts › times out \(\d+ms\) \(timed out\)/);
  }, 60_000);
});
