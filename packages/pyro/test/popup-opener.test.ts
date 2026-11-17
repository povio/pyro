// The object `window.open` returns: `popup.opener = null` (strict code, as
// apps cut the new tab off before navigating it) must not throw, and the
// popup's own `window.opener` reads null afterwards. Danex D4's "Open" did
// `const tab = window.open('', '_blank'); await signedUrl(); tab.opener = null;
// tab.location.href = url` and showed "The file couldn't be opened".
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, css, evaluate, popup, switchTo } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

const app = `<!doctype html><button id=open>Open</button><p id=out></p><script type=module>
  document.getElementById("open").onclick = async () => {
    const tab = window.open('', '_blank');
    try {
      const signed = await fetch('/api/url').then((r) => r.json());
      tab.opener = null;
      tab.location.href = signed.url;
      out.textContent = 'opened ' + String(tab.opener);
    } catch (e) {
      tab?.close();
      out.textContent = 'failed: ' + e;
    }
  };
</script>`;

let site: ReturnType<typeof Bun.serve>;
beforeAll(() => {
  site = Bun.serve({
    port: 0,
    async fetch(req) {
      const path = new URL(req.url).pathname;
      if (path === '/api/url') {
        await Bun.sleep(Number(new URL(req.url).searchParams.get('ms') ?? 50));
        return Response.json({ url: '/doc' });
      }
      if (path === '/doc') return new Response('<!doctype html><title>doc</title><p>document</p>', { headers: { 'content-type': 'text/html' } });
      return new Response(app, { headers: { 'content-type': 'text/html' } });
    },
  });
});
afterAll(() => site.stop(true));

test('popup.opener = null before navigating the popup', async () => {
  const lp = await Pyro.launch({ bin });
  try {
    await using page = await lp.page({ url: `http://127.0.0.1:${site.port}/` });
    const r = await page.run([
      css('#open').click(),
      popup('tab'),
      evaluate('new Promise((r) => { const t = setInterval(() => { if (document.querySelector("#out").textContent) { clearInterval(t); r(document.querySelector("#out").textContent); } }, 10); })', 'out'),
      switchTo('tab'),
      evaluate('new Promise((r) => { const t = setInterval(() => { if (location.pathname === "/doc" && document.readyState === "complete") { clearInterval(t); r([location.pathname, document.title, String(window.opener)]); } }, 10); })', 'tab'),
    ]);
    expect(r.values.out).toBe('opened null');
    expect(r.values.tab).toEqual(['/doc', 'doc', 'null']);
  } finally {
    lp.close();
  }
}, 30000);
