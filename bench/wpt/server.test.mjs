// bun test bench/wpt/server.test.mjs
// Which server a gate run uses, and what reftest and crashtest discovery keeps under each (README "wptserve").
import { expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import vm from 'node:vm';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { discoverCrashtests } from './crashtest.mjs';
import { NEEDS_H2, dedicatedWorkerEntries, discover, discoverReftests, serverFor } from './lib.mjs';

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

function checkout(files) {
  const wpt = fs.mkdtempSync(path.join(os.tmpdir(), 'server-discovery-'));
  for (const [rel, src] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(wpt, rel)), { recursive: true });
    fs.writeFileSync(path.join(wpt, rel), src);
  }
  return wpt;
}

test('dedicated worker discovery uses manifest variants and requires the matching source and wptserve', () => {
  const files = {
    'encoding/both.any.js': '// META: variant=?first\n',
    'encoding/modules.any.js': '// META: global=dedicatedworker-module\n',
    'encoding/other.any.js': '',
    'encoding/protocol.h2.worker.js': 'done();',
    'protocol/only.h2.html': '<script src="/resources/testharness.js"></script>',
    'encoding/only.worker.js': 'importScripts("/resources/testharness.js");',
  };
  const wpt = checkout(files);
  const entries = {};
  function entry(file, ...urls) {
    const src = files['encoding/' + file];
    const hash = crypto.createHash('sha1').update('blob ' + Buffer.byteLength(src) + '\0' + src).digest('hex');
    entries[file] = [hash, ...urls.map((u) => ['encoding/' + u, {}])];
  }
  entry('both.any.js', 'both.any.html?first', 'both.any.worker.html?first', 'both.any.sharedworker.html?first');
  entry('modules.any.js', 'modules.any.worker-module.html', 'modules.any.serviceworker-module.html');
  entry('other.any.js', 'other.any.html', 'other.any.worker.html');
  entry('protocol.h2.worker.js', 'protocol.h2.worker.html');
  entry('only.worker.js', 'only.worker.html');
  try {
    const dedicatedWorkers = dedicatedWorkerEntries({ url_base: '/', items: { testharness: { encoding: entries } } });
    const urls = (opts) => discover(wpt, ['encoding'], opts).tests.map((t) => t.url).sort();
    expect(urls({ wptserve: true })).toEqual(['encoding/both.any.html?first', 'encoding/other.any.html']);
    expect(() => urls({ dedicatedWorkers })).toThrow('needs --server wptserve');
    expect(urls({ dedicatedWorkers, wptserve: true })).toEqual([
      'encoding/both.any.html?first', 'encoding/both.any.worker.html?first',
      'encoding/modules.any.worker-module.html', 'encoding/only.worker.html', 'encoding/other.any.html', 'encoding/other.any.worker.html',
    ]);
    const opts = { wptserve: true, dedicatedWorkers };
    const sample = discover(wpt, ['encoding'], { ...opts, sampleEncoding: 2 }).tests;
    expect(sample.filter((t) => !t.environment).map((t) => t.url).sort()).toEqual(urls({ wptserve: true, sampleEncoding: 2 }));
    const workers = discover(wpt, ['encoding'], opts).tests.filter((t) => t.environment);
    expect(sample.filter((t) => t.environment)).toEqual(workers.filter((_, i) => i % 2 === 0));
    const reasons = new Map();
    discover(wpt, ['encoding'], { wptserve: true, dedicatedWorkers, reasons });
    expect(reasons.get('encoding/protocol.h2.worker.html')).toBe(NEEDS_H2);
    expect(discover(wpt, ['protocol'], { wptserve: true }).tests).toHaveLength(1);
    expect(discover(wpt, ['protocol'], { ...opts, reasons }).tests).toHaveLength(0);
    expect(reasons.get('protocol/only.h2.html')).toBe(NEEDS_H2);
    fs.appendFileSync(path.join(wpt, 'encoding/only.worker.js'), 'done();');
    expect(() => urls({ dedicatedWorkers, wptserve: true })).toThrow('does not match source');
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

test('worker results retain delimiters, repeated names and harness errors even before the first result', () => {
  const callbacks = {};
  const script = fs.readFileSync(new URL('./testharnessreport.js', import.meta.url), 'utf8');
  const context = { setup() {}, add_test_state_callback() {}, add_result_callback(fn) { callbacks.result = fn; },
    add_completion_callback(fn) { callbacks.complete = fn; } };
  vm.runInNewContext(script, context);
  const status = { format_status: () => 'Error', message: 'worker script failed' };
  callbacks.complete([], status);
  expect(context.report.snapshot()).toEqual({ harnessStatus: 'Error', error: 'worker script failed', subtests: [] });
  const name = 'same|name\non two lines';
  const tests = ['Pass', 'Fail'].map((s) => ({ name, format_status: () => s }));
  callbacks.result(tests[0]);
  expect(context.report.snapshot().subtests).toEqual([{ name, status: 'Pass', msg: '' }]);
  callbacks.complete(tests, status);
  expect(context.report.snapshot().subtests.map((t) => [t.name, t.status])).toEqual([[name, 'Pass'], [name, 'Fail']]);
});

test('dedicated worker measurements cannot update a window gate baseline', () => {
  const wpt = checkout({ 'results.json': JSON.stringify({ dedicatedWorkers: true, results: [] }) });
  try {
    const proc = spawnSync(process.execPath, [new URL('./gate.mjs', import.meta.url).pathname,
      'update', path.join(wpt, 'results.json'), '--baseline', path.join(wpt, 'baseline.json')], { encoding: 'utf8' });
    expect(proc.status).toBe(2);
    expect(proc.stderr).toContain('dedicated worker measurements cannot be used');
    expect(fs.existsSync(path.join(wpt, 'baseline.json'))).toBe(false);
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

test('reftest discovery: wptserve and testdriver pages run under wptserve, h2 pages under neither', () => {
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
    expect(stat.excluded).toMatchObject({ 'needs wptserve (substitution, pipe, Python handler)': 3, 'needs testdriver': 1, [NEEDS_H2]: 1 });
    expect(urls({ wptserve: true })).toEqual(['css/a/handler.html', 'css/a/name.sub.html', 'css/a/pipe.html', 'css/a/plain.html']);
    const all = discoverReftests(wpt, ['css'], { wptserve: true, testdriver: true });
    expect(all.tests.map((t) => t.url).sort()).toEqual(['css/a/driver.html', 'css/a/handler.html', 'css/a/name.sub.html', 'css/a/pipe.html', 'css/a/plain.html']);
    // Nothing is left out for the server but h2, which wptserve.mjs does not serve.
    expect(all.excluded).toEqual({ [NEEDS_H2]: 1, 'reference file': 1 });
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});

test('crashtest discovery: wptserve and testdriver pages run under wptserve, h2 pages under neither', () => {
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
      [NEEDS_H2]: 1,
      'needs wptserve (substitution, https, h2, another host)': 1,
      'needs testdriver': 1,
      'needs wptserve (substitution, pipe, Python handler)': 1,
    });
    const all = discoverCrashtests(wpt, ['.'], { wptserve: true, testdriver: true });
    expect(all.tests.map((t) => t.url)).toEqual(['dom/a-crash.html', 'dom/b-crash.https.html', 'dom/c-crash.html', 'dom/d-crash.html']);
    expect(all.excluded).toEqual({ [NEEDS_H2]: 1 });
  } finally {
    fs.rmSync(wpt, { recursive: true, force: true });
  }
});
