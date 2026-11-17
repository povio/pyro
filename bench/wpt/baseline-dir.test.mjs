// A baseline split per top-level directory reads back as the one file it was made from.
import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readBaselineAt, readJson, writeBaselineAt } from './lib.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));

test('a baseline round-trips through a directory of per-directory files', () => {
  const one = readJson(path.join(HERE, 'baseline.json'));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'baseline-dir-')) + '/split/';
  try {
    const { tests, ...head } = one;
    writeBaselineAt(dir, head, tests);
    const files = fs.readdirSync(dir).sort();
    expect(files).toEqual([...new Set(Object.keys(tests).map((u) => u.split('/')[0] + '.json'))].sort());
    const back = readBaselineAt(dir);
    expect(back.wpt).toEqual(one.wpt);
    expect(back.tests).toEqual(one.tests);
    // Unchanged files are not rewritten; a directory with no pages left loses its file.
    const before = fs.statSync(path.join(dir, 'dom.json')).mtimeMs;
    const fewer = Object.fromEntries(Object.entries(tests).filter(([u]) => !u.startsWith('url/')));
    writeBaselineAt(dir, head, fewer);
    expect(fs.existsSync(path.join(dir, 'url.json'))).toBe(false);
    expect(fs.statSync(path.join(dir, 'dom.json')).mtimeMs).toBe(before);
    // A page in the wrong file is an error, not a silent move.
    fs.writeFileSync(path.join(dir, 'xhr.json'), fs.readFileSync(path.join(dir, 'dom.json')));
    expect(() => readBaselineAt(dir)).toThrow(/belongs in dom\.json/);
  } finally {
    fs.rmSync(path.dirname(dir), { recursive: true, force: true });
  }
});

test('a missing baseline reads as none', () => {
  expect(readBaselineAt(path.join(os.tmpdir(), 'no-such-baseline-here.json'))).toBe(null);
});
