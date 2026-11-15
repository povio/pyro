// bun test bench/wpt/crashtest.test.mjs
import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { discoverCrashtests, isCrashtestName } from './crashtest.mjs';

test('a crashtest is named as WPT\'s manifest names one', () => {
  // docs/writing-tests/crashtest.md's examples, and tools/manifest/sourcefile.py's type flag.
  expect(isCrashtestName('css/css-foo/bar-crash.html')).toBe(true);
  expect(isCrashtestName('css/css-foo/crashtests/bar.html')).toBe(true);
  expect(isCrashtestName('css/css-foo/bar-crash-001.html')).toBe(false);
  expect(isCrashtestName('css/css-foo/bar-crash.tentative.html')).toBe(true);
  expect(isCrashtestName('css/css-foo/crash.html')).toBe(false);
  expect(isCrashtestName('css/crashtests-old/bar.html')).toBe(false);
  expect(isCrashtestName('dom/crashtests/deep/er/bar.xhtml')).toBe(true);
});

test('discovery keeps markup crashtests serve.mjs can serve and counts the rest', () => {
  const wpt = fs.mkdtempSync(path.join(os.tmpdir(), 'crashtest-discovery-'));
  const put = (rel, src = '<!doctype html><p>x') => {
    fs.mkdirSync(path.dirname(path.join(wpt, rel)), { recursive: true });
    fs.writeFileSync(path.join(wpt, rel), src);
  };
  try {
    put('css/css-foo/a-crash.html');
    put('css/css-foo/crashtests/b.svg', '<svg xmlns="http://www.w3.org/2000/svg"/>');
    put('css/css-foo/crashtests/long.html', '<meta name="timeout" content="long">');
    put('css/css-foo/crashtests/style.css', 'p{}');
    put('css/css-foo/crashtests/resources/frame.html');
    put('css/css-foo/not-a-test.html');
    put('dom/c-crash.https.html');
    put('dom/d-crash.html', '<script src="/resources/testdriver.js"></script>');
    put('dom/e-crash.html', '<!-- fetch("x.py") --><iframe src="f.html?pipe=trickle(d1)"></iframe>');
    put('dom/g-crash.html', '<!-- only a comment names handler.py -->');
    const { tests, excluded } = discoverCrashtests(wpt, ['.']);
    expect(tests).toEqual([
      { dir: 'css/css-foo', url: 'css/css-foo/a-crash.html', long: false },
      { dir: 'css/css-foo', url: 'css/css-foo/crashtests/b.svg', long: false },
      { dir: 'css/css-foo', url: 'css/css-foo/crashtests/long.html', long: true },
      { dir: 'dom', url: 'dom/g-crash.html', long: false },
    ]);
    expect(excluded).toEqual({
      'needs wptserve (substitution, https, h2, another host)': 1,
      'needs testdriver': 1,
      'needs wptserve (substitution, pipe, Python handler)': 1,
    });
    expect(discoverCrashtests(wpt, ['dom']).tests.map((t) => t.url)).toEqual(['dom/g-crash.html']);
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});
