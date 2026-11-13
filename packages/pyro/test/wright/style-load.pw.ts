// `<style>` elements fire `load` once their sheet applies, after the
// `@import`s it starts with arrive, or `error` when one of them fails, so
// stock Playwright's `page.addStyleTag({ content })` (which waits for that
// `load`) resolves. The orders and values are the ones stock Playwright
// 1.59.1 gives on its Chromium (147) for the same pages.
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test } from './fixtures';

let site: Server;
let SITE = '';
const requested: string[] = [];

const STYLES = `<!doctype html><script>window.log = []; document.addEventListener('DOMContentLoaded', () => log.push('dcl')); addEventListener('load', () => log.push('window:load'));</script>
<style onload="log.push('plain:load')" onerror="log.push('plain:error')">p { color: rgb(1, 2, 3) }</style>
<style onload="log.push('import:load')" onerror="log.push('import:error')">@import url(/slow.css);</style>
<style onload="log.push('missing:load')" onerror="log.push('missing:error')">@import url(/missing.css);</style>
<style onload="log.push('nested:load')" onerror="log.push('nested:error')">@import url(/nest.css);</style>
<style type="text/plain" onload="log.push('text-plain:load')" onerror="log.push('text-plain:error')">p {}</style>
<p>styled</p><script>log.push('script:' + getComputedStyle(document.documentElement).color)</script>`;

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const path = new URL(req.url!, 'http://site').pathname;
    requested.push(path);
    const css = { 'content-type': 'text/css' };
    if (path === '/styles') res.writeHead(200, { 'content-type': 'text/html' }).end(STYLES);
    else if (path === '/sheet.css') res.writeHead(200, css).end('body { background-color: rgb(4, 5, 6) }');
    else if (path === '/slow.css') setTimeout(() => res.writeHead(200, css).end('html { color: rgb(7, 8, 9) }'), 100);
    // A failed import of an imported sheet does not fail the <style>.
    else if (path === '/nest.css') res.writeHead(200, css).end('@import url(/missing-nested.css); p { margin: 1px }');
    else if (path.endsWith('.css')) res.writeHead(404, css).end('/* missing */');
    else res.writeHead(200, { 'content-type': 'text/html' }).end(`<!doctype html><title>${path}</title><p>${path}</p>`);
  });
  await new Promise<void>((resolve) => site.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(site.address() as AddressInfo).port}`;
});

test.afterAll(() => {
  site.closeAllConnections();
  site.close();
});

test('addStyleTag: content and url resolve, their sheets applied', async ({ page }) => {
  await page.goto(`${SITE}/blank`);
  const style = (property: string) => page.evaluate((p) => getComputedStyle(document.body).getPropertyValue(p), property);
  await page.addStyleTag({ content: 'body { color: rgb(1, 2, 3) }' });
  expect(await style('color')).toBe('rgb(1, 2, 3)');
  await page.addStyleTag({ url: '/sheet.css' });
  expect(await style('background-color')).toBe('rgb(4, 5, 6)');
  // Stock Playwright resolves once the import applied; wright's own does not wait for it.
  await page.addStyleTag({ content: '@import url(/slow.css);' });
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).color)).toBe('rgb(7, 8, 9)');
});

test("parsed <style>s: load after their imports (error for a missing one), before the window's load", async ({ page }) => {
  await page.goto(`${SITE}/styles`);
  await page.waitForFunction(() => (window as any).log.includes('window:load'));
  const log: string[] = await page.evaluate(() => (window as any).log);
  // The script waited for the pending import (a script-blocking sheet).
  expect(log[0]).toBe('plain:load');
  expect(log).toContain('script:rgb(7, 8, 9)');
  expect(log.indexOf('script:rgb(7, 8, 9)')).toBeLessThan(log.indexOf('import:load'));
  expect(log.filter((e) => /:(load|error)$/.test(e) && !e.startsWith('window')).sort()).toEqual(['import:load', 'missing:error', 'nested:load', 'plain:load']);
  expect(log.at(-1)).toBe('window:load');
  expect(requested).toContain('/missing-nested.css');
});

test('a <style> script inserts: load in a later task once applied, again for new text, error for a failed import', async ({ page }) => {
  await page.goto(`${SITE}/blank`);
  expect(
    await page.evaluate(
      () =>
        new Promise((resolve) => {
          const log: string[] = [];
          let loads = 0;
          const s = document.createElement('style');
          s.textContent = 'body { color: rgb(1, 2, 3) }';
          s.onload = () => {
            log.push('load:' + getComputedStyle(document.body).color);
            if (++loads === 1) setTimeout(() => (s.textContent = 'body { color: rgb(3, 2, 1) }'));
            else resolve(log);
          };
          document.head.append(s);
          log.push('sync');
          queueMicrotask(() => log.push('microtask'));
        }),
    ),
  ).toEqual(['sync', 'microtask', 'load:rgb(1, 2, 3)', 'load:rgb(3, 2, 1)']);
  expect(
    await page.evaluate(
      () =>
        new Promise((resolve) => {
          const s = document.createElement('style');
          s.textContent = '@import url(/missing-inserted.css);';
          s.onload = () => resolve('load');
          s.onerror = (e) => resolve(`error:${(e as Event).bubbles}:${(e as Event).cancelable}`);
          document.head.append(s);
        }),
    ),
  ).toBe('error:false:false');
  // The import is fetched as Chromium fetches it: a `css` resource.
  expect(await page.evaluate(() => performance.getEntriesByType('resource').filter((e) => e.name.endsWith('/missing-inserted.css')).map((e) => (e as PerformanceResourceTiming).initiatorType))).toEqual(['css']);
});
