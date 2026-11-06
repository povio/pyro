// The rest of the Playwright API: dialogs from actions, handles, locator
// combinators, keyboard, popups, downloads, frames, bindings and HAR replay.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, test } from './fixtures';

test('dialogs an action opens reach page.on("dialog")', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<button onclick="out.textContent = confirm('Delete?') ? 'yes' : 'no'">Delete</button><p id="out"></p>
    <button onclick="out.textContent = prompt('Name?', 'x')">Ask</button>`);
  const messages: string[] = [];
  page.on('dialog', async (d) => {
    messages.push(`${d.type()}:${d.message()}`);
    if (d.type() === 'prompt') await d.accept('Ada');
    else await d.accept();
  });
  await page.getByRole('button', { name: 'Delete' }).click();
  await expect(page.locator('#out')).toHaveText('yes');
  await page.getByRole('button', { name: 'Ask' }).click();
  await expect(page.locator('#out')).toHaveText('Ada');
  expect(messages).toEqual(['confirm:Delete?', 'prompt:Name?']);
  page.removeAllListeners('dialog');
  page.off('dialog', () => {});
  await page.getByRole('button', { name: 'Delete' }).click();
  await expect(page.locator('#out')).toHaveText('no');
});

test('element and JS handles', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<ul><li class="a">One</li><li class="b">Two</li></ul><input id="t"><button onclick="this.textContent='done'">Go</button>`);
  const li = await page.$('li');
  expect(await li!.textContent()).toBe('One');
  expect(await page.$('#missing')).toBeNull();
  const items = await page.$$('li');
  expect(await Promise.all(items.map((h) => h.getAttribute('class')))).toEqual(['a', 'b']);
  expect(await page.evaluate((el) => (el as HTMLElement).textContent, items[1])).toBe('Two');
  expect(await page.evaluate(({ a, b }) => (a as Element).textContent! + (b as Element).textContent, { a: items[0], b: items[1] })).toBe('OneTwo');
  const win = await page.evaluateHandle(() => ({ n: 41, s: 'x' }));
  expect(await win.evaluate((o: { n: number }) => o.n + 1)).toBe(42);
  expect(await (await win.getProperty('s')).jsonValue()).toBe('x');
  expect([...(await win.getProperties()).keys()]).toEqual(['n', 's']);
  await win.dispose();
  const input = await page.locator('#t').elementHandle();
  await input.fill('typed');
  await expect(page.locator('#t')).toHaveValue('typed');
  const button = await page.waitForSelector('button');
  await button!.click();
  await expect(page.locator('button')).toHaveText('done');
  expect(await button!.evaluate((b: HTMLElement) => b.hasAttribute('data-lp-handle'))).toBe(false);
  const ul = await page.$('ul');
  expect(await ul!.$$eval('li', (els) => els.map((e) => e.textContent))).toEqual(['One', 'Two']);
  expect(await (await ul!.$('.b'))!.innerText()).toBe('Two');
});

test('locator combinators, role options, XPath', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`
    <h1>Top</h1><h2>Sub</h2>
    <ul><li>Apple <b>red</b></li><li>Banana</li><li>Cherry <b>red</b></li></ul>
    <input type="checkbox" aria-label="A" checked><input type="checkbox" aria-label="B">
    <button aria-pressed="true">Bold</button><button aria-pressed="false">Italic</button><button disabled>Off</button>
    <button aria-expanded="true">Menu</button>`);
  await expect(page.getByRole('heading', { level: 2 })).toHaveText('Sub');
  await expect(page.getByRole('heading').or(page.locator('li'))).toHaveCount(5);
  await expect(page.locator('h2').or(page.locator('h1')).first()).toHaveText('Top');
  await expect(page.locator('li').and(page.locator(':has(b)'))).toHaveCount(2);
  await expect(page.locator('li').first().filter({ hasText: 'Apple' })).toHaveCount(1);
  await expect(page.locator('li').first().filter({ hasText: 'Banana' })).toHaveCount(0);
  await expect(page.locator('li').nth(1).nth(0)).toHaveText('Banana');
  await expect(page.getByRole('checkbox', { checked: true })).toHaveAccessibleName('A');
  await expect(page.getByRole('checkbox', { checked: false })).toHaveAccessibleName('B');
  await expect(page.getByRole('button', { pressed: true })).toHaveText('Bold');
  await expect(page.getByRole('button', { disabled: true })).toHaveText('Off');
  await expect(page.getByRole('button', { expanded: true })).toHaveText('Menu');
  await expect(page.locator('//li[b]')).toHaveCount(2);
  await expect(page.locator('xpath=//h1')).toHaveText('Top');
});

