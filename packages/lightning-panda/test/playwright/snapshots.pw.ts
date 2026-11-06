// Snapshots: toMatchSnapshot on strings and PNGs, ariaSnapshot() and
// toMatchAriaSnapshot(). Also passes on stock Playwright (the parity run),
// which checks our ARIA tree against Playwright's own on the same page.
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { expect, test } from './fixtures';

const PAGE = `
  <header><h1>Shop</h1></header>
  <nav aria-label="Main"><ul><li><a href="/home">Home</a></li><li><a href="/about">About us</a></li></ul></nav>
  <main>
    <h2>Basket</h2>
    <p>Two items, <b>12 EUR</b> in total.</p>
    <label>Coupon <input placeholder="CODE" value="SPRING"></label>
    <label><input type="checkbox" checked> Gift wrap</label>
    <button disabled>Pay</button>
    <button aria-pressed="true">Favourite</button>
    <button aria-expanded="false">More</button>
    <div hidden>secret</div><span aria-hidden="true">decor</span>
  </main>`;

test.describe('aria snapshots', () => {
  test('ariaSnapshot renders the tree as Playwright does', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    expect(await page.locator('nav').ariaSnapshot()).toBe(
      ['- navigation "Main":', '  - list:', '    - listitem:', '      - link "Home":', '        - /url: /home', '    - listitem:', '      - link "About us":', '        - /url: /about'].join('\n'),
    );
    expect(await page.locator('main').ariaSnapshot()).toBe(
      [
        '- main:',
        '  - heading "Basket" [level=2]',
        '  - paragraph: Two items, 12 EUR in total.',
        '  - text: Coupon',
        '  - textbox "Coupon":',
        '    - /placeholder: CODE',
        '    - text: SPRING',
        '  - checkbox "Gift wrap" [checked]',
        '  - text: Gift wrap',
        '  - button "Pay" [disabled]',
        '  - button "Favourite" [pressed]',
        '  - button "More"',
      ].join('\n'),
    );
    expect(await page.locator('main').ariaSnapshot({ depth: 1 })).toContain('  - textbox "Coupon":\n    - /placeholder: CODE\n  - checkbox');
    expect(await page.ariaSnapshot()).toContain('- banner:\n  - heading "Shop" [level=1]');
  });

  test('toMatchAriaSnapshot matches partial templates', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    await expect(page.locator('body')).toMatchAriaSnapshot(`
      - navigation:
        - list:
          - listitem:
            - link "Home"
          - listitem:
            - link /About/
      - main:
        - button "Pay" [disabled]
    `);
    await expect(page.locator('main')).toMatchAriaSnapshot(`
      - paragraph: /\\d+ EUR/
      - checkbox [checked=true]
    `);
    await expect(page.locator('nav')).toMatchAriaSnapshot(`
      - list:
        - /children: equal
        - listitem
        - listitem
    `);
    await expect(page.locator('nav')).not.toMatchAriaSnapshot(`
      - list:
        - /children: equal
        - listitem
    `);
    await expect(page.locator('main')).not.toMatchAriaSnapshot('- button "Pay" [disabled=false]');
    await expect(page.locator('main')).not.toMatchAriaSnapshot('- link "Home"');
  });

  test('toMatchAriaSnapshot waits for the page to match', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(`<ul id="l"></ul><script>setTimeout(() => { l.innerHTML = '<li>one</li><li>two</li>'; }, 150);</script>`);
    await expect(page.locator('#l')).toMatchAriaSnapshot(`
      - list:
        - listitem: one
        - listitem: two
    `);
  });

  test('a failing template reports the received tree', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    const e = await expect(page.locator('nav')).toMatchAriaSnapshot('- link "Contact"', { timeout: 300 }).catch((e: Error) => e);
    expect(e).toBeInstanceOf(Error);
    expect((e as Error).message).toContain('link "About us"');
  });

  test('toMatchAriaSnapshot without a template uses the .aria.yml file', async ({ page }, info) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    const file = info.snapshotPath('nav.aria.yml', { kind: 'aria' });
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, '- navigation "Main":\n  - list:\n    - listitem:\n      - link "About us"\n');
    try {
      await expect(page.locator('nav')).toMatchAriaSnapshot({ name: 'nav.aria.yml' });
    } finally {
      rmSync(file, { force: true });
    }
  });

  test('options of a select are in the tree; a drop-down\'s optgroups are not', async ({ page }) => {
    // Playwright exempts an option of a <select> from the box check, but
    // still drops one whose computed display is none: our engine reports
    // every option as display:none (Chromium says block), so stock
    // Playwright on it shows no options at all.
    test.skip(!!process.env.PW_ENGINE, "lightningpanda's computed display of an <option> differs from Chromium's");
    await page.goto('/simple');
    await page.setContent(`<select aria-label="Size"><option>S</option><option selected>M</option><optgroup label="G"><option>L</option></optgroup></select>
      <select aria-label="Pick" multiple><optgroup label="G"><option>A</option></optgroup><option>B</option></select>`);
    expect(await page.locator('select').first().ariaSnapshot()).toBe('- combobox "Size":\n  - option "S"\n  - option "M" [selected]');
    expect(await page.locator('select').nth(1).ariaSnapshot()).toBe('- listbox "Pick":\n  - group:\n    - option "A"\n  - option "B"');
  });

  test('hidden, unslotted and owned elements as Playwright walks them', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(`<div aria-hidden="true"><nav aria-label="Hidden"><a href="/x">x</a></nav></div>
      <div id="host"><b>light</b><i slot="s">slotted</i></div>
      <div id="owner" role="list" aria-owns="two"><div role="listitem">one</div><div role="listitem" id="two">two</div></div>
      <script>host.attachShadow({ mode: 'open' }).innerHTML = '<p>shadow</p><slot name="s"></slot>';</script>`);
    expect(await page.locator('nav').ariaSnapshot()).toBe('');
    expect(await page.locator('#host').ariaSnapshot()).toBe('- paragraph: shadow\n- text: slotted');
    expect(await page.locator('#owner').ariaSnapshot()).toBe('- list:\n  - listitem: one\n  - listitem: two');
  });

  test('explicit roles: the first valid token, presentation yields to focus and global aria', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(`<div id="r">
      <div role="foo button">Act</div><div role="bogus">plain</div>
      <button role="presentation">Go</button><h3 role="none" aria-describedby="r">Title</h3><h4 role="none">Flat</h4>
      <div role="heading" aria-level="3.0">Lvl</div>
      <details open><summary>More</summary>body</details>
      <span role="generic" style="display:inline-block">gen</span><div role="generic"></div>
      <label for="m">Fuel</label><meter id="m" title="Level" value="0.5"></meter>
    </div>`);
    expect(await page.locator('#r').ariaSnapshot()).toBe(
      [
        '- button "Act"',
        '- text: plain',
        '- button "Go"',
        '- heading "Title" [level=3]',
        '- text: Flat',
        '- heading "Lvl" [level=3]',
        '- group: More body',
        '- generic: gen',
        '- text: Fuel',
        '- meter "Level"',
      ].join('\n'),
    );
    await expect(page.getByRole('button', { name: 'Go' })).toHaveCount(1);
    await expect(page.getByRole('heading', { name: 'Title' })).toHaveCount(1);
    await expect(page.getByRole('meter', { name: 'Level' })).toHaveCount(1);
    await expect(page.getByRole('meter', { name: 'Fuel' })).toHaveCount(0);
  });

  test('templates take the YAML forms Playwright parses', async ({ page }) => {
    await page.goto('/simple');
    await page.setContent(PAGE);
    const forms = [
      '---\n- paragraph: |\n    Two items, 12 EUR\n    in total.\n...',
      '- paragraph: >-\n    Two items,\n    12 EUR in total.',
      '- paragraph: Two items,\n    12 EUR in total.',
      '- paragraph: "Two items,\n    12 EUR in total."',
      '- {textbox: SPRING}',
      '-\n  textbox "Coupon":\n    - /placeholder: CODE',
    ];
    for (const form of forms) await expect(page.locator('main'), form).toMatchAriaSnapshot(form);
    await expect(page.locator('nav')).toMatchAriaSnapshot(`
      - list: [listitem, listitem]
    `);
    await expect(page.locator('nav')).toMatchAriaSnapshot(`
      - list:
        - /children: deep-equal
        - listitem:
          - /children: contain
        - listitem:
          - /children: contain
    `);
    // Also a quoted scalar whose next line is not indented past its key.
    for (const bad of ['- text: 42', '- text: true', '- paragraph:', '- paragraph: null', '- paragraph: a: b', '- 42', '- paragraph: "two\n  lines"', "- text: 'a\n b'"])
      expect(await expect(page.locator('main')).toMatchAriaSnapshot(bad, { timeout: 200 }).then(() => undefined, (e: Error) => e), bad).toBeInstanceOf(Error);
  });

  test('-u writes the regex rendering: "all" always, "changed" only on a mismatch, never under .not', async ({ page }, info) => {
    test.skip(!!process.env.PW_ENGINE, 'stock Playwright cannot switch the update mode inside a test');
    await page.goto('/simple');
    await page.setContent('<ul><li>Issue 42</li></ul>');
    const file = info.snapshotPath('update.aria.yml', { kind: 'aria' });
    mkdirSync(dirname(file), { recursive: true });
    const before = process.env.LP_UPDATE_SNAPSHOTS;
    try {
      writeFileSync(file, '- list');
      process.env.LP_UPDATE_SNAPSHOTS = 'all';
      await expect(page.locator('ul')).toMatchAriaSnapshot({ name: 'update.aria.yml' });
      expect(readFileSync(file, 'utf8')).toBe('- list:\n  - listitem: /Issue \\d+/');
      writeFileSync(file, '- list');
      process.env.LP_UPDATE_SNAPSHOTS = 'changed';
      await expect(page.locator('ul')).toMatchAriaSnapshot({ name: 'update.aria.yml' });
      expect(readFileSync(file, 'utf8')).toBe('- list', 'a matching snapshot is kept');
      writeFileSync(file, '- button');
      await expect(page.locator('ul')).not.toMatchAriaSnapshot({ name: 'update.aria.yml' });
      expect(readFileSync(file, 'utf8')).toBe('- button', '.not never writes');
      await expect(page.locator('ul')).toMatchAriaSnapshot({ name: 'update.aria.yml', timeout: 300 });
      expect(readFileSync(file, 'utf8')).toBe('- list:\n  - listitem: /Issue \\d+/');
    } finally {
      if (before === undefined) delete process.env.LP_UPDATE_SNAPSHOTS;
      else process.env.LP_UPDATE_SNAPSHOTS = before;
      rmSync(file, { force: true });
    }
  });
});

