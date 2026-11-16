// A 0 timeout is no limit, as in Playwright (Playwright Test's default
// action and navigation timeouts are 0: the test's own timeout bounds them).
// Passes on stock Playwright too.
import { expect, isStock, test } from './fixtures';

/** Adds `<button id=late>` after `ms`. */
const late = (page: any, ms: number) =>
  page.evaluate((ms: number) => {
    document.getElementById('late')?.remove();
    setTimeout(() => document.body.insertAdjacentHTML('beforeend', '<button id="late">late</button>'), ms);
  }, ms);

test.describe('timeouts', () => {
  test('a 0 timeout waits without limit', async ({ page, context }) => {
    await page.setContent('<p>page</p>');
    await late(page, 600);
    await expect(page.locator('#late')).toBeVisible({ timeout: 0 });
    await late(page, 600);
    await expect(page.locator('#late')).toHaveText('late', { timeout: 0 });
    await late(page, 600);
    await page.locator('#late').click({ timeout: 0 });
    await late(page, 600);
    await page.locator('#late').waitFor({ timeout: 0 });
    await late(page, 600);
    page.setDefaultTimeout(0);
    await page.locator('#late').click();
    page.setDefaultTimeout(30_000);
    const opened = context.waitForEvent('page', { timeout: 0 });
    setTimeout(() => void context.newPage(), 300);
    expect(await opened).toBeTruthy();
    const event = page.waitForEvent('console', { timeout: 0 });
    await page.evaluate(() => setTimeout(() => console.log('late'), 300));
    expect((await event).text()).toBe('late');
  });

  test('timeouts resolve as Playwright resolves them', async ({ page, context }) => {
    test.skip(isStock, 'pyro internals');
    const p = page as any;
    // The runner's default: actionTimeout and navigationTimeout 0, no limit.
    expect(p._timeout()).toBe(2 ** 31 - 1);
    expect(p._navTimeout()).toBe(2 ** 31 - 1);
    // No 600 s cap.
    expect(p._timeout(900_000)).toBe(900_000);
    // The page's default timeout comes before the context's navigation timeout.
    context.setDefaultNavigationTimeout(5_000);
    page.setDefaultTimeout(1_234);
    expect(p._navTimeout()).toBe(1_234);
    page.setDefaultNavigationTimeout(2_000);
    expect([p._timeout(), p._navTimeout(), p._navTimeout(0)]).toEqual([1_234, 2_000, 2 ** 31 - 1]);
  });

  test('isVisible and isHidden take the deprecated timeout and do not wait', async ({ page }) => {
    await page.setContent('<p>page</p>');
    await late(page, 500);
    const started = Date.now();
    expect(await page.locator('#late').isVisible({ timeout: 5_000 })).toBe(false);
    expect(await page.locator('#late').isHidden({ timeout: 5_000 })).toBe(true);
    expect(Date.now() - started).toBeLessThan(400);
  });
});
