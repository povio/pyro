// The rest of the Playwright API: dialogs from actions, handles, locator
// combinators, keyboard, popups, downloads, frames, bindings and HAR replay.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
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
  expect(await button!.evaluate((b: HTMLElement) => b.hasAttribute('data-pyro-handle'))).toBe(false);
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
  const to = join(mkdtempSync(join(tmpdir(), 'pyro-dl-test-')), 'saved.txt');
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
  const har = join(mkdtempSync(join(tmpdir(), 'pyro-har-')), 'api.har');
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

test('selector strings: internal engines and Playwright pseudo-classes', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`
    <section class="card"><h2>Fruit</h2><ul><li>Apple <b>red</b></li><li>Banana</li></ul><button>Save</button></section>
    <input placeholder="Search here"><img alt="Logo"><div data-testid="total">0</div>
    <iframe id="f" srcdoc="<button>Inside</button>"></iframe>`);
  await expect(page.locator('internal:role=button[name="Save"i]')).toHaveCount(1);
  await expect(page.locator('internal:role=heading[level=2]')).toHaveText('Fruit');
  await expect(page.locator('internal:text="Banana"s')).toHaveCount(1);
  await expect(page.locator('internal:attr=[placeholder="Search"i]')).toHaveCount(1);
  await expect(page.locator('internal:attr=[alt="Logo"s]')).toHaveCount(1);
  await expect(page.locator('internal:testid=[data-testid="total"s]')).toHaveText('0');
  await expect(page.locator('li >> internal:has="b"')).toHaveText('Apple red');
  await expect(page.locator('li >> internal:has-not="b"')).toHaveText('Banana');
  await expect(page.locator('li >> internal:has-text="banana"i')).toHaveCount(1);
  await expect(page.locator('h2 >> internal:or="button"')).toHaveCount(2);
  await expect(page.locator('li >> internal:and="li:has(b)"')).toHaveCount(1);
  await expect(page.locator('section >> internal:chain="button"')).toHaveText('Save');
  await expect(page.locator('#f >> internal:control=enter-frame >> internal:role=button')).toHaveText('Inside');
  await expect(page.locator('li:has-text("ban")')).toHaveText('Banana');
  await expect(page.locator('section.card:has-text("Fruit") button')).toHaveText('Save');
  await expect(page.locator('section:has-text("Fruit") > button')).toHaveText('Save');
  await expect(page.locator('li:text("apple")')).toHaveCount(1);
  await expect(page.locator('b:text-is("red")')).toHaveCount(1);
  await expect(page.locator('li:text-matches("^ban", "i")')).toHaveCount(1);
  await expect(page.locator(':nth-match(li, 2)')).toHaveText('Banana');
  await expect(page.locator('li:visible')).toHaveCount(2);
  await expect(page.locator('h2, button')).toHaveCount(2);
});

test('child frames: handles, navigation, waiting', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<iframe name="inner" srcdoc="<p id=p>in frame</p><input id=i><button onclick=&quot;this.textContent='hit'&quot;>Inner</button>"></iframe>`);
  await expect(page.frameLocator('iframe').locator('#p')).toHaveText('in frame');
  await expect.poll(() => page.frames().length).toBe(2);
  const frame = page.frame('inner')!;
  const p = await frame.$('#p');
  expect(await p!.textContent()).toBe('in frame');
  expect(await p!.ownerFrame()).toBe(frame);
  const input = await frame.locator('#i').elementHandle();
  await input.fill('typed');
  await expect(frame.locator('#i')).toHaveValue('typed');
  const button = await frame.waitForSelector('button');
  await button!.click();
  await expect(frame.locator('button')).toHaveText('hit');
  const doc = await frame.evaluateHandle(() => ({ title: 'x', n: document.querySelectorAll('*').length }));
  expect(await (await doc.getProperty('title')).jsonValue()).toBe('x');
  expect(await (await page.$('iframe'))!.contentFrame()).toBe(frame);
  await frame.evaluate(() => setTimeout(() => ((window as any).ready = 1), 50));
  await frame.waitForFunction(() => (window as any).ready === 1);
  await frame.goto('/form');
  await frame.waitForURL(/\/form$/);
  expect(frame.url()).toMatch(/\/form$/);
  await expect(frame.getByLabel('Name')).toBeVisible();
  expect(await frame.title()).toBe('form');
});

test('routeFromHAR records with update, then replays', async ({ browser, baseURL }) => {
  const har = join(mkdtempSync(join(tmpdir(), 'pyro-har-rec-')), 'rec.har');
  const recording = await browser.newContext({ baseURL });
  await recording.routeFromHAR(har, { url: '**/api/**', update: true });
  const page = await recording.newPage();
  await page.goto('/form');
  await page.getByLabel('Name').fill('Rec');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('#out')).toHaveText('Hello, Rec');
  await recording.close();
  const log = JSON.parse(readFileSync(har, 'utf8')).log;
  const entry = log.entries.find((e: any) => e.request.url.endsWith('/api/data.json?name=Rec'));
  expect(entry.response.status).toBe(200);
  const content = entry.response.content;
  const body = content._file ? readFileSync(join(dirname(har), content._file), 'utf8') : content.text;
  expect(JSON.parse(body)).toEqual({ greeting: 'Hello, Rec' });
  expect(log.entries.every((e: any) => e.request.url.includes('/api/'))).toBe(true);

  // Replay, with the recorded body changed: the page shows the file's answer.
  delete entry.response.content._file;
  entry.response.content.text = JSON.stringify({ greeting: 'Replayed' });
  writeFileSync(har, JSON.stringify({ log }));
  const replay = await browser.newContext({ baseURL });
  await replay.routeFromHAR(har, { url: '**/api/**' });
  const again = await replay.newPage();
  await again.goto('/form');
  await again.getByLabel('Name').fill('Rec');
  await again.getByRole('button', { name: 'Send' }).click();
  await expect(again.locator('#out')).toHaveText('Replayed');
  await replay.close();
});

test('file choosers', async ({ page }) => {
  await page.goto('/simple');
  await page.setContent(`<input type="file" id="f" multiple><p id="out"></p><script>
    f.addEventListener('change', () => (out.textContent = [...f.files].map((x) => x.name + ':' + x.size).join(',')));
  </script>`);
  const choosing = page.waitForEvent('filechooser');
  await page.locator('#f').click();
  const chooser = await choosing;
  expect(chooser.isMultiple()).toBe(true);
  expect(await chooser.element().getAttribute('id')).toBe('f');
  await chooser.setFiles([{ name: 'a.txt', mimeType: 'text/plain', buffer: Buffer.from('hello') }]);
  await expect(page.locator('#out')).toHaveText('a.txt:5');
});

test('workers', async ({ page }) => {
  await page.route('**/worker.js', (route) =>
    route.fulfill({ contentType: 'text/javascript', body: 'self.answer = 42; self.onmessage = (e) => postMessage(e.data * 2);' }),
  );
  await page.goto('/simple');
  const started = page.waitForEvent('worker');
  await page.evaluate(() => {
    (window as any).w = new Worker('/worker.js');
  });
  const worker = await started;
  expect(worker.url()).toMatch(/\/worker\.js$/);
  await expect.poll(() => worker.evaluate(() => (self as any).answer)).toBe(42);
  expect(page.workers()).toContain(worker);
  expect(await page.evaluate(() => new Promise((r) => { (window as any).w.onmessage = (e: MessageEvent) => r(e.data); (window as any).w.postMessage(21); }))).toBe(42);
});