test.describe('toMatchSnapshot', () => {
  test('strings and buffers against their snapshot file', async ({ page }, info) => {
    const text = info.snapshotPath('greeting.txt');
    mkdirSync(dirname(text), { recursive: true });
    writeFileSync(text, 'hello\nworld');
    try {
      expect('hello\nworld').toMatchSnapshot('greeting.txt');
      expect(() => expect('hello\nthere').toMatchSnapshot('greeting.txt')).toThrow(/greeting/);
      expect('other').not.toMatchSnapshot('greeting.txt');
    } finally {
      rmSync(text, { force: true });
    }
    await page.goto('/simple');
    await page.setContent('<div style="width:40px;height:20px;background:rgb(200,0,0)"></div>');
    const png = await page.locator('div').screenshot();
    const shot = info.snapshotPath('box.png');
    writeFileSync(shot, png);
    try {
      expect(png).toMatchSnapshot('box.png');
      await page.setContent('<div style="width:40px;height:20px;background:rgb(200,0,0)"><i style="display:block;width:4px;height:4px;background:#00f"></i></div>');
      const changed = await page.locator('div').screenshot();
      expect(() => expect(changed).toMatchSnapshot('box.png')).toThrow(/pixels/);
      expect(changed).toMatchSnapshot('box.png', { maxDiffPixels: 16 });
    } finally {
      rmSync(shot, { force: true });
    }
  });

  test('a missing snapshot is written and fails the test at its end', async ({}, info) => {
    test.fail();
    const file = info.snapshotPath('fresh.txt');
    rmSync(file, { force: true });
    try {
      expect('new').toMatchSnapshot('fresh.txt');
      expect(existsSync(file)).toBe(true);
    } finally {
      rmSync(file, { force: true });
    }
  });
});

