// `pyro e2e`: spec files in one process, one case queue, hooks,
// serial tests, timeouts, filters, shards, reporters, setup and the browser.
import { expect, test } from 'bun:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { discover, shardFiles } from '../src/e2e/run';

const pkg = join(import.meta.dir, '..');
const cli = join(pkg, 'src/cli.ts');
const bin = process.env.PYRO_BIN ?? join(pkg, '../../target/debug/pyro');
const work = mkdtempSync(join(tmpdir(), 'pyro-e2e-'));
let runs = 0;

type Event = { event: string; t: number; active?: number; [key: string]: unknown };

function spawnE2e(args: string[], env: Record<string, string> = {}) {
  const log = join(work, `events-${runs++}.jsonl`);
  const proc = Bun.spawn(['bun', cli, 'e2e', ...args], {
    cwd: pkg,
    env: { ...process.env, E2E_LOG: log, NO_COLOR: '1', PYRO_BIN: bin, ...env },
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const events = (): Event[] => (existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);
  const done = Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]).then(([out, err, code]) => {
    const all = events();
    return { code, out: out + err, events: all, names: all.map((e) => e.event) };
  });
  return { proc, events, done };
}
const e2e = (args: string[], env?: Record<string, string>) => spawnE2e(args, env).done;
const at = (names: string[], name: string) => {
  const i = names.indexOf(name);
  if (i < 0) throw new Error(`no event ${name} in ${JSON.stringify(names)}`);
  return i;
};

test('cases of different files interleave up to the concurrency', async () => {
  const r = await e2e(['test/e2e-fixture/interleave/*.e2e.ts', '-j', '4']);
  expect(r.code).toBe(0);
  expect(Math.max(...r.events.map((e) => e.active ?? 0))).toBe(4);
  const starts = r.events.filter((e) => e.event.startsWith('start'));
  const ends = r.events.filter((e) => e.event.startsWith('end'));
  // Eight 100 ms tests four at a time: two rounds, not eight.
  expect(Math.max(...ends.map((e) => e.t)) - Math.min(...starts.map((e) => e.t))).toBeLessThan(380);
  // A file's test starts before the other file's tests are done.
  expect(at(r.names, 'start two/t0')).toBeLessThan(Math.max(...['t0', 't1', 't2', 't3'].map((t) => at(r.names, `end one/${t}`))));
  expect(r.out).toContain('8 pass');
  expect(r.out).toContain('Ran 8 tests across 2 files.');
  const one = await e2e(['test/e2e-fixture/interleave/*.e2e.ts', '-j', '1']);
  expect(Math.max(...one.events.map((e) => e.active ?? 0))).toBe(1);
  // --max-concurrency: tests of one file at a time (bun test's meaning).
  const perFile = await e2e(['test/e2e-fixture/interleave/*.e2e.ts', '-j', '8', '--max-concurrency', '1']);
  expect(Math.max(...perFile.events.map((e) => e.active ?? 0))).toBe(2);
  for (const f of ['one', 'two']) {
    const mine = perFile.events.filter((e) => e.event.includes(` ${f}/`));
    for (let i = 1; i < mine.length; i += 2) expect(mine[i].event.startsWith('end')).toBe(true); // start, end, start, end
  }
}, 30000);

test('hooks run once per describe and around every test, in order', async () => {
  const r = await e2e(['test/e2e-fixture/hooks/hooks.e2e.ts', '-j', '1']);
  expect(r.code).toBe(0);
  expect(r.names).toEqual([
    'file beforeAll', 'outer beforeAll',
    'file beforeEach', 'outer beforeEach', 'test a', 'outer afterEach', 'file afterEach',
    'inner beforeAll',
    'file beforeEach', 'outer beforeEach', 'test b', 'outer afterEach', 'file afterEach',
    'inner afterAll', 'outer afterAll', 'file afterAll',
  ]);
  // Concurrently: the same hooks, each beforeAll still once and first.
  const c = await e2e(['test/e2e-fixture/hooks/hooks.e2e.ts', '-j', '8']);
  expect(c.code).toBe(0);
  expect(c.names.filter((n) => n.endsWith('beforeAll'))).toEqual(['file beforeAll', 'outer beforeAll', 'inner beforeAll']);
  expect(at(c.names, 'outer afterAll')).toBe(c.names.length - 2);
}, 30000);

