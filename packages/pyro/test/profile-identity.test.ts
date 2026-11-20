// A profile's `userAgent` and proxy: its document, its setup's requests and
// every page's requests carry them, in fork mode and without it, and so does
// what the engine loads for its pages (layout, screenshots, PDFs).
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { createServer, type Server } from 'node:net';
import { join } from 'node:path';
import { type Page, Pyro, evaluate, goto } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

/** Requests the site answered: `path user-agent`, and `via proxy` for those the proxy passed on. */
const seen: string[] = [];
let site: ReturnType<typeof Bun.serve>;
/** A forward proxy for `http://site.test/…`: it passes each request on to `site`. */
let proxy: Server;
let proxyPort = 0;
/** A 2x2 red PNG. */
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==', 'base64');
/**
 * A page whose CSS background image, `@font-face` font and `@import`ed sheet
 * (with an image of its own) only the engine loads, beside an `<img>`; their
 * names start with `prefix`.
 */
const assetsPage = (prefix: string) => `<!doctype html><html><head><style>
@import url(${prefix}imported.css);
@font-face { font-family: Probe; src: url(${prefix}font.ttf) format("truetype"); }
body { margin: 0 }
#bg { width: 20px; height: 20px; background-image: url(${prefix}bg.png) }
#imp { width: 10px; height: 10px }
#text { font-family: Probe, sans-serif }
</style></head><body><div id="bg"></div><div id="imp"></div><img id="img" src="img.png"><p id="text">text</p></body></html>`;
beforeAll(async () => {
  site = Bun.serve({
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      seen.push(`${path} ${req.headers.get('user-agent')}${req.headers.get('x-via-test-proxy') ? ' via proxy' : ''}`);
      // Not stored: every load reaches the network.
      const typed = (body: string | Buffer, type: string) => new Response(body, { headers: { 'content-type': type, 'cache-control': 'no-store' } });
      if (path.endsWith('/page')) return new Response('<!doctype html><div id="ready">ready</div>', { headers: { 'content-type': 'text/html' } });
      if (path.endsWith('assets-page')) return typed(assetsPage(path.endsWith('pdf-assets-page') ? 'pdf-' : ''), 'text/html');
      if (path.endsWith('imported.css')) return typed(`#imp { background-image: url(${path.endsWith('pdf-imported.css') ? 'pdf-' : ''}imported-bg.png) }`, 'text/css');
      if (path.endsWith('.png')) return typed(PNG, 'image/png');
      if (path.endsWith('.ttf')) return typed('not a font', 'font/ttf');
      return new Response(`ok ${path}`);
    },
  });
  proxy = createServer((socket) => {
    let head = '';
    socket.on('data', async (chunk) => {
      head += chunk.toString('latin1');
      if (!head.includes('\r\n\r\n')) return;
      const [line, ...fields] = head.split('\r\n\r\n')[0].split('\r\n');
      const [method, target] = line.split(' ');
      const headers: Record<string, string> = {};
      for (const field of fields) {
        const at = field.indexOf(':');
        const name = field.slice(0, at).toLowerCase();
        if (!['host', 'connection', 'proxy-connection'].includes(name)) headers[name] = field.slice(at + 1).trim();
      }
      // Only absolute-form requests for the test's host: anything else is not for this proxy.
      const upstream = target.startsWith('http://site.test/') ? target.replace('http://site.test', `http://127.0.0.1:${site.port}`) : null;
      if (!upstream) return void socket.end('HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n');
      const r = await fetch(upstream, { method, headers: { ...headers, 'x-via-test-proxy': '1' } });
      const body = Buffer.from(await r.arrayBuffer());
      socket.end(Buffer.concat([
        Buffer.from(`HTTP/1.1 ${r.status} OK\r\ncontent-type: ${r.headers.get('content-type')}\r\ncontent-length: ${body.length}\r\nconnection: close\r\n\r\n`),
        body,
      ]));
    });
  });
  await new Promise<void>((resolve) => proxy.listen(0, '127.0.0.1', resolve));
  proxyPort = (proxy.address() as { port: number }).port;
});
afterAll(() => {
  site.stop(true);
  proxy.close();
});
const origin = () => `http://127.0.0.1:${site.port}`;
const requests = (prefix: string) => seen.filter((line) => line.startsWith(prefix));

