import type { Page } from '@povio/pyro';
import { afterEach, browser, evaluate, test } from '../browser/sdk';
import { log } from '../log';

let page: Page | undefined;
afterEach(async () => {
  if (!page) return;
  const r = await page.run([evaluate('document.title', 'title')]);
  log('after', { title: r.values.title });
});

// The run the test waits on spins in the page: the timeout cancels it, so
// the afterEach above can still use the page.
test('a timed-out test frees its page for afterEach', async () => {
  page = await (await browser()).page();
  await page.run([evaluate('document.title = "Kept"')]);
  await page.run([evaluate('while (true) {}')], { timeout: 60_000 });
}, 1_000);
