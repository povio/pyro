import { test, expect } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { manifestUrls, runFromBaselines, rollup, indexEntry, updateIndex, siteData, splitUrl, passes, writeSite, copyPages, pageKey } from './status-export.mjs';

const manifest = {
  version: 9,
  items: {
    testharness: { dom: { 'a.html': ['h', [null, {}]], 'b.any.js': ['h', ['/dom/b.any.html', {}], ['/dom/b.any.worker.html', {}]] } },
    reftest: { css: { x: { 'r1.html': ['h', [null, [['/css/x/r1-ref.html', '==']], {}]], 'r2.html': ['h', [null, [], {}]] } } },
    crashtest: { svg: { crashtests: { 'c.html': ['h', [null, {}]] } } },
    manual: { dom: { 'm-manual.html': ['h', [null, {}]] } },
  },
};
const universe = { revision: 'rev', kinds: manifestUrls(manifest) };
const head = { wpt: { revision: 'rev' } };
const baselines = {
  testharness: { ...head, tests: {
    'dom/a.html': { status: 'complete', ms: 10, n: 3, fail: ['x'], h: '0' },
    'dom/b.any.html': { status: 'timeout', ms: 10, n: 4, pass: ['p', 'q'] },
  } },
  reftest: { ...head, tests: { 'css/x/r1.html': { status: 'complete', ms: 10, n: 1, fail: [], h: '0' } } },
};

test('manifestUrls: own path for null urls, variants by url, leading slash dropped, other kinds ignored', () => {
  expect(universe.kinds).toEqual({
    testharness: ['dom/a.html', 'dom/b.any.html', 'dom/b.any.worker.html'],
    reftest: ['css/x/r1.html', 'css/x/r2.html'],
    crashtest: ['svg/crashtests/c.html'],
  });
});

test('runFromBaselines: status, passed, total and failing names', () => {
  const run = runFromBaselines(baselines, { sha: 's1', date: '2026-11-17T00:00:00Z' });
  expect(run.wpt).toBe('rev');
  expect(run.kinds.testharness['dom/a.html']).toEqual(['complete', 2, 3, ['x']]);
  expect(run.kinds.testharness['dom/b.any.html']).toEqual(['timeout', 2, 4]);
  expect(run.kinds.reftest['css/x/r1.html']).toEqual(['complete', 1, 1]);
  expect(run.kinds.crashtest).toBeUndefined();
  expect(passes(run.kinds.reftest['css/x/r1.html'])).toBe(true);
  expect(passes(run.kinds.testharness['dom/a.html'])).toBe(false);
});