test('a serial test runs alone in its describe, between the tests declared around it', async () => {
  const r = await e2e(['test/e2e-fixture/serial/serial.e2e.ts', '-j', '8']);
  expect(r.code).toBe(0);
  const n = r.names;
  expect(at(n, 'start t2')).toBeLessThan(at(n, 'end t1')); // t1 and t2 overlap
  expect(at(n, 'start s3')).toBeGreaterThan(Math.max(at(n, 'end t1'), at(n, 'end t2')));
  expect(Math.min(at(n, 'start t4'), at(n, 'start t5'))).toBeGreaterThan(at(n, 'end s3'));
  expect(at(n, 'start t5')).toBeLessThan(at(n, 'end t4')); // t4 and t5 overlap
  expect(at(n, 'end u1')).toBeLessThan(at(n, 'start u2'));
  expect(at(n, 'end u2')).toBeLessThan(at(n, 'start u3'));
  // describe.serial does not hold up other describes.
  expect(at(n, 'start u1')).toBeLessThan(at(n, 'end t1'));
}, 30000);

test('tests waiting for a beforeAll start when it is done', async () => {
  const r = await e2e(['test/e2e-fixture/slow-setup/slow.e2e.ts', '-j', '4']);
  expect(r.code).toBe(0);
  for (const name of ['a2', 'a3']) expect(at(r.names, `end ${name}`)).toBeLessThan(at(r.names, 'end long'));
}, 30000);

test('timeouts: --timeout, or the test own', async () => {
  const r = await e2e(['test/e2e-fixture/timeout/timeout.e2e.ts', '--timeout', '150']);
  expect(r.code).toBe(1);
  expect(r.out).toMatch(/✗ .*too slow/);
  expect(r.out).toContain('test timed out after 150 ms');
  expect(r.out).toContain('2 pass');
  expect(r.out).toContain('1 fail');
}, 30000);

test('name pattern, only, skip and todo', async () => {
  const all = await e2e(['test/e2e-fixture/filter/filter.e2e.ts']);
  expect(all.code).toBe(0);
  expect(all.names.sort()).toEqual(['alpha one', 'beta two', 'focused', 'inside']);
  expect(all.out).toContain('1 skip');
  expect(all.out).toContain('1 todo');
  const named = await e2e(['test/e2e-fixture/filter/filter.e2e.ts', '-t', '^group alpha']);
  expect(named.names).toEqual(['alpha one']);
  expect(named.out).toContain('3 filtered out');
  const only = await e2e(['test/e2e-fixture/filter/filter.e2e.ts', '--only']);
  expect(only.names.sort()).toEqual(['focused', 'inside']);
  const none = await e2e(['test/e2e-fixture/filter/filter.e2e.ts', '-t', 'nothing matches']);
  expect(none.code).toBe(1);
  expect(none.out).toContain('no tests matched');
}, 30000);

test('a failing beforeAll fails its tests, and afterAll still runs', async () => {
  const junit = join(work, 'report.xml');
  const r = await e2e(['test/e2e-fixture/failing-hook/hook.e2e.ts', '--reporter', 'junit', '--reporter-outfile', junit]);
  expect(r.code).toBe(1);
  expect(r.names.sort()).toEqual(['afterAll still runs', 'unaffected']);
  expect(r.out).toContain("beforeAll of describe 'broken setup' failed: setup exploded");
  expect(r.out).toContain('2 fail');
  const xml = readFileSync(junit, 'utf8');
  expect(xml).toContain('<testsuites name="pyro e2e" tests="3" failures="2"');
  expect(xml).toContain('<testcase name="broken setup &gt; x" classname="test/e2e-fixture/failing-hook/hook.e2e.ts"');
  expect(xml).toContain('<failure message="beforeAll of describe &apos;broken setup&apos; failed: setup exploded" type="HookError">');
  expect(xml).toContain('<testcase name="unaffected"');
}, 30000);

test('defer runs after its test, or after its describe and its afterAll, latest first', async () => {
  const r = await e2e(['test/e2e-fixture/defer/defer.e2e.ts']);
  expect(r.code).toBe(0);
  expect(r.names).toEqual(['in scoped > t', 'test defer', 'describe afterAll', 'describe defer 2', 'describe defer 1']);
}, 30000);

test('a spec that does not load fails the run; the others still run', async () => {
  const r = await e2e(['test/e2e-fixture/load-error/*.e2e.ts']);
  expect(r.code).toBe(1);
  expect(r.out).toContain('broken.e2e.ts failed to load');
  expect(r.out).toContain('this spec does not load');
  expect(r.out).toContain('1 pass');
}, 30000);

