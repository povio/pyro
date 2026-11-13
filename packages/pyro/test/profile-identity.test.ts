// A profile's `userAgent` and proxy: its document, its setup's requests and
// every page's requests carry them, in fork mode and without it.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { createServer, type Server } from 'node:net';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

/** Requests the site answered: `path user-agent`, and `via proxy` for those the proxy passed on. */
const seen: string[] = [];
let site: ReturnType<typeof Bun.serve>;
/** A forward proxy for `http://site.test/…`: it passes each request on to `site`. */
let proxy: Server;
let proxyPort = 0;
beforeAll(async () => {
  site = Bun.serve({
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      seen.push(`${path} ${req.headers.get('user-agent')}${req.headers.get('x-via-test-proxy') ? ' via proxy' : ''}`);
      if (path.endsWith('/page')) return new Response('<!doctype html><div id="ready">ready</div>', { headers: { 'content-type': 'text/html' } });
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
