import { statSync } from 'node:fs';
import { expect, test } from '../../../src/wright/index';

test.afterEach(async ({ page }, info) => {
  const shot = info.outputPath('after-each.png');
  await page.screenshot({ path: shot });
  const heading = await page.evaluate(() => document.querySelector('h1')?.textContent);
  const attached = info.attachments.map((a) => `${a.name}:${a.path?.split('/').pop()}`).join(',');
  console.log(`AFTEREACH ${info.title}: ${info.status}, heading ${heading}, screenshot ${statSync(shot).size > 0}, attached [${attached}], errors ${info.errors.length}`);
  // After the `screenshot` option's captures: they show the page as the test left it.
  await page.evaluate(() => (document.querySelector('h1')!.textContent = 'changed by afterEach'));
});

test('passes', async ({ page }) => {
  await page.setContent('<h1>Passing</h1>');
});

test('fails', async ({ page }) => {
  await page.setContent('<h1>Failing</h1>');
  expect(1).toBe(2);
});

test('times out while a click waits', async ({ page }) => {
  test.setTimeout(1_000);
  await page.setContent('<h1>Waiting</h1><button>Present</button>');
  await page.getByRole('button', { name: 'Missing' }).click();
});
