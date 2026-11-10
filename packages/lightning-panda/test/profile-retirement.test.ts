import { expect, test } from 'bun:test';
import { join } from 'node:path';
import { Lightningpanda, evaluate, check, css } from '../src/index';

const bin = process.env.LIGHTNINGPANDA_BIN ?? join(import.meta.dir, '../../../target/debug/lightningpanda');
const url = 'data:text/html,<div id="ready">ready</div>';

async function descendants(pid: number) {
  const process = Bun.spawn(['ps', '-axo', 'pid=,ppid=,pgid=,stat='], { stdout: 'pipe' });
  const rows = (await new Response(process.stdout).text()).trim().split('\n').map(line => {
    const [id, parent, group, state] = line.trim().split(/\s+/);
    return { pid: Number(id), parent: Number(parent), group: Number(group), zombie: state.includes('Z') };
  });
  const family = new Set([pid]);
  let size;
  do { size = family.size; for (const row of rows) if (family.has(row.parent)) family.add(row.pid); }
  while (family.size !== size);
  return rows.filter(row => row.group === pid || family.has(row.pid));
}
async function waitForBound(pid: number, bound: number) {
  const deadline = performance.now() + 3000;
  let family = await descendants(pid);
  while ((family.length > bound || family.some(row => row.zombie)) && performance.now() < deadline) {
    await Bun.sleep(20);
    family = await descendants(pid);
  }
  expect(family.some(row => row.zombie)).toBe(false);
  expect(family.length).toBeLessThanOrEqual(bound);
}

test('retirement leaves acquired non-fork pages usable and does not recycle another profile', async () => {
  const browser = await Lightningpanda.launch({ bin });
  try {
    await browser.profile('one', { url });
    await browser.profile('two', { url: `${url}<b id="two">two</b>` });
    await using one = await browser.page({ as: 'one' });
    await browser.releaseProfile('one');
    await browser.releaseProfile('one');
    expect((await one.run([evaluate('document.querySelector("#ready").textContent', 'text')])).values.text).toBe('ready');
    await expect(browser.page({ as: 'one' })).rejects.toThrow('no profile one');
    await using two = await browser.page({ as: 'two' });
    expect((await two.run([evaluate('document.querySelector("#two").textContent', 'text')])).values.text).toBe('two');
    await browser.profile('one', { url: `${url}<b id="fresh">fresh</b>` });
    await using fresh = await browser.page({ as: 'one' });
    expect((await fresh.run([evaluate('document.querySelector("#fresh").textContent', 'text')])).values.text).toBe('fresh');
  } finally { browser.close(); }
}, 20000);

test('completed fork profiles retire their templates and spares while active pages and base stay valid', async () => {
  const browser = await Lightningpanda.launch({ bin, args: ['--fork'] });
  const pid = (browser as unknown as { child: { pid: number } }).child.pid;
  try {
    await browser.profile('app', { url, setup: [check(css('#ready')).visible()] });
    for (let i = 0; i < 8; i++) {
      await browser.profile('feature', { base: 'app', url, setup: [evaluate(`window.marker=${i}`)] });
      await using active = await browser.page({ as: 'feature' });
      expect((await active.run([evaluate('window.marker', 'marker')])).values.marker).toBe(i);
      await browser.releaseProfile('feature');
      expect((await active.run([evaluate('window.marker', 'marker')])).values.marker).toBe(i);
      await active[Symbol.asyncDispose]();
      await waitForBound(pid, 5);
    }
    await using base = await browser.page({ as: 'app' });
    expect((await base.run([evaluate('window.marker === undefined', 'pristine')])).values.pristine).toBe(true);
  } finally { browser.close(); }
}, 20000);

test('a released base stays while a profile derived from it is held, then retires with it', async () => {
  const browser = await Lightningpanda.launch({ bin, args: ['--fork'] });
  const pid = (browser as unknown as { child: { pid: number } }).child.pid;
  const names = async () => (await browser.cdp.send('Lightningpanda.preparedProfiles')).profiles.map((p: { name: string; holders: number }) => `${p.name}:${p.holders}`);
  try {
    await browser.profile('app', { url, setup: [check(css('#ready')).visible()] });
    await browser.profile('feature', { base: 'app', url, setup: [evaluate('window.marker="feature"')] });
    await browser.releaseProfile('app');
    expect(await names()).toEqual(['app:0', 'feature:1']);
    await using page = await browser.page({ as: 'feature' });
    expect((await page.run([evaluate('window.marker', 'marker')])).values.marker).toBe('feature');
    await browser.releaseProfile('feature');
    expect(await names()).toEqual([]);
    expect((await page.run([evaluate('window.marker', 'marker')])).values.marker).toBe('feature');
    await page[Symbol.asyncDispose]();
    await waitForBound(pid, 5);
    await expect(browser.page({ as: 'app' })).rejects.toThrow('no profile app');
  } finally { browser.close(); }
}, 20000);

test('an unready derived template is cancellable and a retired parent id cannot be used to derive', async () => {
  const browser = await Lightningpanda.launch({ bin, args: ['--fork'] });
  const pid = (browser as unknown as { child: { pid: number } }).child.pid;
  try {
    await browser.profile('app', { url, setup: [check(css('#ready')).visible()] });
    await browser.profile('pending', { base: 'app', url, setup: [check(css('#never')).visible()] });
    await browser.releaseProfile('pending');
    await waitForBound(pid, 5);
    await browser.releaseProfile('app');
    const retired = await browser.cdp.send('Lightningpanda.unregisterPreparedPage', { profileId: 0 });
    expect(retired.removed).toBe(false);
    await expect(browser.cdp.send('Lightningpanda.registerPreparedPage', { url, base: 0 })).rejects.toThrow('no profile');
  } finally { browser.close(); }
}, 20000);

for (const fork of [false, true]) test(`equivalent profile aliases retain the shared snapshot until its last name is released (fork=${fork})`, async () => {
  const browser = await Lightningpanda.launch({ bin, args: fork ? ['--fork'] : [] });
  try {
    await browser.profile('app', { url });
    const profile = { base: fork ? 'app' : undefined, url: `${url}<b id="alias">shared</b>`, setup: fork ? [evaluate('window.aliasMarker="shared"')] : undefined };
    await Promise.all([browser.profile('first', profile), browser.profile('second', profile)]);
    await browser.releaseProfile('first');
    await using second = await browser.page({ as: 'second' });
    expect((await second.run([evaluate('window.aliasMarker ?? document.querySelector("#alias")?.textContent', 'text')])).values.text).toBe('shared');
    await second[Symbol.asyncDispose]();
    await browser.profile('third', profile);
    await Promise.all([browser.releaseProfile('second'), browser.releaseProfile('third')]);
    await browser.profile('old', profile);
    await Promise.all([browser.releaseProfile('old'), browser.profile('new', profile)]);
    await using fresh = await browser.page({ as: 'new' });
    expect((await fresh.run([evaluate('window.aliasMarker ?? document.querySelector("#alias")?.textContent', 'text')])).values.text).toBe('shared');
    await fresh[Symbol.asyncDispose]();
    await browser.releaseProfile('new');
    await using base = await browser.page({ as: 'app' });
    expect((await base.run([evaluate('window.aliasMarker === undefined && document.querySelector("#alias") === null', 'pristine')])).values.pristine).toBe(true);
  } finally { browser.close(); }
}, 20000);
