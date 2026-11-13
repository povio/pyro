// HTML fires a document's `load`, `pageshow`, `pagehide`, `unload` and
// `beforeunload` at its Window with the legacy target override flag: the
// listeners are the Window's (`currentTarget`, `this`, `composedPath()`),
// and `target` (and `srcElement`) is the document. Every value checked here
// is what stock Playwright 1.59.1 reads on its Chromium 147 for the same pages.
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test } from './fixtures';

let site: Server;
let SITE = '';

/** Records each event a listener sees, by label, in `seen` and in localStorage (read after leaving). */
const RECORD = `<script>
window.seen = {};
const name = (t) => t === window ? 'window' : t === document ? 'document' : t === null ? null : t.nodeName;
window.rec = (label) => function (e) {
  const r = {
    type: e.type, target: name(e.target), srcElement: name(e.srcElement), currentTarget: name(e.currentTarget),
    self: name(this), phase: e.eventPhase, path: e.composedPath().map(name), bubbles: e.bubbles,
    cancelable: e.cancelable, trusted: e.isTrusted, kind: e.constructor.name, current: window.event === e,
  };
  if ('persisted' in e) r.persisted = e.persisted;
  seen[label] = r;
  const stored = JSON.parse(localStorage.getItem('seen') || '{}');
  stored[label] = r;
  localStorage.setItem('seen', JSON.stringify(stored));
};
addEventListener('load', rec('load listener'));
addEventListener('load', rec('load capture'), true);
addEventListener('pageshow', rec('pageshow'));
addEventListener('pagehide', rec('pagehide'));
addEventListener('unload', rec('unload'));
addEventListener('beforeunload', rec('beforeunload'));
// The Window is the event's whole path: the document's own listeners hear none of them.
document.addEventListener('load', rec('document load'), true);
document.addEventListener('pageshow', rec('document pageshow'), true);
addEventListener('load', (e) => { window.after = e; });
</script>`;

test.beforeAll(async () => {
  site = createServer((req, res) => {
    const path = new URL(req.url!, 'http://site').pathname;
    const html = { 'content-type': 'text/html' };
    if (path === '/handler') res.writeHead(200, html).end(`<!doctype html>${RECORD}<script>window.onload = rec('onload')</script><p>handler</p>`);
    else if (path === '/body') {
      res.writeHead(200, html).end(`<!doctype html>${RECORD}<body onload="rec('body onload').call(this, event)" onpageshow="rec('body onpageshow').call(this, event)"><p>body</p></body>`);
    } else if (path === '/frame') res.writeHead(200, html).end(`<!doctype html>${RECORD}<p>frame</p>`);
    else if (path === '/parent') {
      res
        .writeHead(200, html)
        .end(`<!doctype html>${RECORD}<iframe id="f" src="/frame" onload="rec('iframe onload').call(this, event)"></iframe><script>document.getElementById('f').addEventListener('load', rec('iframe listener'))</script>`);
    } else res.writeHead(200, html).end(`<!doctype html><title>${path}</title>`);
  });
  await new Promise<void>((resolve) => site.listen(0, '127.0.0.1', resolve));
  SITE = `http://127.0.0.1:${(site.address() as AddressInfo).port}`;
});

test.afterAll(() => {
  site.closeAllConnections();
  site.close();
});

const LOAD = { type: 'load', target: 'document', srcElement: 'document', currentTarget: 'window', self: 'window', phase: 2, path: ['window'], bubbles: false, cancelable: false, trusted: true, kind: 'Event', current: true };
// A page transition event bubbles and is cancelable (HTML "fire a page transition event").
const PAGESHOW = { ...LOAD, type: 'pageshow', bubbles: true, cancelable: true, kind: 'PageTransitionEvent', persisted: false };

test("a document's load and pageshow have the document as target and the Window as current target", async ({ page }) => {
  await page.goto(`${SITE}/handler`);
  await page.waitForFunction(() => (window as any).seen.pageshow);
  expect(await page.evaluate(() => (window as any).seen)).toEqual({ 'load listener': LOAD, 'load capture': LOAD, onload: LOAD, pageshow: PAGESHOW });
  // After the dispatch the target stays the document. (Chromium also keeps
  // currentTarget, eventPhase and composedPath(), which DOM resets.)
  expect(await page.evaluate(() => (window as any).after.target === document)).toBe(true);

  // <body onload> and <body onpageshow> are the Window's handlers.
  await page.goto(`${SITE}/body`);
  await page.waitForFunction(() => (window as any).seen['body onpageshow']);
  expect(await page.evaluate(() => (window as any).seen)).toEqual({
    'load listener': LOAD,
    'load capture': LOAD,
    'body onload': LOAD,
    pageshow: PAGESHOW,
    'body onpageshow': PAGESHOW,
  });
});

test("an iframe's load at its element keeps its target; its document's load is its own", async ({ page }) => {
  await page.goto(`${SITE}/parent`);
  await page.waitForFunction(() => (window as any).seen.pageshow);
  const element = { type: 'load', target: 'IFRAME', srcElement: 'IFRAME', currentTarget: 'IFRAME', self: 'IFRAME', phase: 2, path: ['IFRAME', 'BODY', 'HTML', 'document'], bubbles: false, cancelable: false, trusted: true, kind: 'Event', current: true };
  expect(await page.evaluate(() => (window as any).seen)).toEqual({
    // The element's load passes the parent's document (not its Window), capturing.
    'document load': { ...element, currentTarget: 'document', self: 'document', phase: 1 },
    'iframe onload': element,
    'iframe listener': element,
    'load listener': LOAD,
    'load capture': LOAD,
    pageshow: PAGESHOW,
  });
  const frame = page.frames().find((f) => f.url() === `${SITE}/frame`)!;
  expect(await frame.evaluate(() => (window as any).seen)).toEqual({ 'load listener': LOAD, 'load capture': LOAD, pageshow: PAGESHOW });
});

test('leaving a document: beforeunload, pagehide and unload have the document as target', async ({ page }) => {
  await page.goto(`${SITE}/handler`);
  await page.waitForFunction(() => (window as any).seen.pageshow);
  await page.evaluate(() => localStorage.setItem('seen', '{}'));
  await page.goto(`${SITE}/next`);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem('seen')!))).toEqual({
    beforeunload: { ...LOAD, type: 'beforeunload', cancelable: true, kind: 'BeforeUnloadEvent' },
    pagehide: { ...PAGESHOW, type: 'pagehide' },
    unload: { ...LOAD, type: 'unload' },
  });
});
