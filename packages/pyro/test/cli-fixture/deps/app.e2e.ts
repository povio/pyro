import { expect, test } from '../../../src/wright/index';

test('the request fixture sends the saved cookie', async ({ request, storageState }) => {
  expect(storageState).toBe(process.env.DEPS_STATE);
  expect(await (await request.get('/whoami')).text()).toBe('session=signed-in');
});

test('the page starts with the saved cookie', async ({ page }) => {
  await page.goto('/whoami');
  await expect(page.locator('body')).toHaveText('session=signed-in');
});
