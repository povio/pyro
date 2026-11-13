import { test } from '../../../src/wright/index';

test.afterEach(async ({ page }, info) => {
  const started = Date.now();
  const outcome = await page.screenshot().then(() => 'taken', (e: Error) => e.message);
  console.log(`AFTEREACH ${info.title}: ${info.status}, page closed ${page.isClosed()}, screenshot: ${outcome} (${Date.now() - started < 1_000 ? 'at once' : 'slow'})`);
});

// A page script that never returns, outside any command: the browser cannot
// take the cancel, and the runner closes the page instead.
test('times out on a stuck browser', async ({ page }) => {
  test.setTimeout(1_000);
  await page.setContent('<h1>Stuck</h1>');
  await page.evaluate(() => void setTimeout(() => { while (true) {} }, 50));
  await page.getByRole('button', { name: 'Missing' }).click();
});
