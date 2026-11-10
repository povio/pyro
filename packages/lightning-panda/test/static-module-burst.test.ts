import { expect, test } from 'bun:test';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Lightningpanda, goto, check, css, evaluate } from '../src/index';

test('a cached module graph preserves every request without overflowing observations', async () => {
  const root = await mkdtemp(join(tmpdir(), 'lp-static-observations-'));
  const origin = 'http://static-observations.test';
  const count = 300;
  let browser: Lightningpanda | undefined;
  try {
    await writeFile(join(root, 'blank.html'), '<!doctype html><body>blank</body>');
    const imports: string[] = [];
    for (let index = 0; index < count; index++) {
      const name = `part-${index}.js`;
      await writeFile(join(root, name), 'globalThis.loaded = (globalThis.loaded || 0) + 1;');
      imports.push(`import './${name}';`);
    }
    await writeFile(join(root, 'index.html'), `<!doctype html><body><script type="module">${imports.join('\n')}\ndocument.body.innerHTML='<div id="ready">ready</div>';</script></body>`);
    browser = await Lightningpanda.launch({ args: ['--fork', '--static', `${origin}=${root}`] });
    await browser.profile('blank', { url: `${origin}/blank.html` });
    await using page = await browser.page({ as: 'blank' });
    const result = await page.run([goto(`${origin}/index.html`), check(css('#ready')).visible(), evaluate('globalThis.loaded', 'loaded')], { timeout: 5000 });
    expect(result.values.loaded).toBe(count);
    const modules = result.requests.filter(request => /\/part-\d+\.js$/.test(request.url));
    expect(modules).toHaveLength(count);
    expect(new Set(modules.map(request => request.url)).size).toBe(count);
    for (const request of modules) {
      expect(request.status).toBe(200);
      expect(request.error).toBeNull();
    }
  } finally {
    browser?.close();
    await rm(root, { recursive: true, force: true });
  }
}, 10000);
