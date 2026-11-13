// Playwright API coverage of `@povio/pyro/test`: each test is plain
// Playwright code against the bench site (bench/scripts/cdp-perf-site.mjs).
import { expect, test } from './fixtures';

const lpOnly = () => test.skip(process.env.PW_ENGINE === 'chromium-cdp', 'pyro specific');

test('form flow: fill, click, text, response', async ({ page }) => {
  await page.goto('/form');
  await expect(page).toHaveTitle('form');
  await expect(page).toHaveURL(/\/form$/);
  await page.getByLabel('Name').fill('Ada');
  await expect(page.getByLabel('Name')).toHaveValue('Ada');
  const response = page.waitForResponse('**/api/data.json?name=Ada');
  await page.getByRole('button', { name: 'Send' }).click();
  const r = await response;
  expect(r.status()).toBe(200);
  expect(await r.json()).toEqual({ greeting: 'Hello, Ada' });
  await expect(page.locator('#out')).toHaveText('Hello, Ada');
  await expect(page.locator('#out')).toContainText('ada', { ignoreCase: true });
  await expect(page.locator('#out')).toHaveText(/^Hello, \w+$/);
  await expect(page.locator('#out')).not.toHaveText('Hello, Bob');
  expect(await page.locator('#out').textContent()).toBe('Hello, Ada');
  expect(await page.locator('#out').innerText()).toBe('Hello, Ada');
});

test('click counts, reads, evaluate', async ({ page }) => {
  await page.goto('/click');
  const button = page.getByRole('button', { name: 'Click' });
  await button.click();
  await button.dblclick();
  await expect(page.locator('#n')).toHaveText('3');
  expect(await page.evaluate(() => (window as any).__clicks)).toBe(3);
  expect(await page.evaluate((x) => x * 2, 21)).toBe(42);
  expect(await page.evaluate('1 + 1')).toBe(2);
  expect(await button.evaluate((el: HTMLElement, suffix: string) => el.textContent + suffix, '!')).toBe('Click!');
  expect(await page.locator('button, input').count()).toBe(2);
  expect(await button.isVisible()).toBe(true);
  expect(await page.locator('#missing').isVisible()).toBe(false);
  expect(await button.getAttribute('type')).toBe('button');
  const rect = await button.evaluate((el: Element) => el.getBoundingClientRect().toJSON());
  expect(await button.boundingBox()).toMatchObject({ width: rect.width, height: rect.height });
  await page.locator('#t').pressSequentially('hi');
  await expect(page.locator('#t')).toHaveValue('hi');
  await page.locator('#t').clear();
  await expect(page.locator('#t')).toHaveValue('');
  await expect(page.locator('#t')).toBeEditable();
  await expect(page.locator('#t')).toBeEmpty();
  await page.locator('#t').focus();
  await expect(page.locator('#t')).toBeFocused();
  await page.keyboard.type('ok');
  await expect(page.locator('#t')).toHaveValue('ok');
});

test('evaluate keeps doubles exact both ways', async ({ page }) => {
  // 0.9591810769706007 once came back as ...008; one random double in ten was off by one ulp.
  await page.goto('/simple');
  const read = await page.evaluate(() => {
    const xs = [0.9591810769706007, ...Array.from({ length: 1000 }, Math.random)];
    return { xs, texts: xs.map(String) };
  });
  expect(read.xs.map(String)).toEqual(read.texts);
  const sent = [0.9591810769706007, ...Array.from({ length: 1000 }, Math.random)];
  expect(await page.evaluate((xs) => xs.map(String), sent)).toEqual(sent.map(String));
});

