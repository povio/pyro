// `check.request(...).responseBody(as)`: a run hands out the body of the
// response a request step matched.
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, check, css, evaluate, goto } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

let site: ReturnType<typeof Bun.serve>;
beforeAll(() => {
  site = Bun.serve({
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      if (path === '/api/item') return Response.json({ id: 7, name: 'Zinc' });
      if (path === '/api/save') return Response.json({ saved: true, echo: req.method });
      if (path === '/bin') return new Response(new Uint8Array([0, 1, 2, 255]), { headers: { 'content-type': 'application/octet-stream' } });
      return new Response('<!doctype html><button onclick="fetch(\'/api/save\', { method: \'POST\', body: \'{}\' })">Save</button>', { headers: { 'content-type': 'text/html' } });
    },
  });
});
afterAll(() => site.stop(true));
const url = (path = '/') => `http://127.0.0.1:${site.port}${path}`;

test('a request check hands out the matched response body', async () => {
  const lp = await Pyro.launch({ bin });
  try {
    await using page = await lp.page({ url: url() });
    const r = await page.run([
      css('button').click(),
      check.request('POST', '/api/save').status(200).responseBody('saved'),
      evaluate('fetch("/api/item").then((r) => r.status)'),
      evaluate('fetch("/bin").then((r) => r.status)'),
      check.request('GET', '/api/item').responseBody(),
      check.request('GET', '/bin').responseBody('bytes'),
    ]);
    expect(JSON.parse(r.values.saved as string)).toEqual({ saved: true, echo: 'POST' });
    const item = r.results[4] as { status: number; responseBody: string };
    expect(item.status).toBe(200);
    expect(JSON.parse(item.responseBody)).toEqual({ id: 7, name: 'Zinc' });
    expect(Buffer.from(r.values.bytes as string, 'base64')).toEqual(Buffer.from([0, 1, 2, 255]));
    expect((r.results[5] as { responseBodyBase64?: boolean }).responseBodyBase64).toBe(true);
    // Without `responseBody()` a check's result carries no body, and the run's request list never does.
    const plain = await page.run([evaluate('fetch("/api/item").then((r) => r.text())'), check.request('GET', '/api/item')]);
    expect((plain.results[1] as Record<string, unknown>).responseBody).toBeUndefined();
    expect(plain.requests.every((q) => !('responseBody' in q))).toBe(true);
    // `as` without a body: the matched request.
    const asIs = await page.run([evaluate('fetch("/api/item")'), check.request('GET', '/api/item').as('req'), goto(url())]);
    expect((asIs.values.req as { status: number }).status).toBe(200);
  } finally {
    lp.close();
  }
}, 30000);
