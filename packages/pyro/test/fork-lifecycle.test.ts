import { expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate, check, css } from '../src/index';

test('closing a launched browser terminates an unresponsive fork as well as its server', async () => {
  const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/release/pyro');
  const browser = await Pyro.launch({ bin, args: ['--fork'] });
  const pid = (browser as unknown as { child: { pid: number } }).child.pid;
  const alive = () => { try { process.kill(-pid, 0); return true; } catch { return false; } };
  try {
    await browser.profile('busy', { url: 'data:text/html,<div id="ready">ready</div>', setup: [check(css('#ready')).visible()] });
    const page = await browser.page({ as: 'busy' });
    await page.run([evaluate('1')]);
    const busy = page.run([evaluate('while (true) {}')]).catch(error => error);
    await Bun.sleep(25);
    browser.close();
    expect(await busy).toBeInstanceOf(Error);
    const deadline = Date.now() + 2000;
    while (alive() && Date.now() < deadline) await Bun.sleep(10);
    expect(alive()).toBe(false);
    browser.close(); // cleanup remains idempotent
  } finally {
    browser.close();
    if (alive()) process.kill(-pid, 'SIGKILL');
  }
}, 10000);
