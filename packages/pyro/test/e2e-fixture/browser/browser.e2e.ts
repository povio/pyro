import { beforeAll, browser, defer, describe, evaluate, expect, test } from './sdk';
import { log } from '../log';
const url = 'data:text/html,<div id="ready">ready</div><script>window.bootId = String(Math.random())</script>';
for (const name of ['one', 'two']) describe(name, () => {
  beforeAll(async () => {
    const lp = await browser();
    await lp.profile(name, { url, setup: [evaluate(`window.profile = ${JSON.stringify(name)}`)] });
    defer(() => lp.releaseProfile(name));
  });
  for (let i = 0; i < 3; i++) test(`page ${i}`, async () => {
    await using page = await (await browser()).page({ as: name });
    const r = await page.run([evaluate('window.profile + ":" + window.bootId', 'v')]);
    log('page', { value: r.values.v, name });
    expect(String(r.values.v).startsWith(name + ':')).toBe(true);
  });
});
test('a timed-out test gives its page back', async () => {
  const page = await (await browser()).page();
  log('holding a page', { pid: process.pid });
  await page.run([evaluate('new Promise(() => {})')], { timeout: 60_000 });
}, 500);
