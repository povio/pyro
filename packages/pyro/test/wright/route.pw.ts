// page.route / context.route: Playwright's request interception.
import { expect, test } from './fixtures';

test('fulfill, abort, continue with overrides', async ({ page }) => {
  await page.route('**/api/data.json*', (route) => route.fulfill({ json: { greeting: 'Mocked' } }));
  await page.goto('/form');
  await page.getByLabel('Name').fill('Ada');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('Mocked');

  await page.unroute('**/api/data.json*');
  await page.route(/\/api\/data\.json/, (route) => route.continue({ url: route.request().url().replace('Ada', 'Bob') }));
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('Hello, Bob');

  await page.route('**/api/routed.json', (route) => route.abort());
  const failed = page.waitForEvent('requestfailed');
  await page.getByRole('button', { name: 'Load routed' }).click();
  expect((await failed).url()).toContain('/api/routed.json');
});

test('fallback chain, times, predicates and route.fetch', async ({ page }) => {
  const seen: string[] = [];
  await page.route('**/api/**', async (route) => {
    seen.push('outer');
    await route.fallback();
  });
  await page.route(
    (url) => url.pathname === '/api/data.json',
    async (route) => {
      seen.push('inner');
      const response = await route.fetch();
      const json = await response.json();
      await route.fulfill({ response, json: { greeting: json.greeting.toUpperCase() } });
    },
    { times: 1 },
  );
  await page.goto('/form');
  await page.getByLabel('Name').fill('ada');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('HELLO, ADA');
  expect(seen).toEqual(['inner']);
  // `times: 1` is used up: the next request falls through to the outer route, then the network.
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('Hello, ada');
  expect(seen).toEqual(['inner', 'outer']);
});

test('context.route applies to its pages, before navigation', async ({ context }) => {
  await context.route('**/simple', (route) =>
    route.fulfill({ status: 200, contentType: 'text/html', body: '<title>stubbed</title><h1>From route</h1>' }),
  );
  const page = await context.newPage();
  await page.goto('/simple');
  await expect(page).toHaveTitle('stubbed');
  await expect(page.getByRole('heading')).toHaveText('From route');
});
