// bun test bench/wpt/server.test.mjs
// Which server a gate run uses, and what reftest and crashtest discovery keeps under each (README "wptserve").
import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { discoverCrashtests } from './crashtest.mjs';
import { discover, discoverReftests, serverFor } from './lib.mjs';

test('reftests and crashtests gate under wptserve with testdriver; testharness pages under serve.mjs', () => {
  expect(serverFor('reftest')).toEqual({ server: 'wptserve', testdriver: true });
  expect(serverFor('crashtest')).toEqual({ server: 'wptserve', testdriver: true });
  expect(serverFor('testharness')).toEqual({ server: 'static', testdriver: false });
  expect(serverFor('testharness', 'wptserve')).toEqual({ server: 'wptserve', testdriver: false });
  expect(serverFor('testharness', 'wptserve', true)).toEqual({ server: 'wptserve', testdriver: true });
  // --server static: serve.mjs, so no testdriver (run.mjs refuses --testdriver without wptserve).
  expect(serverFor('reftest', 'static')).toEqual({ server: 'static', testdriver: false });
  expect(serverFor('crashtest', 'static', true)).toEqual({ server: 'static', testdriver: false });
  expect(() => serverFor('reftest', 'h2o')).toThrow();
});

test('testharness discovery reads UTF-16 markup with either byte order mark', () => {
  const markup = Buffer.from('\ufeff<script src="/resources/testharness.js"></script>', 'utf16le');
  const wpt = checkout({ 'css/little.html': markup, 'css/big.html': Buffer.from(markup).swap16() });
  try {
    expect(discover(wpt, ['css']).tests.map(test => test.url).sort()).toEqual(['css/big.html', 'css/little.html']);
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

test('worker discovery expands classic globals and query variants only when requested', () => {
  const wpt = checkout({ 'css/a.any.js': '// META: global=worker\n// META: variant=?x', 'css/b.any.js': '', 'css/c.worker.js': '' });
  try {
    const urls = options => discover(wpt, ['css'], options).tests.map(test => test.url).sort();
    expect(urls({ wptserve: true })).toEqual(['css/b.any.html']);
    expect(urls({ wptserve: true, workerGlobals: true })).toEqual(['css/a.any.serviceworker.html?x', 'css/a.any.sharedworker.html?x', 'css/a.any.worker.html?x', 'css/b.any.html', 'css/b.any.worker.html', 'css/c.worker.html']);
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

function checkout(files) {
  const wpt = fs.mkdtempSync(path.join(os.tmpdir(), 'server-discovery-'));
  for (const [rel, src] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(wpt, rel)), { recursive: true });
    fs.writeFileSync(path.join(wpt, rel), src);
  }
  return wpt;
}

test('reftest discovery: wptserve, h2 and testdriver pages run under wptserve, h2 pages not under serve.mjs', () => {
  const match = '<link rel="match" href="ref.html">';
  const wpt = checkout({
    'css/a/ref.html': '<p>ref',
    'css/a/plain.html': `${match}<p>x`,
    'css/a/name.sub.html': `${match}<p>{{host}}`,
    'css/a/handler.html': `${match}<img src="support/img.py">`,
    'css/a/pipe.html': `<link rel="match" href="ref.html?pipe=trickle(d1)"><p>x`,
    'css/a/driver.html': `${match}<script src="/resources/testdriver.js"></script><script src="/resources/testdriver-vendor.js"></script>`,
    'css/a/push.h2.html': `${match}<p>x`,
  });
  try {
    const urls = (o) => discoverReftests(wpt, ['css'], o).tests.map((t) => t.url).sort();
    const stat = discoverReftests(wpt, ['css']);
    expect(stat.tests.map((t) => t.url)).toEqual(['css/a/plain.html']);
    expect(stat.excluded).toMatchObject({ 'needs wptserve (substitution, pipe, Python handler)': 4, 'needs testdriver': 1 });
    expect(urls({ wptserve: true })).toEqual(['css/a/handler.html', 'css/a/name.sub.html', 'css/a/pipe.html', 'css/a/plain.html', 'css/a/push.h2.html']);
    const all = discoverReftests(wpt, ['css'], { wptserve: true, testdriver: true });
    expect(all.tests.map((t) => t.url).sort()).toEqual(['css/a/driver.html', 'css/a/handler.html', 'css/a/name.sub.html', 'css/a/pipe.html', 'css/a/plain.html', 'css/a/push.h2.html']);
    // Nothing is left out for the server: wptserve.mjs serves h2 too.
    expect(all.excluded).toEqual({ 'reference file': 1 });
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

test('crashtest discovery: wptserve, h2 and testdriver pages run under wptserve, h2 pages not under serve.mjs', () => {
  const wpt = checkout({
    'dom/a-crash.html': '<p>x',
    'dom/b-crash.https.html': '<p>x',
    'dom/c-crash.html': '<script src="/resources/testdriver.js"></script>',
    'dom/d-crash.html': '<iframe src="f.html?pipe=trickle(d1)"></iframe>',
    'dom/e-crash.h2.html': '<p>x',
  });
  try {
    const stat = discoverCrashtests(wpt, ['.']);
    expect(stat.tests.map((t) => t.url)).toEqual(['dom/a-crash.html']);
    expect(stat.excluded).toEqual({
      'needs wptserve (substitution, https, h2, another host)': 2,
      'needs testdriver': 1,
      'needs wptserve (substitution, pipe, Python handler)': 1,
    });
    const all = discoverCrashtests(wpt, ['.'], { wptserve: true, testdriver: true });
    expect(all.tests.map((t) => t.url)).toEqual(['dom/a-crash.html', 'dom/b-crash.https.html', 'dom/c-crash.html', 'dom/d-crash.html', 'dom/e-crash.h2.html']);
    expect(all.excluded).toEqual({});
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});