// Snapshot paths, names and comparison as Playwright's testInfo and comparators do them.
import { spawnSync as spSpawn } from 'node:child_process';
import { createHash as spHash } from 'node:crypto';
import { existsSync as spExists, mkdirSync as spMkdir, mkdtempSync as spMkdtemp, readFileSync as spRead, rmSync as spRm, statSync as spStat, writeFileSync as spWrite } from 'node:fs';
import { tmpdir as spTmpdir } from 'node:os';
import { dirname as spDirname, join as spJoin } from 'node:path';
import { deflateSync as spDeflate } from 'node:zlib';

/** A PNG from rows of samples (filter 0), optionally Adam7-interlaced. */
function encodePng(width: number, height: number, depth: number, type: number, sample: (x: number, y: number) => number[], o: { interlace?: boolean; plte?: number[]; trns?: number[] } = {}): Buffer {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (b: Buffer) => {
    let c = 0xffffffff;
    for (const x of b) c = crcTable[(c ^ x) & 255] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (t: string, body: Buffer) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(body.length);
    const tb = Buffer.concat([Buffer.from(t, 'latin1'), body]);
    const c = Buffer.alloc(4);
    c.writeUInt32BE(crc(tb));
    return Buffer.concat([len, tb, c]);
  };
  const row = (xs: number[], y: number) => {
    const bits: number[] = [];
    for (const x of xs) for (const v of sample(x, y)) depth === 16 ? bits.push(v >> 8, v & 255) : bits.push(v);
    if (depth >= 8) return [0, ...bits];
    const out: number[] = [0];
    for (let i = 0; i < bits.length; i += 8 / depth) {
      let byte = 0;
      for (let j = 0; j < 8 / depth; j++) byte |= (bits[i + j] ?? 0) << (8 - depth * (j + 1));
      out.push(byte);
    }
    return out;
  };
  const raw: number[] = [];
  const passes = o.interlace ? [[0, 0, 8, 8], [4, 0, 8, 8], [0, 4, 4, 8], [2, 0, 4, 4], [0, 2, 2, 4], [1, 0, 2, 2], [0, 1, 1, 2]] : [[0, 0, 1, 1]];
  for (const [x0, y0, dx, dy] of passes) {
    const xs: number[] = [];
    for (let x = x0; x < width; x += dx) xs.push(x);
    if (!xs.length) continue;
    for (let y = y0; y < height; y += dy) raw.push(...row(xs, y));
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr.set([depth, type, 0, 0, o.interlace ? 1 : 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    ...(o.plte ? [chunk('PLTE', Buffer.from(o.plte))] : []),
    ...(o.trns ? [chunk('tRNS', Buffer.from(o.trns))] : []),
    chunk('IDAT', spDeflate(Buffer.from(raw))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

test.describe('snapshot paths', () => {
  test('names are sanitized as Playwright does: a string before its extension, an array not at all', async ({ page: _page }, info) => {
    const dir = `${info.file}-snapshots`;
    const suffix = `-${process.platform}`;
    expect(info.snapshotDir).toBe(dir);
    expect(info.snapshotSuffix).toBe(process.platform);
    expect(info.snapshotPath('a b/c d.txt')).toBe(spJoin(dir, `a-b-c-d${suffix}.txt`));
    expect(info.snapshotPath('a b', 'c d.txt')).toBe(spJoin(dir, 'a b', `c d${suffix}.txt`));
    expect(info.snapshotPath('shot one.png', { kind: 'screenshot' })).toBe(spJoin(dir, `shot-one${suffix}.png`));
    // ARIA: no project or suffix in the default template, `.aria.yml` is the whole extension.
    expect(info.snapshotPath('main nav.aria.yml', { kind: 'aria' })).toBe(spJoin(dir, 'main-nav.aria.yml'));
    info.snapshotSuffix = '';
    expect(info.snapshotPath('plain.txt')).toBe(spJoin(dir, 'plain.txt'));
    info.snapshotSuffix = process.platform;
  });

  test('anonymous snapshots are named <titles> <n>, trimmed and sanitized', async ({ page: _page }, info) => {
    const file = spJoin(`${info.file}-snapshots`, `snapshot-paths-anonymous-snapshots-are-named-titles-n-trimmed-and-sanitized-1-${process.platform}.txt`);
    expect(info.snapshotPath()).toBe(file);
    spMkdir(spDirname(file), { recursive: true });
    spWrite(file, 'anon');
    try {
      expect('anon').toMatchSnapshot();
    } finally {
      spRm(file, { force: true });
    }
  });

  test(`a long title is trimmed to 100 characters with a hash of it in the middle ${'x'.repeat(80)}`, async ({ page: _page }, info) => {
    const full = [...info.titlePath.slice(1), 1].join(' ');
    const middle = `-${spHash('sha1').update(full).digest('hex').slice(0, 5)}-`;
    const start = Math.floor((100 - middle.length) / 2);
    const trimmed = full.slice(0, start) + middle + full.slice(-(100 - middle.length - start));
    const name = trimmed.replace(/[\x00-\x2C\x2E-\x2F\x3A-\x40\x5B-\x60\x7B-\x7F]+/g, '-');
    expect(info.snapshotPath()).toBe(spJoin(`${info.file}-snapshots`, `${name}-${process.platform}.txt`));
  });

  test('maxDiffPixels and maxDiffPixelRatio together allow the smaller', async ({ page }, info) => {
    await page.goto('/simple');
    await page.setContent('<div style="width:40px;height:20px;background:rgb(200,0,0)"></div>');
    const file = info.snapshotPath('limits.png');
    spMkdir(spDirname(file), { recursive: true });
    spWrite(file, await page.locator('div').screenshot());
    try {
      await page.setContent('<div style="width:40px;height:20px;background:rgb(200,0,0)"><i style="display:block;width:4px;height:4px;background:#00f"></i></div>');
      const changed = await page.locator('div').screenshot();
      // 16 pixels differ; 0.01 of 800 allows 8.
      expect(() => expect(changed).toMatchSnapshot('limits.png', { maxDiffPixels: 100, maxDiffPixelRatio: 0.01 })).toThrow(/pixels/);
      expect(changed).toMatchSnapshot('limits.png', { maxDiffPixels: 100, maxDiffPixelRatio: 0.05 });
      expect(changed).toMatchSnapshot('limits.png', { maxDiffPixelRatio: 0.05 });
    } finally {
      spRm(file, { force: true });
    }
  });

  test('every PNG variant compares by pixel: 16-bit, sub-8-bit gray and palette, interlaced', async ({ page: _page }, info) => {
    const w = 11, h = 9;
    const on = (x: number, y: number) => (x + y) % 3 === 0;
    const rgba8 = encodePng(w, h, 8, 6, (x, y) => (on(x, y) ? [255, 255, 255, 255] : [0, 0, 0, 255]));
    const variants: Record<string, Buffer> = {
      rgb16: encodePng(w, h, 16, 2, (x, y) => (on(x, y) ? [65535, 65535, 65535] : [0, 0, 0])),
      gray1: encodePng(w, h, 1, 0, (x, y) => [on(x, y) ? 1 : 0]),
      gray4: encodePng(w, h, 4, 0, (x, y) => [on(x, y) ? 15 : 0]),
      palette2: encodePng(w, h, 2, 3, (x, y) => [on(x, y) ? 1 : 0], { plte: [0, 0, 0, 255, 255, 255] }),
      interlaced: encodePng(w, h, 8, 6, (x, y) => (on(x, y) ? [255, 255, 255, 255] : [0, 0, 0, 255]), { interlace: true }),
      interlaced16: encodePng(w, h, 16, 4, (x, y) => (on(x, y) ? [65535, 65535] : [0, 65535]), { interlace: true }),
    };
    const file = info.snapshotPath('variants.png');
    spMkdir(spDirname(file), { recursive: true });
    spWrite(file, rgba8);
    try {
      for (const [name, png] of Object.entries(variants)) {
        expect(png.equals(rgba8), name).toBe(false);
        expect(png, name).toMatchSnapshot('variants.png');
      }
      const inverted = encodePng(w, h, 1, 0, (x, y) => [on(x, y) ? 0 : 1], { interlace: true });
      expect(() => expect(inverted).toMatchSnapshot('variants.png')).toThrow(/pixels/);
    } finally {
      spRm(file, { force: true });
    }
  });

  test('an anonymous PNG snapshot takes .png from the magic bytes', async ({ page: _page }, info) => {
    const png = encodePng(3, 3, 16, 0, () => [40000]);
    const file = spJoin(`${info.file}-snapshots`, `snapshot-paths-an-anonymous-PNG-snapshot-takes-png-from-the-magic-bytes-1-${process.platform}.png`);
    spMkdir(spDirname(file), { recursive: true });
    spWrite(file, png);
    try {
      expect(png).toMatchSnapshot();
    } finally {
      spRm(file, { force: true });
    }
  });

  test('toHaveScreenshot keeps its baseline at the screenshot snapshot path', async ({ page }, info) => {
    await page.goto('/simple');
    await page.setContent('<div style="width:40px;height:20px;background:rgb(0,120,0)"></div>');
    const file = info.snapshotPath('green box.png', { kind: 'screenshot' });
    spMkdir(spDirname(file), { recursive: true });
    spWrite(file, await page.locator('div').screenshot());
    try {
      await expect(page.locator('div')).toHaveScreenshot('green box.png');
      await page.setContent('<div style="width:40px;height:20px;background:rgb(0,0,200)"></div>');
      const e = await expect(page.locator('div')).toHaveScreenshot('green box.png', { timeout: 1000 }).catch((e: Error) => e);
      expect(e).toBeInstanceOf(Error);
      // As Playwright: the images go to the test's output dir, named after the snapshot.
      // The name's second call in this test: `-1`.
      expect(spExists(info.outputPath('green-box-1-actual.png'))).toBe(true);
      expect(spExists(info.outputPath('green-box-1-diff.png'))).toBe(true);
      expect(spExists(info.outputPath('green-box-1-expected.png'))).toBe(true);
      expect(spExists(file.replace(/\.png$/, '.actual.png'))).toBe(false);
      const named = await expect(page).toHaveScreenshot('no-extension').catch((e: Error) => e);
      expect((named as Error).message).toContain(`must have '.png' extension`);
    } finally {
      spRm(file, { force: true });
      for (const ext of ['.json', '.actual.png', '.diff.png']) spRm(file.replace(/\.png$/, ext), { force: true });
    }
  });

  test('a written missing snapshot is not retried; "changed" rewrites only what differs', async () => {
    test.skip(!!process.env.PW_ENGINE, 'runs our CLI on a generated project');
    test.setTimeout(120_000);
    // Outside the specs' testDir, so no runner picks the generated spec up.
    const dir = spMkdtemp(spJoin(spTmpdir(), 'lp-snapshots-'));
    try {
      const src = spJoin(import.meta.dirname, '../../src');
      spWrite(spJoin(dir, 'playwright.config.ts'), `export default { testDir: '.', testMatch: '*.pw.ts', workers: 1 };\n`);
      spWrite(
        spJoin(dir, 'x.pw.ts'),
        `import { expect, test } from ${JSON.stringify(spJoin(src, 'playwright/index'))};
  test.describe.configure({ retries: 2 });
  test('missing', async ({}, info) => {
    console.log('ATTEMPT ' + info.retry);
    expect(process.env.TEXT ?? 'one').toMatchSnapshot('m.txt');
  });
  test('shot', async ({ page }) => {
    await page.setContent('<div style="width:40px;height:20px;background:' + (process.env.COLOR ?? 'red') + '"></div>');
    await expect(page.locator('div')).toHaveScreenshot('c.png');
  });
  `,
      );
      const run = (env: Record<string, string>) =>
        spSpawn('bun', [spJoin(src, 'cli.ts'), 'test', '-c', spJoin(dir, 'playwright.config.ts')], { cwd: dir, env: { ...process.env, LP_RESULTS: '', ...env }, encoding: 'utf8' });
      const snaps = spJoin(dir, 'x.pw.ts-snapshots');
      const txt = spJoin(snaps, `m-${process.platform}.txt`), png = spJoin(snaps, `c-${process.platform}.png`);

      const first = run({});
      const out = first.stdout + first.stderr;
      expect(first.status, out).not.toBe(0);
      expect(out).toContain('ATTEMPT 0');
      expect(out).not.toContain('ATTEMPT 1');
      expect(spRead(txt, 'utf8')).toBe('one');
      expect(spExists(png), out).toBe(true);

      // "changed": a matching baseline is left alone ...
      const before = spRead(png);
      const mtime = spStat(png).mtimeMs;
      const same = run({ LP_UPDATE_SNAPSHOTS: 'changed' });
      expect(same.status, same.stdout + same.stderr).toBe(0);
      expect(spStat(png).mtimeMs).toBe(mtime);
      // ... and a mismatching one is rewritten, and passes.
      const changed = run({ LP_UPDATE_SNAPSHOTS: 'changed', COLOR: 'blue', TEXT: 'two' });
      expect(changed.status, changed.stdout + changed.stderr).toBe(0);
      expect(spRead(png).equals(before)).toBe(false);
      expect(spRead(txt, 'utf8')).toBe('two');
      expect(run({ COLOR: 'blue', TEXT: 'two' }).status).toBe(0);
    } finally {
      spRm(dir, { recursive: true, force: true });
    }
  });
});