test('keyboard down/up and click modifiers', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<button id="b">B</button><input id="i"><p id="log"></p><script>
    const log = (s) => (document.getElementById('log').textContent += s + ';');
    b.addEventListener('click', (e) => log('click' + (e.shiftKey ? '+shift' : '') + (e.altKey ? '+alt' : '')));
    i.addEventListener('keydown', (e) => log('down:' + e.key + (e.shiftKey ? '+shift' : '')));
    i.addEventListener('keyup', (e) => log('up:' + e.key));
  </script>`);
  await page.locator('#b').click({ modifiers: ['Alt'] });
  await page.keyboard.down('Shift');
  await page.locator('#b').click();
  await page.keyboard.up('Shift');
  await page.locator('#i').focus();
  await page.keyboard.down('a');
  await page.keyboard.up('a');
  await expect(page.locator('#log')).toHaveText('click+alt;click+shift;down:a;up:a;');
  await expect(page.locator('#i')).toHaveValue('a');
});

test('popups', async ({ page, context }) => {
  await page.goto('/simple');
  await page.setContent(`<a href="/form" target="_blank">Open</a>`);
  const pages = context.waitForEvent('page');
  const popupEvent = page.waitForEvent('popup');
  await page.getByRole('link', { name: 'Open' }).click();
  const popup = await popupEvent;
  expect(await pages).toBe(popup);
  await popup.waitForLoadState();
  await expect(popup).toHaveURL(/\/form$/);
  await expect(popup.getByLabel('Name')).toBeVisible();
  expect(context.pages()).toContain(popup);
  await popup.close();
});

test('downloads', async ({ page }) => {
  await page.route('**/report.txt', (route) =>
    route.fulfill({ body: 'report body', headers: { 'content-type': 'text/plain', 'content-disposition': 'attachment; filename="report.txt"' } }),
  );
  await page.goto('/simple');
  await page.setContent(`<a href="/report.txt" download>Get report</a>`);
  const downloading = page.waitForEvent('download');
  await page.getByRole('link', { name: 'Get report' }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe('report.txt');
  const to = join(mkdtempSync(join(tmpdir(), 'lp-dl-test-')), 'saved.txt');
  await download.saveAs(to);
  expect(readFileSync(to, 'utf8')).toBe('report body');
  expect(await download.failure()).toBeNull();
});

test('frames', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<iframe name="inner" srcdoc="<p id=p>in frame</p><button>Inner</button>"></iframe>`);
  await expect(page.frameLocator('iframe').locator('#p')).toHaveText('in frame');
  await expect.poll(() => page.frames().length).toBe(2);
  const frame = page.frame('inner')!;
  expect(frame.parentFrame()).toBe(page.mainFrame());
  expect(page.mainFrame().childFrames()).toEqual([frame]);
  await expect(frame.locator('#p')).toHaveText('in frame');
  await frame.getByRole('button', { name: 'Inner' }).click();
  expect(await frame.evaluate(() => document.getElementById('p')!.textContent)).toBe('in frame');
  expect(await frame.title()).toBe('');
});

test('exposeFunction and exposeBinding', async ({ page, context }) => {
  await page.exposeFunction('add', (a: number, b: number) => a + b);
  await context.exposeBinding('who', ({ page: p }, x: string) => `${x} on ${p === page ? 'the page' : 'another page'}`);
  await page.goto('/simple');
  expect(await page.evaluate(() => (window as any).add(20, 22))).toBe(42);
  expect(await page.evaluate(() => (window as any).who('called'))).toBe('called on the page');
  await page.reload();
  expect(await page.evaluate(() => (window as any).add(1, 2))).toBe(3);
});

test('routeFromHAR replays recorded responses', async ({ page, baseURL }) => {
  const har = join(mkdtempSync(join(tmpdir(), 'lp-har-')), 'api.har');
  writeFileSync(har, JSON.stringify({
    log: {
      entries: [{
        request: { method: 'GET', url: `${baseURL}/api/data.json?name=Ada` },
        response: { status: 200, headers: [{ name: 'Content-Type', value: 'application/json' }], content: { mimeType: 'application/json', text: '{"greeting":"From HAR"}' } },
      }],
    },
  }));
  await page.routeFromHAR(har, { url: '**/api/**' });
  await page.goto('/form');
  await page.getByLabel('Name').fill('Ada');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('From HAR');
});