test('setContent: roles, filters, checks, select, attributes', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`
    <h1>Shop</h1>
    <ul>
      <li class="item a">Apple <button>Buy</button></li>
      <li class="item b">Banana <button>Buy</button></li>
      <li class="item c" hidden>Cherry <button>Buy</button></li>
    </ul>
    <label><input type="checkbox" id="agree"> Agree</label>
    <select id="fruit"><option value="a">Apple</option><option value="b">Banana</option></select>
    <button disabled>Pay</button>
    <div data-testid="total" title="Total price" style="color: rgb(255, 0, 0)">0</div>
  `);
  await expect(page.getByRole('heading', { name: 'Shop' })).toBeVisible();
  await expect(page.getByRole('listitem')).toHaveCount(2);
  await expect(page.locator('li')).toHaveCount(3);
  await expect(page.locator('li').filter({ visible: true })).toHaveCount(2);
  await expect(page.locator('li:visible')).toHaveCount(2);
  await expect(page.locator('li').filter({ hasText: 'Banana' })).toHaveClass('item b');
  await expect(page.locator('li').filter({ hasText: 'Banana' })).toContainClass('b');
  await expect(page.locator('li').filter({ visible: true })).toHaveText(['Apple Buy', 'Banana Buy']);
  await expect(page.locator('li').filter({ visible: true })).toContainText(['Banana']);
  expect(await page.locator('li').filter({ visible: true }).allTextContents()).toEqual(['Apple Buy', 'Banana Buy']);
  await page.locator('li').filter({ hasText: 'Banana' }).getByRole('button', { name: 'Buy' }).click();
  // getByRole skips hidden elements: Cherry's button does not count.
  await expect(page.locator('li').filter({ has: page.getByRole('button') })).toHaveCount(2);
  await expect(page.locator('li').filter({ has: page.locator('button') })).toHaveCount(3);
  await expect(page.locator('li').last()).toBeHidden();
  await expect(page.locator('li').nth(1)).toHaveText(/Banana/);
  await page.getByLabel('Agree').check();
  await expect(page.getByLabel('Agree')).toBeChecked();
  expect(await page.getByLabel('Agree').isChecked()).toBe(true);
  await page.getByLabel('Agree').setChecked(false);
  await expect(page.getByLabel('Agree')).not.toBeChecked();
  expect(await page.locator('#fruit').selectOption('Banana')).toEqual(['b']);
  await expect(page.locator('#fruit')).toHaveValue('b');
  await expect(page.getByRole('button', { name: 'Pay' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Pay' })).not.toBeEnabled();
  await expect(page.getByTestId('total')).toHaveText('0');
  await expect(page.getByTestId('total')).toHaveAttribute('title', 'Total price');
  await expect(page.getByTestId('total')).toHaveAttribute('title');
  await expect(page.getByTitle('Total price')).toHaveCount(1);
  await expect(page.getByTestId('total')).toHaveCSS('color', 'rgb(255, 0, 0)');
  await expect(page.getByTestId('total')).toHaveId('');
  await expect(page.getByText('Cherry')).toBeAttached();
  await expect(page.getByText('Durian')).not.toBeAttached();
  expect(await page.content()).toContain('<h1>Shop</h1>');
});

test('waiting: auto-wait for late elements, waitForFunction, expect.poll', async ({ page }) => {
  await page.goto('/simple');
  await page.evaluate(() => {
    setTimeout(() => {
      const b = document.createElement('button');
      b.textContent = 'Late';
      b.onclick = () => (document.title = 'clicked');
      document.body.append(b);
    }, 150);
  });
  await page.getByRole('button', { name: 'Late' }).click();
  await expect(page).toHaveTitle('clicked');
  await page.evaluate(() => setTimeout(() => ((window as any).ready = 7), 100));
  const handle = await page.waitForFunction(() => (window as any).ready);
  expect(await handle.jsonValue()).toBe(7);
  await expect.poll(() => page.evaluate(() => (window as any).ready)).toBe(7);
  await page.waitForTimeout(1);
});

test('failures read like Playwright', async ({ page }) => {
  await page.goto('/simple');
  const error = await page.getByRole('button', { name: 'Nope' }).click({ timeout: 300 }).catch((e) => e);
  expect(error.name).toBe('TimeoutError');
  expect(error.message).toContain('Timeout 300ms exceeded');
  const failed = await expect(page.getByText('Nope')).toBeVisible({ timeout: 200 }).catch((e) => e);
  expect(String(failed.message)).toContain('toBeVisible');
  expect(String(failed.message)).toContain("getByText('Nope')");
});

test('routes in place: pushState URL tracking and waitForURL', async ({ page }) => {
  await page.goto('/simple');
  await page.evaluate(() => history.pushState(null, '', '/simple?step=2'));
  expect(page.url()).toMatch(/\/simple\?step=2$/);
  await page.waitForURL('**/simple?step=2');
  await expect(page).toHaveURL('/simple?step=2');
});

test('context: storageState localStorage, init scripts, cookies, events', async ({ browser, baseURL }) => {
  lpOnly();
  const ctx = await browser.newContext({
    storageState: { cookies: [{ name: 'sid', value: 'abc', url: baseURL! }], origins: [{ origin: baseURL!, localStorage: [{ name: 'token', value: 't1' }] }] },
  });
  await ctx.addInitScript((v) => ((window as any).__init = v), 'yes');
  const page = await ctx.newPage();
  const consoles: string[] = [];
  page.on('console', (m) => consoles.push(m.text()));
  await page.goto('/simple');
  expect(await page.evaluate(() => localStorage.getItem('token'))).toBe('t1');
  expect(await page.evaluate(() => (window as any).__init)).toBe('yes');
  expect(await page.evaluate(() => document.cookie)).toContain('sid=abc');
  await page.evaluate(() => console.log('hello', 1));
  await expect.poll(() => consoles).toContain('hello 1');
  expect((await ctx.cookies()).map((c) => c.name)).toContain('sid');
  const state = await ctx.storageState();
  expect(state.origins[0].localStorage).toContainEqual({ name: 'token', value: 't1' });
  await ctx.close();
});

test('frames: frameLocator', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<iframe id="f" srcdoc="<button>Inner</button><p id=p>in frame</p>"></iframe>`);
  const frame = page.frameLocator('#f');
  await expect(frame.getByRole('button', { name: 'Inner' })).toBeVisible();
  await expect(frame.locator('#p')).toHaveText('in frame');
  await expect(page.locator('#f').contentFrame().locator('#p')).toHaveText('in frame');
});

test('dialogs are dismissed unless handled', async ({ page }) => {
  await page.goto('/simple');
  expect(await page.evaluate(() => confirm('sure?'))).toBe(false);
  page.once('dialog', (d) => d.accept());
  expect(await page.evaluate(() => confirm('sure?'))).toBe(true);
});
