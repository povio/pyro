// Measurements see web fonts: the layout engine that answers getBoundingClientRect starts
// loading the faces the styles use when the document is parsed (not at the first query), and
// `document.fonts.ready` resolves only once they are in. Before, a span in a web font measured
// in its fallback font until some later DOM change (axe reported text overlapping the next card
// on Danex's admin/facilities page; Playwright boxes were off).
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Pyro, evaluate } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');
const ahem = readFileSync(join(import.meta.dir, '../../../crates/pyro-render/tests/fonts/Ahem.ttf'));

const app = `<!doctype html><style>
  @font-face { font-family: TestAhem; src: url(/ahem.ttf) format("truetype"); }
  #t { font: 20px TestAhem, serif; white-space: nowrap; }
</style><span id=t>XXXXXXXXXX</span>`;

let site: ReturnType<typeof Bun.serve>;
beforeAll(() => {
  site = Bun.serve({
    port: 0,
    async fetch(req) {
      if (new URL(req.url).pathname === '/ahem.ttf') {
        // Slow enough that a measurement right after load would beat it.
        await Bun.sleep(300);
        return new Response(ahem, { headers: { 'content-type': 'font/ttf' } });
      }
      return new Response(app, { headers: { 'content-type': 'text/html' } });
    },
  });
});
afterAll(() => site.stop(true));

test('a web-font span measures in its font once document.fonts.ready resolves', async () => {
  const lp = await Pyro.launch({ bin });
  try {
    await using page = await lp.page({ url: `http://127.0.0.1:${site.port}/` });
    const r = await page.run([
      evaluate(
        'document.fonts.ready.then(() => [document.fonts.status, document.fonts.check("20px TestAhem"), document.getElementById("t").getBoundingClientRect().width])',
        'ready',
      ),
    ]);
    // Ahem: every glyph is one em wide.
    expect(r.values.ready).toEqual(['loaded', true, 200]);
  } finally {
    lp.close();
  }
}, 30000);
