// TOS's "closes an open Position from the Position overview", written as TOS
// writes it (bench/scripts/cdp-perf.mjs `pw.tos.close`): stock Playwright
// code, only the `test` import differs.
import { expect, test } from './fixtures';

const AUTH = { authorization: 'Bearer e2e-token' };
const overview = (id: string) => `/tos/platform/logistics/positions/${id}/overview`;

test.describe.configure({ mode: 'parallel' });

for (let i = 0; i < Number(process.env.REPEAT ?? 1); i++) {
  test(`closes an open Position from the Position overview #${i}`, async ({ browser, request, baseURL }) => {
    const seeded = await request.post(`${baseURL}/tos-api/positions`, { headers: AUTH });
    expect(seeded.status()).toBe(201);
    const p = (await seeded.json()) as { id: string; number: string };

    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await ctx.addInitScript(() => localStorage.setItem('auth-token', 'e2e-token'));
    const page = await ctx.newPage();
    const isGet = (r: { request(): { method(): string }; url(): string }) =>
      r.request().method() === 'GET' && r.url().endsWith(`/tos-api/positions/${p.id}`);
    try {
      const details = page.waitForResponse(isGet);
      await page.goto(overview(p.id));
      await details;
      await page.getByText(p.number).waitFor();
      await page.getByRole('button', { name: 'Settings', exact: true }).press('Enter');
      await page.getByRole('button', { name: 'Close Position' }).filter({ visible: true }).first().click();
      const dialog = page.getByRole('dialog').filter({ hasText: 'This position will be closed' }).last();
      await dialog.waitFor();
      const closed = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/close') && r.status() === 200);
      const refresh = page.waitForResponse(isGet);
      await dialog.getByRole('button', { name: 'Close Position', exact: true }).click();
      await Promise.all([closed, refresh]);
      await page.getByText(/^Position Closed · .* · Manual · Closed by:/).filter({ visible: true }).waitFor();
      await expect(page.getByRole('button', { name: 'Reopen Position' })).toHaveCount(1);
      await expect(page.getByRole('button', { name: 'Close Position' }).filter({ visible: true })).toHaveCount(0);
    } finally {
      await ctx.close();
    }
  });
}