test('the setup module runs first, its result is readable, and teardown runs even on SIGINT', async () => {
  const r = await e2e(['test/e2e-fixture/setup/uses.e2e.ts', '--setup', 'test/e2e-fixture/setup/setup.ts']);
  expect(r.code).toBe(0);
  expect(r.names).toEqual(['setup', 'read', 'long started', 'teardown from-setup']);
  const run = spawnE2e(['test/e2e-fixture/setup/uses.e2e.ts', '--setup', 'test/e2e-fixture/setup/setup.ts'], { E2E_LONG: '20000' });
  const deadline = Date.now() + 10_000;
  while (!run.events().some((e) => e.event === 'long started') && Date.now() < deadline) await Bun.sleep(20);
  run.proc.kill('SIGINT');
  const stopped = await run.done;
  expect(stopped.code).toBe(130);
  expect(stopped.names.at(-1)).toBe('teardown from-setup');
}, 30000);

test('a stray rejection fails the run instead of ending it', async () => {
  const r = await e2e(['test/e2e-fixture/stray/stray.e2e.ts']);
  expect(r.code).toBe(1);
  expect(r.out).toContain('nobody awaited this');
  expect(r.out).toContain('2 pass');
}, 30000);

test('timings: written per file, and shards split by them', async () => {
  const timings = join(work, 'timings.json');
  const r = await e2e(['test/e2e-fixture/interleave/*.e2e.ts', '--timings', timings, '--update-timings']);
  expect(r.code).toBe(0);
  const files = JSON.parse(readFileSync(timings, 'utf8')).files as Record<string, number>;
  expect(Object.keys(files).sort()).toEqual(['test/e2e-fixture/interleave/one.e2e.ts', 'test/e2e-fixture/interleave/two.e2e.ts']);
  for (const ms of Object.values(files)) expect(ms).toBeGreaterThanOrEqual(400);
  const shard = await e2e(['test/e2e-fixture/interleave/*.e2e.ts', '--timings', timings, '--shard', '2/2']);
  expect(shard.code).toBe(0);
  expect(shard.out).toContain('Ran 4 tests across 1 file.');

  const weights = new Map([['a', 100], ['b', 60], ['c', 50], ['d', 40], ['e', 10]]);
  const split = [1, 2].map((i) => shardFiles(['a', 'b', 'c', 'd', 'e', 'f'], weights, i, 2));
  expect(split).toEqual([['a', 'c', 'e'], ['b', 'f', 'd']]); // f counts as the average, 52
  expect(shardFiles(['a', 'b'], new Map(), 3, 3)).toEqual([]);
}, 30000);

test('discovery: spec names under directories, globs and path filters', () => {
  const dir = mkdtempSync(join(tmpdir(), 'pyro-e2e-discover-'));
  try {
    for (const f of ['a.test.ts', 'b.spec.tsx', 'c.ts', 'node_modules/x.test.ts', 'sub/d_test.js', '.hidden/e.test.ts']) {
      mkdirSync(join(dir, f, '..'), { recursive: true });
      writeFileSync(join(dir, f), '');
    }
    expect(discover(dir, [])).toEqual(['a.test.ts', 'b.spec.tsx', 'sub/d_test.js']);
    expect(discover(dir, ['sub'])).toEqual(['sub/d_test.js']);
    expect(discover(dir, ['spec'])).toEqual(['b.spec.tsx']);
    expect(discover(dir, ['c.ts'])).toEqual(['c.ts']);
    expect(discover(dir, ['*.ts'])).toEqual(['a.test.ts', 'c.ts']);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a timed-out test's runs are cancelled before its afterEach hooks, which can use its page", async () => {
  const r = await e2e(['test/e2e-fixture/browser-timeout/timeout.e2e.ts', '--timeout', '20000']);
  expect(r.code).toBe(1);
  expect(r.out).toContain('test timed out after 1000 ms');
  expect(r.events.filter((e) => e.event === 'after').map((e) => e.title)).toEqual(['Kept']);
}, 60000);

test('one fork-mode browser for the run: profiles per describe, pages closed, nothing left running', async () => {
  const r = await e2e(['test/e2e-fixture/browser/browser.e2e.ts', '-j', '4', '--timeout', '20000']);
  expect(r.code).toBe(1); // the last test times out on purpose
  expect(r.out).toContain('test timed out after 500 ms');
  expect(r.out).toContain('6 pass');
  const pages = r.events.filter((e) => e.event === 'page') as unknown as { value: string; name: string }[];
  expect(pages.length).toBe(6);
  // Pages of a profile fork from one boot.
  for (const name of ['one', 'two']) expect(new Set(pages.filter((p) => p.name === name).map((p) => p.value)).size).toBe(1);
  // The browser (its whole process group) is gone with the run.
  const ps = Bun.spawnSync(['ps', '-axo', 'pid=,ppid=,command=']).stdout.toString();
  expect(ps.split('\n').filter((l) => l.includes(bin) && l.includes(' serve '))).toEqual([]);
}, 60000);
