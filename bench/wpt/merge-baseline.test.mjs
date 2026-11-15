// bun test bench/wpt/merge-baseline.test.mjs
import { expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { formatBaseline, namesHash } from './lib.mjs';

const HEAD = { wpt: { repo: 'https://github.com/web-platform-tests/wpt', revision: 'a421f2fe' }, dirs: ['css/CSS2'] };
const H1 = namesHash(['reftest']);
const PASS = { status: 'complete', ms: 200, n: 1, fail: [], h: H1 };
const FAIL = { status: 'complete', ms: 200, n: 1, pass: [] };

function merge(base, ours, theirs, { head = [HEAD, HEAD, HEAD] } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'merge-baseline-'));
  const files = [base, ours, theirs].map((t, i) => {
    const f = path.join(dir, ['base', 'ours', 'theirs'][i]);
    fs.writeFileSync(f, typeof t === 'string' ? t : formatBaseline(head[i], t));
    return f;
  });
  const r = spawnSync(process.execPath, [path.join(import.meta.dir, 'merge-baseline.mjs'), ...files, '7', 'reftest-baseline.json'], { encoding: 'utf8' });
  const text = fs.readFileSync(files[1], 'utf8');
  fs.rmSync(dir, { recursive: true, force: true });
  return { status: r.status, text, stderr: r.stderr, json: () => JSON.parse(text).tests };
}

test('pages only one side changed or added are taken from that side, sorted', () => {
  const base = { 'a.html': FAIL, 'b.html': FAIL };
  const r = merge(base, { ...base, 'a.html': PASS, 'c.html': PASS }, { ...base, 'b.html': PASS, '0.html': FAIL });
  expect(r.status).toBe(0);
  expect(Object.keys(r.json())).toEqual(['0.html', 'a.html', 'b.html', 'c.html']);
  expect(r.json()['a.html']).toEqual(PASS);
  expect(r.json()['b.html']).toEqual(PASS);
  expect(r.text).toBe(formatBaseline(HEAD, r.json()));
});

test('a pass one side removed while the other left the page alone stays removed', () => {
  const base = { 'a.html': PASS, 'b.html': FAIL };
  const r = merge(base, { ...base, 'b.html': PASS }, { 'b.html': FAIL, 'a.html': FAIL });
  expect(r.status).toBe(0);
  expect(r.json()).toEqual({ 'a.html': FAIL, 'b.html': PASS });
});

test('a page both sides changed keeps the entry that passes more', () => {
  const base = { 'a.html': { ...FAIL, ms: 100 } };
  const r = merge(base, { 'a.html': { ...FAIL, ms: 300 } }, { 'a.html': PASS });
  expect(r.status).toBe(0);
  expect(r.json()['a.html']).toEqual(PASS);
});

test('a page both sides changed keeps the passes of both', () => {
  const failing = (fail) => ({ status: 'complete', ms: 500, n: 4, fail, h: namesHash(['w', 'x', 'y', 'z']) });
  const base = { 'p.html': { status: 'timeout', ms: 15000, n: 3, pass: ['x'] }, 'f.html': failing(['w', 'x', 'y']) };
  const ours = { 'p.html': { status: 'timeout', ms: 15000, n: 3, pass: ['x', 'y'] }, 'f.html': failing(['x', 'y']) };
  const theirs = { 'p.html': { status: 'complete', ms: 900, n: 3, pass: ['x', 'z'] }, 'f.html': failing(['w', 'y']) };
  const r = merge(base, ours, theirs);
  expect(r.status).toBe(0);
  expect(r.json()['p.html']).toEqual({ status: 'complete', ms: 15000, n: 3, pass: ['x', 'y', 'z'] });
  expect(r.json()['f.html']).toEqual(failing(['y']));
});

test('a fail list and a pass list combine when the hash proves the names', () => {
  const names = ['a', 'b', 'c'];
  const base = { 'x.html': { status: 'timeout', ms: 15000, n: 3, pass: [] } };
  const ours = { 'x.html': { status: 'complete', ms: 400, n: 3, fail: ['b', 'c'], h: namesHash(names) } };
  const theirs = { 'x.html': { status: 'timeout', ms: 15000, n: 3, pass: ['b'] } };
  const r = merge(base, ours, theirs);
  expect(r.status).toBe(0);
  expect(r.json()['x.html']).toEqual({ status: 'complete', ms: 15000, n: 3, fail: ['c'], h: namesHash(names) });
});

test('entries that cannot be combined are left as a conflict of their lines, the rest merged', () => {
  const base = { 'a.html': FAIL, 'x.html': { status: 'complete', ms: 100, n: 2, fail: ['p', 'q'], h: namesHash(['p', 'q']) } };
  const ours = { ...base, 'x.html': { status: 'complete', ms: 100, n: 2, fail: ['q'], h: namesHash(['p', 'q']) } };
  const theirs = { 'a.html': PASS, 'x.html': { status: 'complete', ms: 100, n: 3, fail: ['q'], h: namesHash(['p', 'q', 'r']) } };
  const r = merge(base, ours, theirs);
  expect(r.status).toBe(1);
  expect(r.text).toContain(`  "a.html": ${JSON.stringify(PASS)},\n<<<<<<< ours\n  "x.html": ${JSON.stringify(ours['x.html'])}\n=======\n  "x.html": ${JSON.stringify(theirs['x.html'])}\n>>>>>>> theirs\n }`);
});

test('a header change on one side with page changes on the other is a textual merge', () => {
  const base = { 'a.html': FAIL };
  const head2 = { ...HEAD, wpt: { ...HEAD.wpt, revision: 'b0000000' } };
  const r = merge(base, { 'a.html': PASS }, { 'a.html': FAIL }, { head: [HEAD, HEAD, head2] });
  expect(r.stderr).toContain('textual merge');
  expect(r.text).toContain('b0000000');
});

test('an unreadable side falls back to a textual merge with conflict markers', () => {
  const base = formatBaseline(HEAD, { 'a.html': FAIL });
  const r = merge(base, base.replace('"pass":[]', '"pass":[] BROKEN'), formatBaseline(HEAD, { 'a.html': PASS }));
  expect(r.status).toBe(1);
  expect(r.text).toContain('<<<<<<< ours');
});