/**
 * Lays `page` out until the site saw each of `paths`: an arriving body makes
 * the next layout query lay the document out again with it (the imported
 * sheet's image is only asked for then).
 */
async function layOutUntil(page: Page, paths: string[]): Promise<void> {
  const deadline = Date.now() + 10000;
  for (;;) {
    await page.run([evaluate("['bg', 'imp', 'text'].map((id) => document.getElementById(id).getBoundingClientRect().width).join()")]);
    const missing = paths.filter((path) => requests(`${path} `).length === 0);
    if (missing.length === 0) return;
    if (Date.now() > deadline) throw new Error(`never requested: ${missing.join(', ')}; saw ${seen.join(' | ')}`);
    await Bun.sleep(30);
  }
}

test('a fork-mode profile sends its userAgent from its document, its setup and every page', async () => {
  const launcher = await Pyro.launch({ bin, args: ['--fork'] });
  const worker = await Pyro.connect(launcher.wsEndpoint);
  const agent = 'ScraperBot/1.0 (identity one)';
  try {
    const def = { url: `${origin()}/fork/page`, userAgent: agent, setup: [evaluate("fetch('/fork/setup').then((r) => r.text())")] };
    await launcher.profile('one', def);
    await using page = await launcher.page({ as: 'one' });
    const r = await page.run([evaluate('navigator.userAgent', 'agent'), evaluate("fetch('/fork/page-fetch').then((r) => r.text())", 'fetched')]);
    expect(r.values).toEqual({ agent, fetched: 'ok /fork/page-fetch' });
    // Another client's page of it, and a profile derived from it: the same agent.
    await worker.profile('derived', { base: 'one', url: def.url, setup: [evaluate("fetch('/fork/derived-setup').then((r) => r.text())")] });
    await using derived = await worker.page({ as: 'derived' });
    expect((await derived.run([evaluate("fetch('/fork/derived-page').then(() => navigator.userAgent)", 'agent')])).values.agent).toBe(agent);
    expect(requests('/fork/')).toEqual([
      `/fork/page ${agent}`,
      `/fork/setup ${agent}`,
      `/fork/page-fetch ${agent}`,
      `/fork/derived-setup ${agent}`,
      `/fork/derived-page ${agent}`,
    ]);
    // The agent is part of the definition: a different one is refused while the name is held.
    await expect(worker.profile('one', { ...def, userAgent: 'Other/2.0' })).rejects.toThrow('different definition');
    // A derived profile has its base's agent and proxy.
    await expect(worker.profile('mismatch', { base: 'one', url: def.url, userAgent: 'Other/2.0' })).rejects.toThrow("has its base's userAgent");
    // Pages of other profiles keep Pyro's own agent.
    await launcher.profile('plain', { url: `${origin()}/plain/page`, setup: [evaluate('1')] });
    await using plain = await launcher.page({ as: 'plain' });
    expect((await plain.run([evaluate('navigator.userAgent', 'agent')])).values.agent).toStartWith('Mozilla/5.0 (compatible; pyro/');
  } finally {
    worker.close();
    launcher.close();
  }
}, 30000);

test('a profile goes through its own proxy, in fork mode and without it', async () => {
  for (const fork of [true, false]) {
    const lp = await Pyro.launch({ bin, args: fork ? ['--fork'] : [] });
    const tag = fork ? 'fork' : 'thread';
    const agent = `Proxied/1.0 (${tag})`;
    try {
      // `site.test` resolves nowhere: only the proxy can answer for it.
      await lp.profile('proxied', {
        url: `http://site.test/${tag}-proxy/page`,
        userAgent: agent,
        proxyServer: `http://127.0.0.1:${proxyPort}`,
        ...(fork ? { setup: [evaluate(`fetch('/${tag}-proxy/setup').then((r) => r.text())`)] } : {}),
      });
      await using page = await lp.page({ as: 'proxied' });
      const r = await page.run([evaluate('document.querySelector("#ready").textContent', 'ready'), evaluate(`fetch('/${tag}-proxy/fetch').then((r) => r.text())`, 'fetched')]);
      expect(r.values).toEqual({ ready: 'ready', fetched: `ok /${tag}-proxy/fetch` });
      // Without --fork the pool boots the page more than once.
      expect([...new Set(requests(`/${tag}-proxy/`))]).toEqual([
        `/${tag}-proxy/page ${agent} via proxy`,
        ...(fork ? [`/${tag}-proxy/setup ${agent} via proxy`] : []),
        `/${tag}-proxy/fetch ${agent} via proxy`,
      ]);
    } finally {
      lp.close();
    }
  }
}, 30000);

