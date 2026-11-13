// A download a popup's navigation turns into (a `target=_blank` link or
// `window.open` answered with an attachment) is its opener's. What each test
// checks is what stock Playwright 1.59.1 gives on its Chromium 147 for the
// same pages: the full build (`channel: 'chromium'`, as Chrome) closes the tab
// it opened for the download; chromium-headless-shell, Playwright's default
// headless browser, leaves it open on its uncommitted initial document.
import { readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test } from './fixtures';
import type { Page } from '../../src/wright/index';

let site: Server;
let SITE = '';

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const path = new URL(req.url!, 'http://site').pathname;
    if (path === '/file.zip') {
      res.writeHead(200, { 'content-type': 'application/zip', 'content-disposition': 'attachment; filename=report.zip' }).end('zipbytes');
    } else if (path === '/opener') {
      res
        .writeHead(200, { 'content-type': 'text/html' })
        .end(
          '<!doctype html><title>opener</title><a id="link" target="_blank" href="/file.zip">link</a>' +
            '<button id="open" onclick="window.w = window.open(\'/file.zip\')">open</button><a id="page" target="_blank" href="/next">next</a>',
        );
    } else res.writeHead(200, { 'content-type': 'text/html' }).end(`<!doctype html><title>${path}</title>`);
  });
  await new Promise<void>((resolve) => site.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(site.address() as AddressInfo).port}`;
});

test.afterAll(() => {
  site.closeAllConnections();
  site.close();
});

for (const via of ['target=_blank', 'window.open'] as const) {
  test(`a download a popup ${via} opened is its opener's, and the popup closes`, async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.goto(`${SITE}/opener`);
    // Chromium: `popup` comes too (for a popup with no document of its own),
    // then the popup closes.
    const popup = page.waitForEvent('popup');
    const [download] = await Promise.all([page.waitForEvent('download'), page.locator(via === 'window.open' ? '#open' : '#link').click()]);
    expect(download.page()).toBe(page);
    expect([download.url(), download.suggestedFilename()]).toEqual([`${SITE}/file.zip`, 'report.zip']);
    expect(readFileSync(await download.path(), 'utf8')).toBe('zipbytes');
    const opened: Page = await popup;
    if (!opened.isClosed()) await opened.waitForEvent('close');
    // No stray page: the opener is the context's one page and stays where it was.
    expect(context.pages().length).toBe(1);
    expect(context.pages()[0]).toBe(page);
    expect(page.url()).toBe(`${SITE}/opener`);
    // The object window.open returned sees its popup closed.
    if (via === 'window.open') expect(await page.evaluate(() => (window as any).w.closed)).toBe(true);
    await context.close();
  });
}

test('a popup with a document of its own stays open', async ({ browser }) => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(`${SITE}/opener`);
  const [popup] = await Promise.all([page.waitForEvent('popup'), page.locator('#page').click()]);
  await popup.waitForLoadState();
  expect(await popup.title()).toBe('/next');
  expect(context.pages().length).toBe(2);
  expect(context.pages()[1]).toBe(popup);
  await context.close();
});
