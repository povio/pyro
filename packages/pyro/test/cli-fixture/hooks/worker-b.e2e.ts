import { test } from './fixtures';

let seen: unknown;
test('another file uses it and the browser', async ({ shared, browser, page }) => {
  seen = browser;
  await page.setContent('<p>hi</p>');
  console.log(`B sees worker fixture ${shared.id}`);
});
test.afterAll(async ({ browser, shared }) => {
  console.log(`AFTERALL same browser ${browser === seen}, stats ${JSON.stringify((browser as any)._stats)}, worker fixture ${shared.id}`);
});
