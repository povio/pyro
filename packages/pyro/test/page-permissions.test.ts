// `Page.grantPermissions` / `resetPermissions`: a page's browser context
// permissions, set on the page's own connection. `Page.send` always adds the
// page's session id, and the engine takes `Browser.*` permission commands only
// at the browser level, so `page.send('Browser.grantPermissions', ...)` is
// "not supported" (SloSki's integration-token clipboard test hit this).
import { afterAll, beforeAll, expect, test } from 'bun:test';
import { join } from 'node:path';
import { Pyro, evaluate, type Page } from '../src/index';

const bin = process.env.PYRO_BIN ?? join(import.meta.dir, '../../../target/debug/pyro');

let site: ReturnType<typeof Bun.serve>;
let origin: string;
let browser: Pyro;
beforeAll(async () => {
  site = Bun.serve({
    port: 0,
    hostname: '127.0.0.1',
    fetch: () => new Response('<!doctype html><title>permissions</title><p>permissions</p>', { headers: { 'content-type': 'text/html' } }),
  });
  origin = `http://127.0.0.1:${site.port}`;
  browser = await Pyro.launch({ bin, args: ['--fork'] });
  await browser.profile('app', { url: `${origin}/`, setup: [] });
});
afterAll(() => {
  browser?.close();
  site?.stop(true);
});

async function state(page: Page, name: string): Promise<string> {
  const r = await page.run([evaluate(`navigator.permissions.query({ name: ${JSON.stringify(name)} }).then((p) => p.state)`, 'state')]);
  return r.values.state as string;
}

async function write(page: Page): Promise<string> {
  const r = await page.run([evaluate(`navigator.clipboard.writeText('permission-probe').then(() => 'written', (e) => e.name)`, 'write')]);
  return r.values.write as string;
}

test('a page session command cannot set permissions: the SDK sets them on the browser level', async () => {
  await using page = await browser.page({ as: 'app' });
  await expect(page.send('Browser.grantPermissions', { browserContextId: page.browserContextId, origin, permissions: [] })).rejects.toThrow(
    /not supported/,
  );
});

test('grantPermissions grants and denies navigator.permissions and clipboard writes', async () => {
  await using page = await browser.page({ as: 'app' });
  await page.grantPermissions(['clipboardSanitizedWrite'], { origin });
  expect(await state(page, 'clipboard-write')).toBe('granted');
  expect(await write(page)).toBe('written');

  await page.grantPermissions([], { origin });
  expect(await state(page, 'clipboard-write')).toBe('denied');
  expect(await write(page)).not.toBe('written');

  await page.grantPermissions(['clipboardSanitizedWrite'], { origin });
  expect(await state(page, 'clipboard-write')).toBe('granted');
  expect(await write(page)).toBe('written');
});

test('a grant covers the permission it names, not the other clipboard direction', async () => {
  await using page = await browser.page({ as: 'app' });
  await page.grantPermissions(['clipboardSanitizedWrite'], { origin });
  expect(await state(page, 'clipboard-write')).toBe('granted');
  expect(await state(page, 'clipboard-read')).not.toBe('granted');
  await page.grantPermissions(['clipboardReadWrite'], { origin });
  expect(await state(page, 'clipboard-read')).toBe('granted');
});

test('resetPermissions drops a grant', async () => {
  await using page = await browser.page({ as: 'app' });
  await page.grantPermissions(['geolocation'], { origin });
  expect(await state(page, 'geolocation')).toBe('granted');
  await page.resetPermissions();
  expect(await state(page, 'geolocation')).not.toBe('granted');
});

test('permissions stay in the page that set them', async () => {
  await using first = await browser.page({ as: 'app' });
  await using second = await browser.page({ as: 'app' });
  // Forked pages are separate processes (context ids repeat across them), so
  // isolation is observed, not read off the ids.
  const before = await state(second, 'geolocation');
  expect(before).not.toBe('granted');

  await first.grantPermissions(['geolocation'], { origin });
  expect(await state(first, 'geolocation')).toBe('granted');
  expect(await state(second, 'geolocation')).toBe(before);

  await second.grantPermissions(['geolocation'], { origin });
  await first.resetPermissions();
  expect(await state(first, 'geolocation')).not.toBe('granted');
  expect(await state(second, 'geolocation')).toBe('granted');
});

test('an omitted origin grants all origins while an explicit grant stays scoped', async () => {
  await using page = await browser.page({ as: 'app' });
  await page.grantPermissions(['geolocation'], { origin: 'https://other.example' });
  expect(await state(page, 'geolocation')).not.toBe('granted');
  await page.grantPermissions(['geolocation']);
  expect(await state(page, 'geolocation')).toBe('granted');
  await page.resetPermissions();
  expect(await state(page, 'geolocation')).not.toBe('granted');
});

test('an unknown permission or an opaque origin is rejected', async () => {
  await using page = await browser.page({ as: 'app' });
  await expect(page.grantPermissions(['bogus'], { origin })).rejects.toThrow(/Unknown permission type: bogus/);
  await expect(page.grantPermissions(['geolocation'], { origin: '*' })).rejects.toThrow(/opaque origins/);
});

test('in one shared browser process, pages keep separate context permissions', async () => {
  const shared = await Pyro.launch({ bin });
  try {
    await shared.profile('app', { url: `${origin}/`, setup: [] });
    await using first = await shared.page({ as: 'app' });
    await using second = await shared.page({ as: 'app' });
    expect(first.browserContextId).not.toBe(second.browserContextId);
    await first.grantPermissions(['geolocation'], { origin });
    expect(await state(first, 'geolocation')).toBe('granted');
    expect(await state(second, 'geolocation')).not.toBe('granted');
    await first.resetPermissions();
    expect(await state(first, 'geolocation')).not.toBe('granted');
  } finally {
    shared.close();
  }
});
