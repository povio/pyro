// A launched browser ends with the process that launched it, and a fork-mode
// server leaves no socket files behind, however it ends.
import { expect, test } from 'bun:test';
import { readdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');
const url = 'data:text/html,<div id="ready">ready</div>';

const sockets = (pid: number) => readdirSync(tmpdir()).filter((name) => name.startsWith(`pyro-${pid}.sock`));

/** Live processes of the server's process group. */
async function group(pid: number) {
  const ps = Bun.spawn(['ps', '-axo', 'pid=,pgid=,stat='], { stdout: 'pipe' });
  const rows = (await new Response(ps.stdout).text()).trim().split('\n').map((line) => line.trim().split(/\s+/));
  return rows.filter(([, pgid, state]) => Number(pgid) === pid && !state.includes('Z'));
}

async function gone(pid: number, ms = 5000) {
  const deadline = performance.now() + ms;
  while ((await group(pid)).length) {
    if (performance.now() > deadline) throw new Error(`server ${pid} still runs: ${JSON.stringify(await group(pid))}`);
    await Bun.sleep(20);
  }
}

test('a launched browser exits when the process that launched it is killed', async () => {
  const script = join(tmpdir(), `pyro-launcher-${process.pid}.ts`);
  writeFileSync(script, `
    import { Pyro, evaluate } from ${JSON.stringify(join(import.meta.dir, '../src/index'))};
    const lp = await Pyro.launch({ bin: ${JSON.stringify(bin)}, args: ['--fork'] });
    await lp.profile('app', { url: ${JSON.stringify(url)} });
    await lp.profile('feature', { base: 'app', url: ${JSON.stringify(url)}, setup: [evaluate('1')] });
    (await lp.page({ as: 'feature' })).close();
    console.log(JSON.stringify({ pid: lp.child.pid }));
    setInterval(() => {}, 1000); // never closes it
  `);
  const launcher = Bun.spawn(['bun', script], { stdout: 'pipe', stderr: 'inherit' });
  try {
    const reader = launcher.stdout.getReader();
    let line = '';
    while (!line.includes('\n')) {
      const { value, done } = await reader.read();
      if (done) throw new Error('the launcher exited');
      line += new TextDecoder().decode(value);
    }
    const { pid } = JSON.parse(line);
    expect(sockets(pid).length).toBeGreaterThanOrEqual(4); // root and derived template, with their connection sockets
    launcher.kill('SIGKILL'); // no exit handler runs
    await gone(pid);
    expect(sockets(pid)).toEqual([]);
  } finally {
    launcher.kill('SIGKILL');
    rmSync(script, { force: true });
  }
}, 30000);

test('closing a launched fork-mode browser leaves no socket files', async () => {
  const lp = await Pyro.launch({ bin, args: ['--fork'] });
  const pid = (lp as unknown as { child: { pid: number } }).child.pid;
  await lp.profile('app', { url });
  await lp.profile('feature', { base: 'app', url, setup: [evaluate('1')] });
  await using page = await lp.page({ as: 'feature' });
  await page.run([evaluate('1')]);
  expect(sockets(pid).length).toBeGreaterThanOrEqual(4);
  lp.close();
  await gone(pid);
  expect(sockets(pid)).toEqual([]);
}, 30000);