test('rollup: WPT is the denominator, not-run pages count, Chrome per page', () => {
  const run = runFromBaselines(baselines, { sha: 's1', date: 'd' });
  const chrome = { tests: { 'css/x/r1.html': [1, 1, 'P'], 'css/x/r2.html': [0, 1, 'F'], 'dom/a.html': [3, 3, 'O'] } };
  const r = rollup(run, universe, chrome);
  // [WPT, run, passed, sub passed, subs, Chrome passed, Chrome ran, Chrome passed of run, Chrome sub passed, Chrome subs]
  expect(r.get('').reftest).toEqual([2, 1, 1, 1, 1, 1, 2, 1, 1, 2]);
  expect(r.get('css/x').reftest).toEqual([2, 1, 1, 1, 1, 1, 2, 1, 1, 2]);
  expect(r.get('dom').testharness).toEqual([3, 2, 0, 4, 7, 1, 1, 1, 3, 3]);
  expect(r.get('svg/crashtests').crashtest).toEqual([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
});

test('a page pyro runs that the manifest lacks still counts once', () => {
  const run = runFromBaselines({ testharness: { ...head, tests: { 'dom/extra.html': { status: 'complete', ms: 1, n: 1, fail: [], h: '0' } } } }, { sha: 's', date: 'd' });
  expect(rollup(run, universe, null).get('dom').testharness.slice(0, 3)).toEqual([4, 1, 1]);
});

test('index: totals per kind, top-level dirs with a run page, sorted by date, a sha replaces itself', () => {
  const a = indexEntry(runFromBaselines(baselines, { sha: 'b', date: '2026-11-02' }), universe);
  const b = indexEntry(runFromBaselines(baselines, { sha: 'a', date: '2026-11-01' }), universe);
  expect(a.totals.reftest).toEqual([2, 1, 1, 1, 1]);
  expect(Object.keys(a.dirs).sort()).toEqual(['css', 'dom']);
  const idx = updateIndex(updateIndex(updateIndex(null, a), b), { ...a, subject: 'again' });
  expect(idx.runs.map((r) => r.sha)).toEqual(['a', 'b']);
  expect(idx.runs[1].subject).toBe('again');
});

test('splitUrl keeps a variant query with slashes in the name', () => {
  expect(splitUrl('encoding/x/a.html?include=/b/c')).toEqual(['encoding/x', 'a.html?include=/b/c']);
  expect(splitUrl('top.html')).toEqual(['', 'top.html']);
});

test('siteData: one page per directory, children, tests, history', () => {
  const runs = [runFromBaselines(baselines, { sha: 'a'.repeat(40), date: '2026-11-01' }), runFromBaselines(baselines, { sha: 'b'.repeat(40), date: '2026-11-02' })];
  const pages = siteData(runs, { rev: universe }, {});
  expect([...pages.keys()].sort()).toEqual(['', 'css', 'css/x', 'dom', 'svg', 'svg/crashtests']);
  expect(pages.get('').dirs.map((d) => d.name)).toEqual(['css', 'dom', 'svg']);
  expect(pages.get('dom').tests).toEqual([
    { n: 'a.html', k: 'testharness', s: 'complete', p: 2, t: 3, f: ['x'] },
    { n: 'b.any.html', k: 'testharness', s: 'timeout', p: 2, t: 4 },
    { n: 'b.any.worker.html', k: 'testharness', s: null },
  ]);
  expect(pages.get('css').history.length).toBe(2);
  expect(pages.get('css').history[1][2].reftest).toEqual([1, 2, 1, 1]);
});

test('writeSite: pages and data files, and a placeholder without data', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wpt-status-'));
  const docs = path.join(tmp, 'docs');
  expect(writeSite(null, docs)).toBe(0);
  expect(fs.readFileSync(path.join(docs, 'status/wpt.md'), 'utf8')).toContain('No WPT results');
  const data = path.join(tmp, 'data');
  fs.mkdirSync(path.join(data, 'runs'), { recursive: true });
  fs.mkdirSync(path.join(data, 'manifest'), { recursive: true });
  const run = runFromBaselines(baselines, { sha: 'c'.repeat(40), date: '2026-11-03' });
  fs.writeFileSync(path.join(data, 'runs', `${run.sha}.json.gz`), Bun.gzipSync(JSON.stringify(run)));
  fs.writeFileSync(path.join(data, 'manifest', 'rev.json.gz'), Bun.gzipSync(JSON.stringify(universe)));
  fs.writeFileSync(path.join(data, 'index.json'), JSON.stringify(updateIndex(null, indexEntry(run, universe))));
  expect(writeSite(data, docs)).toBe(6);
  expect(fs.readFileSync(path.join(docs, 'status/wpt.md'), 'utf8')).toContain('<WptStatus />');
  expect(JSON.parse(fs.readFileSync(path.join(docs, 'public/status-data/wpt/svg/crashtests.json'), 'utf8')).counts.crashtest[0]).toBe(1);
  // The build's output: public/ copied to the root, the one page rendered.
  const dist = path.join(tmp, 'dist');
  fs.cpSync(path.join(docs, 'public'), dist, { recursive: true });
  fs.mkdirSync(path.join(dist, 'status'));
  fs.writeFileSync(path.join(dist, 'status/wpt.html'), '<html>shell</html>');
  expect(copyPages(dist)).toBe(5);
  expect(fs.readFileSync(path.join(dist, 'status/wpt/svg/crashtests.html'), 'utf8')).toBe('<html>shell</html>');
  fs.rmSync(tmp, { recursive: true });
});

test('copyPages: each copy maps itself to the status page module, so it hydrates as that page', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'wpt-pages-'));
  fs.mkdirSync(path.join(tmp, 'status-data'), { recursive: true });
  fs.mkdirSync(path.join(tmp, 'status'));
  fs.mkdirSync(path.join(tmp, 'assets'));
  fs.writeFileSync(path.join(tmp, 'status-data/dirs.json'), JSON.stringify(['css/x', 'a/B', 'a/b', 'odd/[dir]']));
  fs.writeFileSync(path.join(tmp, 'status/wpt.html'), '<head><script>window.__VP_HASH_MAP__=JSON.parse("{\\"status_wpt.md\\":\\"Ab-1\\"}");</script></head><body>x</body>');
  expect(copyPages(tmp)).toBe(4);
  const page = fs.readFileSync(path.join(tmp, 'status/wpt/css/x.html'), 'utf8');
  expect(page).toContain('</script><script>window.__VP_HASH_MAP__["status_wpt_css_x.md"]="Ab-1"</script></head>');
  expect(fs.readFileSync(path.join(tmp, 'assets/status_wpt_css_x.md.Ab-1.lean.js'), 'utf8')).toBe(
    'import * as m from "./status_wpt.md.Ab-1.lean.js";export default m.default;' +
      'export const __pageData = { ...m.__pageData, relativePath: "status/wpt/css/x.md", filePath: "status/wpt/css/x.md" };\n',
  );
  expect(pageKey('odd/[dir]')).toBe('status_wpt_odd__dir_.md');
  // Two directories whose names differ only in case share a key (the map is case-insensitive): plain copies.
  expect(fs.readFileSync(path.join(tmp, 'status/wpt/a/B.html'), 'utf8')).not.toContain('status_wpt_a_b');
  fs.rmSync(tmp, { recursive: true });
});
