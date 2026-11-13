// One `serve --fork` shared by several clients: profiles are kept by name on
// the server, booted once, and held by the clients that use them.
import { expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate, check, css } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');
// Every boot of the page draws a new id: pages forked from one boot share it.
const url = 'data:text/html,<div id="ready">ready</div><script>window.bootId = Math.random()</script>';
const app = { url, setup: [check(css('#ready')).visible()] };

const read = async (lp: Pyro, as: string, expression: string) => {
  await using page = await lp.page({ as });
  return (await page.run([evaluate(expression, 'value')])).values.value;
};
const listed = async (lp: Pyro) =>
  (await lp.cdp.send('Pyro.preparedProfiles')).profiles.map((p: { name: string; holders: number }) => `${p.name}:${p.holders}`);

async function until(check: () => Promise<boolean>, what: string, ms = 3000) {
  const deadline = performance.now() + ms;
  while (!(await check())) {
    if (performance.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await Bun.sleep(20);
  }
}

test('clients of one server share a profile booted once, and its derived profiles', async () => {
  const launcher = await Pyro.launch({ bin, args: ['--fork'] });
  const workers: Pyro[] = [];
  try {
    await launcher.profile('app', app);
    const boot = await read(launcher, 'app', 'window.bootId');
    const [one, two, three] = await Promise.all([0, 1, 2].map(() => Pyro.connect(launcher.wsEndpoint)));
    workers.push(one, two, three);
    // The same definition under the same name adopts the booted profile.
    await one.profile('app', app);
    expect(await read(one, 'app', 'window.bootId')).toBe(boot);
    // A name another client registered: adopted explicitly, or by asking for a page.
    await two.adopt('app');
    expect(await read(two, 'app', 'window.bootId')).toBe(boot);
    expect(await read(three, 'app', 'window.bootId')).toBe(boot);
    expect(await listed(launcher)).toEqual(['app:4']);
    // A derived profile on a base this client never declared, used by another client.
    await two.profile('feature', { base: 'app', url, setup: [evaluate('window.marker = "feature"')] });
    expect(await read(one, 'feature', 'window.marker + ":" + (window.bootId === ' + boot + ')')).toBe('feature:true');
    // A different definition is refused while others hold the name.
    await expect(one.profile('app', { url: `${url}<b>other</b>` })).rejects.toThrow('different definition');
    await expect(three.page({ as: 'missing' })).rejects.toThrow('no profile missing');
  } finally {
    for (const worker of workers) worker.close();
    launcher.close();
  }
}, 30000);

test('a profile stays until its last holder releases it or disconnects', async () => {
  const launcher = await Pyro.launch({ bin, args: ['--fork'] });
  const pid = (launcher as unknown as { child: { pid: number } }).child.pid;
  const worker = await Pyro.connect(launcher.wsEndpoint);
  let closed = false;
  try {
    await launcher.profile('app', app);
    // The worker adopts its base, too.
    await worker.profile('feature', { base: 'app', url, setup: [evaluate('window.marker = "feature"')] });
    await launcher.adopt('feature');
    expect(await listed(launcher)).toEqual(['app:2', 'feature:2']);
    // One holder released: the others keep it.
    await launcher.releaseProfile('feature');
    expect(await read(worker, 'feature', 'window.marker')).toBe('feature');
    expect(await listed(launcher)).toEqual(['app:2', 'feature:1']);
    await launcher.releaseProfile('app');
    await worker.releaseProfile('app');
    // Nobody holds the base, but a held profile derives from it.
    expect(await listed(launcher)).toEqual(['app:0', 'feature:1']);
    expect(await read(worker, 'feature', 'window.marker')).toBe('feature');
    // Its last holder goes away without releasing: the server retires it,
    // and then the base it kept.
    worker.close();
    closed = true;
    await until(async () => (await listed(launcher)).length === 0, 'the worker holds to end');
    await expect(launcher.page({ as: 'feature' })).rejects.toThrow('no profile feature');
    // Its derived template and spares are gone too: server, root template and its spares remain.
    await until(async () => (await family(pid)).length <= 5, 'the derived template to exit');
  } finally {
    if (!closed) worker.close();
    launcher.close();
  }
}, 30000);

test('a profile derived from one that was just registered does not wait for its template', async () => {
  const lp = await Pyro.launch({ bin, args: ['--fork'] });
  try {
    // The base takes a while to get ready, so its derived templates are not up yet.
    const slow = 'data:text/html,<div id="ready">ready</div><script>setTimeout(() => document.body.insertAdjacentHTML("beforeend", "<i id=late>late</i>"), 300)</script>';
    await lp.profile('app', { url: slow, setup: [check(css('#late')).visible()] });
    await lp.profile('a', { base: 'app', url: slow, setup: [evaluate('window.path = "a"')] });
    await lp.profile('b', { base: 'a', url: slow, setup: [evaluate('window.path += "b"')] });
    await lp.profile('c', { base: 'b', url: slow, setup: [evaluate('window.path += "c"')] });
    // Released before its template exists.
    await lp.profile('gone', { base: 'b', url: slow, setup: [evaluate('window.path += "x"')] });
    await lp.releaseProfile('gone');
    expect(await read(lp, 'c', 'window.path')).toBe('abc');
    expect(await read(lp, 'b', 'window.path')).toBe('ab');
    expect(await listed(lp)).toEqual(['a:1', 'app:1', 'b:1', 'c:1']);
  } finally {
    lp.close();
  }
}, 30000);

/** The server's process group: it, its templates, spares and forked pages. */
async function family(pid: number) {
  const ps = Bun.spawn(['ps', '-axo', 'pid=,pgid=,stat='], { stdout: 'pipe' });
  const rows = (await new Response(ps.stdout).text()).trim().split('\n').map((line) => line.trim().split(/\s+/));
  return rows.filter(([, group, state]) => Number(group) === pid && !state.includes('Z'));
}
