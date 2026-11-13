// tos-close.pw.ts on pages booted ahead at the app's dashboard: the goto to
// the overview becomes a route change in the booted app.
import { expect, isStock, test } from './fixtures';
import { closeFlow, REPEAT } from './tos-flow';

test.use({ pyro: { boot: [{ url: '/tos/platform', routes: '/tos/platform/**' }] } });
test.skip(isStock, 'pyro boot config');

for (let i = 0; i < REPEAT; i++) {
  test(`closes an open Position from the Position overview #${i}`, async ({ browser, request, baseURL }) => {
    await closeFlow(browser, request, baseURL!);
  });
}

test('later tests take a booted page', async ({ browser, request, baseURL }) => {
  await closeFlow(browser, request, baseURL!);
  await closeFlow(browser, request, baseURL!);
  expect((browser as any)._stats.pooled).toBeGreaterThan(0);
});

test('a goto outside the boot routes gets a fresh page', async ({ page }) => {
  await page.goto('/simple');
  await expect(page.getByRole('heading', { name: 'Simple' })).toBeVisible();
});
