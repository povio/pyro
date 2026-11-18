import { expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';
import { Cdp } from '../src/cdp';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

for (const spares of ['0', '2']) test(`spares=${spares}: a profile-specific fork keeps only its rendered document, including derived snapshots`, async () => {
  const previous = process.env.PYRO_FORK_SPARES;
  process.env.PYRO_FORK_SPARES = spares;
  const browser = await Pyro.launch({ bin, args: ['--fork'] });
  if (previous === undefined) delete process.env.PYRO_FORK_SPARES;
  else process.env.PYRO_FORK_SPARES = previous;
  try {
    for (let i = 0; i < 19; i++) {
      await browser.profile(`p${i}`, {
        url: `data:text/html,<p id="state">profile ${i}</p>`,
        setup: [evaluate(`window.boot = Math.random(); window.ticks = 0; setInterval(() => window.ticks++, 10); document.querySelector('#state').textContent = 'rendered ${i}'`)],
      });
    }
    await browser.profile('derived', { base: 'p0', url: 'data:text/html,<p id="state">profile 0</p>', setup: [evaluate("document.querySelector('#state').textContent = 'derived'")] });
    for (const name of ['p0', 'p18', 'derived']) {
      await using first = await browser.page({ as: name });
      await using second = await browser.page({ as: name });
      // This queries the fork itself, not the root registry. Hidden sibling
      // documents must not keep their timers, requests or DOMs in this process.
      const profiles = (await browser.cdp.send('Pyro.preparedProfiles')).profiles;
      const endpoint = new URL(browser.wsEndpoint);
      endpoint.searchParams.set('profile', String(profiles.find((p: {name: string}) => p.name === name).profileId));
      const connection = await Cdp.open(endpoint.toString());
      try {
        const waiting = (await connection.send('Pyro.preparedPages')).pages;
        expect(waiting).toHaveLength(1);
      } finally { connection.close(); }
      const expression = "({boot: window.boot, text: document.querySelector('#state').textContent, ticks: window.ticks})";
      const a = (await first.run([evaluate(expression, 'state')])).values.state as { boot: number; text: string; ticks: number };
      const b = (await second.run([evaluate(expression, 'state')])).values.state as { boot: number; text: string; ticks: number };
      expect(a.boot).toBe(b.boot); // the rendered snapshot was inherited, not booted again
      expect(a.text).toBe(name === 'derived' ? 'derived' : `rendered ${name.slice(1)}`);
      await first.run([evaluate('new Promise(resolve => setTimeout(resolve, 35))')]);
      const later = (await first.run([evaluate('window.ticks', 'ticks')])).values.ticks as number;
      expect(later).toBeGreaterThan(a.ticks); // the selected document remains live
    }
  } finally { browser.close(); }
}, 20000);
