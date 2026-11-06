// Compile-only checks (`bun run test:types`): what the types accept and,
// with @ts-expect-error, what they reject.
import { expect, test, type Locator, type Page } from '../../src/playwright/index.js';

test('types', async ({ page, request }) => {
  const l: Locator = page.getByRole('button', { name: 'Go' });
  // @ts-expect-error not an ARIA role
  page.getByRole('buton');
  await expect(l).toHaveText('Go');
  await expect(l).not.toBeVisible({ timeout: 10 });
  // @ts-expect-error a Locator matcher on a value
  await expect(1).toHaveText('1');
  await expect(page).toHaveURL(/x/);
  // @ts-expect-error a Page matcher on a Locator
  await expect(l).toHaveURL(/x/);
  await expect(await request.get('/')).toBeOK();
  expect(1).toBe(1);
  // @ts-expect-error toBeGreaterThan takes a number
  expect(1).toBeGreaterThan('2');
  await expect.poll(() => 1).toBe(1);
  await expect(async () => {}).toPass();
  // @ts-expect-error toPass needs a function
  await expect(1).toPass();
  // @ts-expect-error snapshots are not supported
  expect('x').toMatchSnapshot();
  const ext = expect.extend({ toBeEven(r: number, by?: number) { return { pass: r % (by ?? 2) === 0, message: () => '' }; } });
  ext(4).toBeEven();
  ext(4).not.toBeEven(3);
  // @ts-expect-error toBeEven takes a number
  ext(4).toBeEven('x');
  const n: number = await page.evaluate(() => 1);
  const s: string = await page.evaluate((a) => a.x, { x: 'y' });
  // @ts-expect-error evaluate returns a number here
  const bad: string = await page.evaluate(() => 1);
  // @ts-expect-error deprecated, removed
  await l.type('x');
  // @ts-expect-error deprecated, removed
  page.accessibility;
  const el = await page.$('div');
  const tag: string | undefined = await el?.evaluate((e) => e.tagName);
  void [n, s, bad, tag, (p: Page) => p];
});