test('without --fork a profile sends its userAgent too, from booted and fresh pages', async () => {
  const lp = await Pyro.launch({ bin });
  const agent = 'ThreadAgent/3.0';
  try {
    await lp.profile('booted', { url: `${origin()}/thread/page`, userAgent: agent, pool: 1 });
    await lp.profile('fresh', { url: `${origin()}/thread-fresh/page`, userAgent: agent, prepare: false });
    for (const as of ['booted', 'fresh']) {
      await using page = await lp.page({ as });
      const r = await page.run([evaluate('navigator.userAgent', 'agent'), evaluate(`fetch('/thread/${as}-fetch').then((r) => r.text())`, 'fetched')]);
      expect(r.values).toEqual({ agent, fetched: `ok /thread/${as}-fetch` });
    }
    expect(requests('/thread/booted-fetch')).toEqual([`/thread/booted-fetch ${agent}`]);
    expect(requests('/thread/fresh-fetch')).toEqual([`/thread/fresh-fetch ${agent}`]);
    expect(requests('/thread-fresh/page')).toEqual([`/thread-fresh/page ${agent}`]);
    expect(requests('/thread/page').every((line) => line === `/thread/page ${agent}`)).toBe(true);
  } finally {
    lp.close();
  }
}, 30000);

test("what layout, screenshots and PDFs load for a profile's pages goes through its proxy with its userAgent", async () => {
  for (const fork of [true, false]) {
    const lp = await Pyro.launch({ bin, args: fork ? ['--fork'] : [] });
    const tag = fork ? 'fork-assets' : 'thread-assets';
    const agent = `Assets/1.0 (${tag})`;
    const dir = `/${tag}/`;
    try {
      await lp.profile('assets', { url: `http://site.test${dir}assets-page`, userAgent: agent, proxyServer: `http://127.0.0.1:${proxyPort}` });
      await using page = await lp.page({ as: 'assets' });
      await layOutUntil(page, ['bg.png', 'imported.css', 'imported-bg.png', 'font.ttf', 'img.png'].map((name) => dir + name));
      // A screenshot paints the images the document's layout loaded, without loading them again (as Chrome:
      // Chrome 158 makes no second request for a page's CSS background when Page.captureScreenshot paints it).
      const before = requests(`${dir}bg.png `).length;
      expect((await page.send('Page.captureScreenshot', {})).data.length).toBeGreaterThan(0);
      expect(requests(`${dir}bg.png `).length).toBe(before);
      // A PDF of a document nothing laid out yet: its layout loads what it prints first.
      await page.run([goto(`http://site.test${dir}pdf-assets-page`)]);
      expect((await page.send('Page.printToPDF', {})).data.length).toBeGreaterThan(0);
      for (const name of ['pdf-bg.png', 'pdf-font.ttf', 'pdf-imported.css']) expect(requests(`${dir}${name} `)).not.toEqual([]);
      // `site.test` resolves nowhere: what reached the site came through the proxy, with the profile's agent.
      expect(requests(dir).filter((line) => !line.endsWith(` ${agent} via proxy`))).toEqual([]);

      // A page of no profile loads such a page straight from the site, with Pyro's own agent.
      const plain = `/${tag}-plain/`;
      await using direct = await lp.page({ url: `${origin()}${plain}assets-page` });
      await layOutUntil(direct, ['bg.png', 'imported.css', 'imported-bg.png', 'font.ttf'].map((name) => plain + name));
      expect(requests(plain).filter((line) => line.endsWith(' via proxy') || !line.includes(' Mozilla/5.0 (compatible; pyro/'))).toEqual([]);
    } finally {
      lp.close();
    }
  }
}, 60000);
