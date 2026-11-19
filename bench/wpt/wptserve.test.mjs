// bun test bench/wpt/wptserve.test.mjs
// The runner never passes a page it could not load.
// The parts that start wptserve need WPT_DIR (a checkout with fetch-wpt.sh --venv); the parts that run the browser
// also need PYRO_BIN. Without them those tests are skipped.
import { expect, test } from 'bun:test';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { documentError } from './lib.mjs';
import { refKey } from './reftest.mjs';

const WPT = process.env.WPT_DIR;
const BIN = process.env.PYRO_BIN;
const HAS_WPT = !!WPT && fs.existsSync(path.join(WPT, 'wpt'));
const res = (status) => ({ status: () => status });

test('a main document that is not a 2xx response is an error, never a page to judge', () => {
  expect(documentError(res(200), 'a.html')).toBeNull();
  expect(documentError(res(204), 'a.html')).toBeNull();
  // The proxy's answer for a server that stopped answering, a missing file, a redirect left unfollowed.
  expect(documentError(res(502), 'a.html')).toBe('HTTP 502 for a.html');
  expect(documentError(res(404), 'a.html')).toBe('HTTP 404 for a.html');
  expect(documentError(res(301), 'a.html')).toBe('HTTP 301 for a.html');
  expect(documentError(null, 'a.html')).toBe('no response for a.html');
  expect(documentError(null, 'about:blank')).toBeNull();
});

test('a reference render is cached per origin it loads from', () => {
  const v = { width: 800, height: 600 };
  expect(refKey(v, 'http://web-platform.test:8000', 'r.html')).not.toBe(refKey(v, 'https://web-platform.test:8443', 'r.html'));
  expect(refKey(v, 'http://web-platform.test:8000', 'r.html')).not.toBe(refKey(v, 'http://www.web-platform.test:8000', 'r.html'));
  expect(refKey(v, 'http://a:1', 'r.html')).toBe(refKey(v, 'http://a:1', 'r.html'));
  expect(refKey({ width: 600, height: 600 }, 'http://a:1', 'r.html')).not.toBe(refKey(v, 'http://a:1', 'r.html'));
  expect(refKey(v, 'http://a:1', 'about:blank')).toBe(refKey(v, 'https://b:2', 'about:blank'));
});

// A page discovery keeps, served by a server that answers every request with an empty 502 (what the proxy answers
// for a wptserve that stopped answering): the crashtest and the reftest (whose test and reference would render the
// same empty document) are errors.
test.skipIf(!HAS_WPT || !BIN)('pages served as HTTP errors are errors, not passes', async () => {
  const bad = http.createServer((q, s) => { s.writeHead(502, { 'content-length': 0 }); s.end(); });
  const port = await new Promise((r) => bad.listen(0, '127.0.0.1', () => r(bad.address().port)));
  try {
    for (const [kind, match] of [['crashtest', '^css/css-multicol/crashtests/(balance-|change-)'], ['reftest', '^css/css-backgrounds/background-color-body-propagation-00']]) {
      const out = path.join(os.tmpdir(), `wptserve-test-${process.pid}-${kind}.json`);
      // Not spawnSync: the 502 server answers from this process's event loop.
      const run = spawn(process.execPath, [path.join(import.meta.dir, 'run.mjs'), '--kind', kind, '--wpt', WPT, '--bin', `t=${BIN}`, '--base', `http://127.0.0.1:${port}`,
        '--match', match, '--jobs', '2', '--out', out], { stdio: 'ignore' });
      expect(await new Promise((r) => run.once('exit', r))).toBe(0);
      const results = JSON.parse(fs.readFileSync(out, 'utf8')).results.slice(0, 6);
      expect(results.length).toBeGreaterThan(0);
      for (const t of results) {
        expect(t.status).toBe('error');
        expect(t.error).toMatch(/HTTP 502/);
        expect((t.subtests ?? []).some((s) => s.status === 'Pass')).toBe(false);
      }
      fs.rmSync(out, { force: true });
    }
  } finally { bad.close(); }
}, 300000);
