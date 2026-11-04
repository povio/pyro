// CDP performance suite: latency and throughput of realistic CDP workloads against one or more
// `serve` binaries, written as JSON (plus a Markdown summary) that cdp-perf-compare.mjs diffs run to run.
//
// Usage:
//   node cdp-perf.mjs --bin [label=]/abs/lightningpanda [--bin base=/abs/other] --out /abs/result.json
//                     [--rounds 5] [--scale 1] [--quick] [--filter REGEX] [--list]
//                     [--sampler /abs/rss-sampler] [--playwright] [--perf-trace] [--fork]
//
// Run it with `bun` to include the Bun.WebView scenarios (its Chrome backend attached to the endpoint);
// `node` runs the raw CDP scenarios only. --playwright adds the Playwright scenarios (playwright-core).
//
// Every scenario runs in a fresh browser process, once per round and binary. With several binaries the order
// alternates every round (AB, BA, ...), so host drift hits each binary equally; that is the only fair way to
// compare on a busy machine. Timings are wall-clock milliseconds measured by this harness (Node + `ws`), so
// they include the WebSocket hop. Browser CPU per operation comes from the native sampler (macOS) or /proc
// (Linux) and excludes the harness. The macOS sampler also reads instructions retired, which unlike wall clock
// and CPU time barely move with host load: on a machine that cannot go quiet, compare those.
//
// --perf-trace needs binaries built with `--features perf-trace`: it resets the browser's stage timers after
// warmup and reads them (LP.perfStats) after the measured loop, attributing inner CDP costs per scenario.
//
// Exit status: 0 when every scenario passed for every binary, 1 if any failed (results are written either
// way), 2 for usage errors.
import { spawn, execFile } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { summarize, median } from './cdp-perf-stats.mjs';

const require = createRequire(import.meta.url);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const SCHEMA = 'lightningpanda-cdp-perf/1';

// ---------- arguments ----------
const argv = process.argv.slice(2);
const flag = (k) => argv.includes(k);
const opt = (k, d) => {
  const i = argv.indexOf(k);
  return i >= 0 ? argv[i + 1] : d;
};
const many = (k) => argv.flatMap((a, i) => (a === k ? [argv[i + 1]] : []));
const usage = () => {
  console.error('usage: node cdp-perf.mjs --bin [label=]/abs/binary [--bin ...] --out /abs/result.json');
  console.error('       [--rounds N] [--scale X] [--quick] [--filter REGEX] [--list] [--sampler PATH] [--playwright] [--perf-trace] [--fork]');
  process.exit(2);
};

const QUICK = flag('--quick');
const ROUNDS = Number(opt('--rounds', QUICK ? 1 : 5));
const SCALE = Number(opt('--scale', QUICK ? 0.2 : 1));
const FILTER = opt('--filter') ? new RegExp(opt('--filter')) : null;
const OUT = opt('--out');
const PERF_TRACE = flag('--perf-trace');
// serve --fork: connections run in forked children, so CPU is read from the descendants of serve.
const FORK = flag('--fork');
const BINS = many('--bin').map((spec, i) => {
  const eq = spec.indexOf('=');
  const label = eq > 0 ? spec.slice(0, eq) : i === 0 ? 'current' : `bin${i}`;
  return { label, path: path.resolve(eq > 0 ? spec.slice(eq + 1) : spec) };
});

// Same flags and environment the TOS fixture uses (see run-baseline.mjs).
const LP_FLAGS = ['--log-level', 'error', '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'];
const LP_ENV = { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' };

// ---------- small helpers ----------
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const now = () => performance.now();
const freePort = () =>
  new Promise((res, rej) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => {
      const p = s.address().port;
      s.close(() => res(p));
    });
    s.on('error', rej);
  });
const getJson = (url) =>
  new Promise((res, rej) => {
    const req = http.get(url, { agent: false }, (r) => {
      const c = [];
      r.on('data', (d) => c.push(d));
      r.on('end', () => res({ status: r.statusCode, body: Buffer.concat(c).toString() }));
    });
    req.on('error', rej);
    req.setTimeout(1000, () => req.destroy(new Error('timeout')));
  });
const sha256 = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const withTimeout = (p, ms, what) => {
  let t;
  return Promise.race([
    p.finally(() => clearTimeout(t)),
    new Promise((_, rej) => {
      t = setTimeout(() => rej(new Error(`${what}: timed out after ${ms} ms`)), ms);
    }),
  ]);
};
// Warmup iterations pass negative indexes.
const wrap = (i, n) => ((i % n) + n) % n;
function assert(cond, msg) {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
}

const children = new Set();
const track = (cp) => {
  children.add(cp);
  cp.once('exit', () => children.delete(cp));
  return cp;
};
const killGroup = (cp, signal = 'SIGKILL') => {
  try {
    process.kill(-cp.pid, signal);
  } catch {
    try {
      cp.kill(signal);
    } catch {}
  }
};
const exited = (cp) => new Promise((r) => (cp.exitCode !== null || cp.signalCode !== null ? r() : cp.once('exit', r)));
process.on('exit', () => {
  for (const cp of children) killGroup(cp);
});
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => process.exit(130));

// ---------- minimal CDP client ----------
const WebSocket = require('ws');

class Cdp {
  static open(url, timeoutMs = 10_000) {
    return new Promise((res, rej) => {
      const ws = new WebSocket(url, { perMessageDeflate: false, maxPayload: 256 * 1024 * 1024 });
      const t = setTimeout(() => {
        ws.terminate();
        rej(new Error(`connect ${url}: timed out`));
      }, timeoutMs);
      ws.once('open', () => {
        clearTimeout(t);
        res(new Cdp(ws));
      });
      ws.once('error', (e) => {
        clearTimeout(t);
        rej(e);
      });
    });
  }

  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.waiters = new Set();
    this.closed = new Promise((r) => ws.once('close', r));
    ws.on('message', (data) => this.onMessage(data));
    ws.on('close', () => {
      for (const p of this.pending.values()) p.rej(new Error(`connection closed while waiting for ${p.method}`));
      this.pending.clear();
      for (const w of this.waiters) w.rej(new Error(`connection closed while waiting for ${w.method}`));
      this.waiters.clear();
    });
  }

  onMessage(data) {
    const msg = JSON.parse(data);
    if (msg.id !== undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.rej(new Error(`${p.method}: ${msg.error.message ?? JSON.stringify(msg.error)}`));
      else p.res(msg.result);
      return;
    }
    for (const w of this.waiters) {
      if (w.method === msg.method && (w.sessionId === undefined || w.sessionId === msg.sessionId) && w.pred(msg.params)) {
        this.waiters.delete(w);
        clearTimeout(w.timer);
        w.res(msg.params);
      }
    }
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const msg = { id, method, params };
    if (sessionId) msg.sessionId = sessionId;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej, method });
      this.ws.send(JSON.stringify(msg));
    });
  }

  // Register before triggering the event; events are not buffered.
  waitFor(method, { sessionId, pred = () => true, timeoutMs = 30_000 } = {}) {
    return new Promise((res, rej) => {
      const w = { method, sessionId, pred, res, rej };
      w.timer = setTimeout(() => {
        this.waiters.delete(w);
        rej(new Error(`waiting for ${method}: timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.waiters.add(w);
    });
  }

  async close() {
    if (this.ws.readyState === WebSocket.OPEN) this.ws.close();
    await withTimeout(this.closed, 5000, 'ws close').catch(() => this.ws.terminate());
  }
}

// A page in its own browser context, attached in flatten mode with Page and Runtime enabled.
async function newPage(cdp) {
  const { browserContextId } = await cdp.send('Target.createBrowserContext');
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  await cdp.send('Page.enable', {}, sessionId);
  await cdp.send('Runtime.enable', {}, sessionId);
  return { browserContextId, targetId, sessionId };
}

async function closePage(cdp, page) {
  await cdp.send('Target.closeTarget', { targetId: page.targetId });
  await cdp.send('Target.disposeBrowserContext', { browserContextId: page.browserContextId });
}

// Navigates and waits for the load event. Returns per-milestone times (ms since the command was sent).
async function navigate(cdp, sessionId, url) {
  const t0 = now();
  const phases = {};
  const mark = (name) => (v) => {
    phases[name] = now() - t0;
    return v;
  };
  const navigated = cdp.waitFor('Page.frameNavigated', { sessionId, pred: (p) => !p.frame.parentId }).then(mark('frameNavigated'));
  const dcl = cdp.waitFor('Page.domContentEventFired', { sessionId }).then(mark('domContentEventFired'));
  const load = cdp.waitFor('Page.loadEventFired', { sessionId }).then(mark('loadEventFired'));
  const r = await cdp.send('Page.navigate', { url }, sessionId).then(mark('navigateReply'));
  if (r.errorText) throw new Error(`Page.navigate ${url}: ${r.errorText}`);
  await Promise.all([navigated, dcl, load]);
  return phases;
}

async function evaluate(cdp, sessionId, expression, extra = {}) {
  const r = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, ...extra }, sessionId);
  if (r.exceptionDetails) throw new Error(`evaluate ${expression.slice(0, 60)}: ${JSON.stringify(r.exceptionDetails).slice(0, 300)}`);
  return r.result.value;
}

async function objectId(cdp, sessionId, expression) {
  const r = await cdp.send('Runtime.evaluate', { expression }, sessionId);
  assert(r.result?.objectId, `${expression} returned no object`);
  return r.result.objectId;
}

// Playwright re-sends its large injected utilities through callFunctionOn; this mimics that size (~38 KB).
const BIG_FN = `function(a) { ${Array.from({ length: 2000 }, (_, i) => `const v${i} = ${i};`).join('\n')} return a + 1; }`;

// ---------- scenarios ----------
// Page.printToPDF cuts a document without `.pz-page` elements into pages (`paint_paper` in lp-render): layout at
// the paper's width, shrink to fit, `Breaks::of`, one cut per page. bench/pdf's ToS documents have `.pz-page` and
// never get here. The content is set once per scenario; the sample is the print alone, and checks the page count roughly.
const PAPER_CUT_WORDS = (n) => Array.from({ length: n }, (_, i) => `word${i}`).join(' ');
const PAPER_CUT_BOX = (i) => `<div style="break-inside:avoid;height:${60 + (i % 7) * 37}px;margin-bottom:12px">box ${i}<br>${PAPER_CUT_WORDS(12)}</div>`;
const PAPER_CUT = {
  text: {
    what: 'paper-cut print of about 20 Letter pages of plain lines: every line a span for the widows and orphans check',
    html: `<div style="width:400px">${PAPER_CUT_WORDS(4950)}</div>`,
    pages: 20,
  },
  avoid: {
    what: 'paper-cut print of about 20 Letter pages of break-inside:avoid boxes of mixed height',
    html: Array.from({ length: 94 }, (_, i) => PAPER_CUT_BOX(i)).join(''),
    pages: 20,
  },
  shrink: {
    what: 'paper-cut print of about 11 Letter pages of a 1200 px wide document: overflow at the paper width, so one more layout, shrunk to 2/3',
    html: `<div style="width:1200px">${PAPER_CUT_WORDS(9750)}</div>`,
    pages: 11,
  },
};
const paperCutScenario = ([key, { what, html, pages }]) => ({
  name: `pdf.paginate.${key}`,
  group: 'pdf',
  what,
  iterations: 12,
  setup: async (env) => {
    const page = await newPage(env.cdp);
    const sid = page.sessionId;
    const { frameTree } = await env.cdp.send('Page.getFrameTree', {}, sid);
    await env.cdp.send(
      'Page.setDocumentContent',
      { frameId: frameTree.frame.id, html: `<!doctype html><body style="margin:0;font:16px/20px Arial">${html}` },
      sid,
    );
    return { page };
  },
  run: async (s, i, env) => {
    const { data } = await env.cdp.send(
      'Page.printToPDF',
      { paperWidth: 8.5, paperHeight: 11, marginTop: 0.4, marginBottom: 0.4, marginLeft: 0.4, marginRight: 0.4 },
      s.page.sessionId,
    );
    const got = Buffer.from(data, 'base64').toString('latin1').match(/\/Type\s*\/Page\b/g)?.length;
    // Not exact: comparing versions is the point, and pagination rules change the count.
    assert(Math.abs(got - pages) <= 2, `${got} pages, not about ${pages}`);
  },
});

// Each scenario: name, group, what it measures, iterations (before --scale), optional opsPerSample,
// setup(env) -> state, run(state, i) -> optional phase timings, check(state) after all iterations.
// env = { wsUrl, site, cdp (one browser-level connection, raw-CDP scenarios only), playwright, WebView }.
const SCENARIOS = [
  {
    name: 'connect.ws+getVersion',
    group: 'connection',
    what: 'New WebSocket (spawns a browser thread and JSC engine) + Browser.getVersion + close',
    iterations: 20,
    run: async (s, i, env) => {
      const c = await Cdp.open(env.wsUrl);
      const v = await c.send('Browser.getVersion');
      assert(v.product, 'Browser.getVersion product');
      // Close is part of the cost: the server tears the engine down before the next connect.
      await c.close();
    },
  },
  {
    name: 'rtt.Browser.getVersion',
    group: 'protocol',
    what: 'Browser-level no-op round trip: floor for WebSocket + dispatch overhead',
    iterations: 500,
    run: async (s, i, env) => {
      await env.cdp.send('Browser.getVersion');
    },
  },
  {
    name: 'rtt.Runtime.evaluate',
    group: 'protocol',
    what: 'Runtime.evaluate "1+1" by value on a page session (browser thread hop + JSC eval)',
    iterations: 500,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => {
      assert((await evaluate(env.cdp, s.page.sessionId, '1+1')) === 2, '1+1');
    },
  },
  {
    name: 'throughput.evaluate.pipelined256',
    group: 'protocol',
    what: '256 Runtime.evaluate commands in flight at once; one sample = one batch',
    iterations: 20,
    opsPerSample: 256,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => {
      const all = [];
      for (let k = 0; k < 256; k++) all.push(env.cdp.send('Runtime.evaluate', { expression: `${k}+1`, returnByValue: true }, s.page.sessionId));
      const rs = await Promise.all(all);
      assert(rs[255].result.value === 256, 'pipelined result');
    },
  },
  {
    name: 'runtime.evaluate.largeValue',
    group: 'runtime',
    what: 'Return 1000 small objects by value (serialization of the result)',
    iterations: 100,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => {
      const v = await evaluate(env.cdp, s.page.sessionId, 'Array.from({length:1000},(_, i)=>({i, s:"item"+i, f:i/3}))');
      assert(v.length === 1000, 'array length');
    },
  },
  {
    name: 'runtime.callFunctionOn.small',
    group: 'runtime',
    what: 'callFunctionOn with a tiny function on a handle, by value',
    iterations: 300,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      return { page, obj: await objectId(env.cdp, page.sessionId, 'globalThis') };
    },
    run: async (s, i, env) => {
      const r = await env.cdp.send(
        'Runtime.callFunctionOn',
        { functionDeclaration: 'function(a){return a+1}', objectId: s.obj, arguments: [{ value: i }], returnByValue: true },
        s.page.sessionId,
      );
      assert(r.result.value === i + 1, 'callFunctionOn result');
    },
  },
  {
    name: 'runtime.callFunctionOn.38KB',
    group: 'runtime',
    what: 'callFunctionOn with a ~38 KB function, as Playwright re-sends its utilities',
    iterations: 200,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      return { page, obj: await objectId(env.cdp, page.sessionId, 'globalThis') };
    },
    run: async (s, i, env) => {
      const r = await env.cdp.send(
        'Runtime.callFunctionOn',
        { functionDeclaration: BIG_FN, objectId: s.obj, arguments: [{ value: 1 }], returnByValue: true },
        s.page.sessionId,
      );
      assert(r.result.value === 2, 'callFunctionOn result');
    },
  },
  {
    name: 'runtime.handle+release',
    group: 'runtime',
    what: 'Create an object handle, then Runtime.releaseObject (remote object registry churn)',
    iterations: 300,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => {
      const id = await objectId(env.cdp, s.page.sessionId, `({a:${i}})`);
      await env.cdp.send('Runtime.releaseObject', { objectId: id }, s.page.sessionId);
    },
  },
  {
    name: 'dom.querySelectorAll.2k',
    group: 'dom',
    what: 'Selector query over a 2000-row document, count by value',
    iterations: 200,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await navigate(env.cdp, page.sessionId, `${env.site}/dom?n=2000`);
      return { page };
    },
    run: async (s, i, env) => {
      const n = await evaluate(env.cdp, s.page.sessionId, `document.querySelectorAll('.item[data-k="3"] span').length`);
      assert(n === 286, `query count ${n}`);
    },
  },
  {
    name: 'dom.textContent.2k',
    group: 'dom',
    what: 'Read document.body.textContent of a 2000-row document (tree walk + string building)',
    iterations: 100,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await navigate(env.cdp, page.sessionId, `${env.site}/dom?n=2000`);
      return { page };
    },
    run: async (s, i, env) => {
      const n = await evaluate(env.cdp, s.page.sessionId, 'document.body.textContent.length');
      assert(n > 20000, `textContent length ${n}`);
    },
  },
  {
    name: 'dom.mutate.1k',
    group: 'dom',
    what: 'Create and append 1000 elements, then remove them',
    iterations: 50,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await navigate(env.cdp, page.sessionId, `${env.site}/simple`);
      return { page };
    },
    run: async (s, i, env) => {
      const n = await evaluate(
        env.cdp,
        s.page.sessionId,
        `(() => { const box = document.createElement('div'); document.body.appendChild(box);
          for (let i = 0; i < 1000; i++) { const e = document.createElement('p'); e.className = 'x' + (i % 10); e.textContent = 'p' + i; box.appendChild(e); }
          const n = box.childNodes.length; box.remove(); return n; })()`,
      );
      assert(n === 1000, 'mutate count');
    },
  },
  {
    name: 'dom.handle.describe+quads',
    group: 'dom',
    what: 'Playwright click preparation: element handle, DOM.describeNode, DOM.getContentQuads, release',
    iterations: 200,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await navigate(env.cdp, page.sessionId, `${env.site}/dom?n=2000`);
      return { page };
    },
    run: async (s, i, env) => {
      const sid = s.page.sessionId;
      const id = await objectId(env.cdp, sid, `document.getElementById('i${wrap(i, 2000)}')`);
      const d = await env.cdp.send('DOM.describeNode', { objectId: id }, sid);
      assert(d.node?.nodeName === 'DIV', 'describeNode');
      const q = await env.cdp.send('DOM.getContentQuads', { objectId: id }, sid);
      assert(q.quads?.length, 'quads');
      await env.cdp.send('Runtime.releaseObject', { objectId: id }, sid);
    },
  },
  {
    name: 'page.navigate.about:blank',
    group: 'navigation',
    what: 'Navigate to about:blank and wait for load (document + realm rebuild, no network)',
    iterations: 100,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => navigate(env.cdp, s.page.sessionId, 'about:blank'),
  },
  {
    name: 'page.navigate.simple',
    group: 'navigation',
    what: 'Navigate to a small local HTTP page and wait for load',
    iterations: 50,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => navigate(env.cdp, s.page.sessionId, `${env.site}/simple`),
  },
  {
    name: 'page.navigate.scripts',
    group: 'navigation',
    what: 'Page with 10 external scripts and 5 stylesheets; wait for load',
    iterations: 30,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => {
      const phases = await navigate(env.cdp, s.page.sessionId, `${env.site}/scripts`);
      assert((await evaluate(env.cdp, s.page.sessionId, 'window.__loaded')) === 10, 'all scripts ran');
      return phases;
    },
  },
  {
    name: 'page.navigate.dom2k',
    group: 'navigation',
    what: 'Navigate to a 2000-row document (HTML parse + tree build); wait for load',
    iterations: 30,
    setup: async (env) => ({ page: await newPage(env.cdp) }),
    run: async (s, i, env) => navigate(env.cdp, s.page.sessionId, `${env.site}/dom?n=2000`),
  },
  {
    name: 'target.lifecycle',
    group: 'targets',
    what: 'createBrowserContext + createTarget + attach + Page/Runtime.enable + close + dispose',
    iterations: 30,
    run: async (s, i, env) => {
      const c = env.cdp;
      const t0 = now();
      const phases = {};
      const lap = (k) => {
        phases[k] = now() - t0;
      };
      const { browserContextId } = await c.send('Target.createBrowserContext');
      lap('createBrowserContext');
      const { targetId } = await c.send('Target.createTarget', { url: 'about:blank', browserContextId });
      lap('createTarget');
      const { sessionId } = await c.send('Target.attachToTarget', { targetId, flatten: true });
      lap('attachToTarget');
      await c.send('Page.enable', {}, sessionId);
      await c.send('Runtime.enable', {}, sessionId);
      await c.send('Page.getFrameTree', {}, sessionId);
      lap('enable');
      await c.send('Target.closeTarget', { targetId });
      lap('closeTarget');
      await c.send('Target.disposeBrowserContext', { browserContextId });
      lap('disposeBrowserContext');
      return phases;
    },
  },
  {
    name: 'network.fetch20',
    group: 'network',
    what: 'Page runs 20 parallel fetch() calls with Network.enable (request + event emission)',
    iterations: 30,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await env.cdp.send('Network.enable', {}, page.sessionId);
      await navigate(env.cdp, page.sessionId, `${env.site}/simple`);
      return { page };
    },
    run: async (s, i, env) => {
      const n = await evaluate(
        env.cdp,
        s.page.sessionId,
        `Promise.all(Array.from({length:20},(_, k)=>fetch('/api/data.json?name=' + k).then(r=>r.json()))).then(a=>a.length)`,
        { awaitPromise: true },
      );
      assert(n === 20, 'fetch count');
    },
  },
  {
    name: 'fetch.intercept.fulfill',
    group: 'network',
    what: 'Fetch.requestPaused -> Fetch.fulfillRequest round trip for one page fetch()',
    iterations: 50,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      await navigate(env.cdp, page.sessionId, `${env.site}/form`);
      await env.cdp.send('Fetch.enable', { patterns: [{ urlPattern: '*/api/routed.json*' }] }, page.sessionId);
      return { page, body: Buffer.from(JSON.stringify({ source: 'fulfilled' })).toString('base64') };
    },
    run: async (s, i, env) => {
      const sid = s.page.sessionId;
      const paused = env.cdp.waitFor('Fetch.requestPaused', { sessionId: sid }).then((p) =>
        env.cdp.send(
          'Fetch.fulfillRequest',
          { requestId: p.requestId, responseCode: 200, responseHeaders: [{ name: 'content-type', value: 'application/json' }], body: s.body },
          sid,
        ),
      );
      const v = await evaluate(env.cdp, sid, `fetch('/api/routed.json?i=${i}').then(r=>r.json()).then(j=>j.source)`, { awaitPromise: true });
      await paused;
      assert(v === 'fulfilled', `intercepted body ${v}`);
    },
  },
  {
    name: 'input.mouse.click',
    group: 'input',
    what: 'Input.dispatchMouseEvent pressed + released on a button with a click listener',
    iterations: 200,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      const sid = page.sessionId;
      await navigate(env.cdp, sid, `${env.site}/click`);
      const id = await objectId(env.cdp, sid, `document.getElementById('b')`);
      const { quads } = await env.cdp.send('DOM.getContentQuads', { objectId: id }, sid);
      const q = quads[0];
      return { page, x: (q[0] + q[2] + q[4] + q[6]) / 4, y: (q[1] + q[3] + q[5] + q[7]) / 4, n: 0 };
    },
    run: async (s, i, env) => {
      const sid = s.page.sessionId;
      const base = { x: s.x, y: s.y, button: 'left', clickCount: 1 };
      await env.cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...base }, sid);
      await env.cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...base }, sid);
      s.n++;
    },
    check: async (s, env) => {
      const clicks = await evaluate(env.cdp, s.page.sessionId, 'window.__clicks');
      assert(clicks === s.n, `clicks ${clicks} != ${s.n}`);
    },
  },
  {
    name: 'input.key.type',
    group: 'input',
    what: 'Input.dispatchKeyEvent keyDown(text) + keyUp into a focused input',
    iterations: 200,
    setup: async (env) => {
      const page = await newPage(env.cdp);
      const sid = page.sessionId;
      await navigate(env.cdp, sid, `${env.site}/click`);
      await evaluate(env.cdp, sid, `document.getElementById('t').focus()`);
      return { page, n: 0 };
    },
    run: async (s, i, env) => {
      const sid = s.page.sessionId;
      await env.cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'a', code: 'KeyA', text: 'a', windowsVirtualKeyCode: 65 }, sid);
      await env.cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'a', code: 'KeyA', windowsVirtualKeyCode: 65 }, sid);
      s.n++;
    },
    check: async (s, env) => {
      const len = await evaluate(env.cdp, s.page.sessionId, `document.getElementById('t').value.length`);
      assert(len === s.n, `typed ${len} != ${s.n}`);
    },
  },
  ...Object.entries(PAPER_CUT).map(paperCutScenario),

  // ---- Bun.WebView (Chrome backend over CDP; needs `bun`, which this script then runs under) ----
  {
    name: 'wv.open+navigate+close',
    group: 'webview',
    what: 'new Bun.WebView on the endpoint, navigate a small page, close (client attach cost)',
    iterations: 20,
    webview: true,
    run: async (s, i, env) => {
      const view = wvOpen(env);
      try {
        await view.navigate(`${env.site}/simple`);
      } finally {
        await view.close();
      }
    },
  },
  {
    name: 'wv.navigate.simple',
    group: 'webview',
    what: 'view.navigate of a small page (resolves on load)',
    iterations: 50,
    webview: true,
    setup: wvSetup('about:blank'),
    run: async (s, i, env) => {
      await s.view.navigate(`${env.site}/simple`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.navigate.scripts',
    group: 'webview',
    what: 'view.navigate of the 10-script / 5-stylesheet page',
    iterations: 30,
    webview: true,
    setup: wvSetup('about:blank'),
    run: async (s, i, env) => {
      await s.view.navigate(`${env.site}/scripts`);
      assert((await s.view.evaluate('window.__loaded')) === 10, 'all scripts ran');
    },
    teardown: wvClose,
  },
  {
    name: 'wv.navigate.dom2k',
    group: 'webview',
    what: 'view.navigate of a 2000-row document',
    iterations: 30,
    webview: true,
    setup: wvSetup('about:blank'),
    run: async (s, i, env) => {
      await s.view.navigate(`${env.site}/dom?n=2000`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.evaluate',
    group: 'webview',
    what: 'view.evaluate of a small expression',
    iterations: 300,
    webview: true,
    setup: wvSetup('/simple'),
    run: async (s, i) => {
      assert((await s.view.evaluate(`${i}+1`)) === i + 1, 'evaluate');
    },
    teardown: wvClose,
  },
  {
    name: 'wv.evaluate.query2k',
    group: 'webview',
    what: 'view.evaluate of a selector query over a 2000-row document',
    iterations: 200,
    webview: true,
    setup: wvSetup('/dom?n=2000'),
    run: async (s) => {
      const n = await s.view.evaluate(`document.querySelectorAll('.item[data-k="3"] span').length`);
      assert(n === 286, `query count ${n}`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.cdp.Runtime.evaluate',
    group: 'webview',
    what: 'view.cdp("Runtime.evaluate") raw round trip through Bun',
    iterations: 300,
    webview: true,
    setup: wvSetup('/simple'),
    run: async (s) => {
      const r = await s.view.cdp('Runtime.evaluate', { expression: '1+1', returnByValue: true });
      assert(r.result.value === 2, 'cdp evaluate');
    },
    teardown: wvClose,
  },
  {
    name: 'wv.click.selector',
    group: 'webview',
    what: 'view.click("#b"): actionability checks, then a real mouse click',
    iterations: 100,
    webview: true,
    setup: async (env) => ({ ...(await wvSetup('/click')(env)), n: 0 }),
    run: async (s) => {
      await s.view.click('#b');
      s.n++;
    },
    check: async (s) => {
      const clicks = await s.view.evaluate('window.__clicks');
      assert(clicks === s.n, `clicks ${clicks} != ${s.n}`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.type',
    group: 'webview',
    what: 'view.type of 10 characters into a focused input',
    iterations: 100,
    webview: true,
    setup: async (env) => {
      const s = await wvSetup('/click')(env);
      await s.view.evaluate(`document.getElementById('t').focus()`);
      return { ...s, n: 0 };
    },
    run: async (s) => {
      await s.view.type('abcdefghij');
      s.n += 10;
    },
    check: async (s) => {
      const len = await s.view.evaluate(`document.getElementById('t').value.length`);
      assert(len === s.n, `typed ${len} != ${s.n}`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.flow.e2e',
    group: 'webview',
    what: 'Whole test: open view, navigate form, click + type name, click submit, wait for result, close',
    iterations: 15,
    webview: true,
    run: async (s, i, env) => {
      const t0 = now();
      const phases = {};
      const lap = (k) => {
        phases[k] = now() - t0;
      };
      const view = wvOpen(env);
      try {
        await view.navigate(`${env.site}/form`);
        lap('navigate');
        await view.click('#name');
        await view.type(`w${i}`);
        await view.click('#submit');
        lap('input');
        await wvPollText(view, 'out', `Hello, w${i}`);
        lap('result');
      } finally {
        await view.close();
      }
      lap('close');
      return phases;
    },
  },

  // ---- LP.* native driver through the same Bun.WebView client (view.cdp) ----
  // One command per action: locators, actionability and waiting run in the browser.
  {
    name: 'wv.lp.click',
    group: 'webview',
    what: 'LP.act click on "#b" through view.cdp (compare wv.click.selector)',
    iterations: 100,
    webview: true,
    setup: async (env) => ({ ...(await wvSetup('/click')(env)), n: 0 }),
    run: async (s) => {
      await s.view.cdp('LP.act', { locator: { css: '#b' }, action: 'click' });
      s.n++;
    },
    check: async (s) => {
      const clicks = await s.view.evaluate('window.__clicks');
      assert(clicks === s.n, `clicks ${clicks} != ${s.n}`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.lp.fill',
    group: 'webview',
    what: 'LP.act fill of 10 characters into "#t" (compare wv.type, which needs a focused input)',
    iterations: 100,
    webview: true,
    setup: wvSetup('/click'),
    run: async (s, i) => {
      await s.view.cdp('LP.act', { locator: { css: '#t' }, action: 'fill', value: `abcdefg${String(i).padStart(3, '0')}` });
    },
    check: async (s) => {
      const len = await s.view.evaluate(`document.getElementById('t').value.length`);
      assert(len === 10, `value length ${len}`);
    },
    teardown: wvClose,
  },
  {
    name: 'wv.lp.flow.e2e',
    group: 'webview',
    what: 'wv.flow.e2e on LP.*: open view, navigate form, fill name, click submit, LP.expect the result, close',
    iterations: 15,
    webview: true,
    run: async (s, i, env) => lpFlow(env, i, false, false),
  },
  // Bun.WebView's view.cdp() allows one call in flight ("a cdp() is already pending"),
  // so the pipelined flow runs on the raw CDP client, next to a sequential twin.
  {
    name: 'lp.flow.e2e',
    group: 'webview',
    what: 'wv.lp.flow.e2e on the raw CDP client: new page, navigate form, LP.act fill + click, LP.expect, close',
    iterations: 15,
    run: async (s, i, env) => cdpLpFlow(env, i, false),
  },
  {
    name: 'lp.flow.pipelined',
    group: 'webview',
    what: 'lp.flow.e2e with fill, click and expect sent at once (one wait for all three)',
    iterations: 15,
    run: async (s, i, env) => cdpLpFlow(env, i, true),
  },
  {
    name: 'lp.run.steps',
    group: 'webview',
    what: 'lp.flow.e2e with fill, click and expect as one LP.run (click settles on its fetch)',
    iterations: 15,
    run: async (s, i, env) => cdpRunFlow(env, i, false),
  },
  {
    name: 'lp.run.flow',
    group: 'webview',
    what: 'lp.run.steps with the navigation as a goto step too: new page, one LP.run, close',
    iterations: 15,
    run: async (s, i, env) => cdpRunFlow(env, i, true),
  },
  {
    name: 'wv.lp.run.flow',
    group: 'webview',
    what: 'wv.lp.flow.e2e with fill, click and expect as one LP.run: open view, navigate form, LP.run, close',
    iterations: 15,
    webview: true,
    run: async (s, i, env) => {
      const t0 = now();
      const phases = {};
      const view = wvOpen(env);
      try {
        await view.navigate(`${env.site}/form`);
        phases.navigate = now() - t0;
        await view.cdp('LP.run', { steps: formSteps(i) });
        phases.result = now() - t0;
      } finally {
        await view.close();
      }
      phases.close = now() - t0;
      return phases;
    },
  },
  {
    name: 'wv.lp.flow.role',
    group: 'webview',
    what: 'wv.lp.flow.e2e with label / role / text locators instead of CSS ids',
    iterations: 15,
    webview: true,
    run: async (s, i, env) => lpFlow(env, i, false, true),
  },
  ...[
    ['css', { css: '.item[data-k="3"] span' }, 286],
    ['testId', { testId: '3', testIdAttribute: 'data-k' }, 286],
    ['text', { text: 'row 1999', exact: true }, 1],
    ['role', { role: 'link', name: 'link' }, 2000],
  ].map(([kind, locator, want]) => ({
    name: `wv.lp.locate.${kind}`,
    group: 'webview',
    what: `LP.query count of a ${kind} locator over a 2000-row document (compare wv.evaluate.query2k)`,
    iterations: 100,
    webview: true,
    setup: wvSetup('/dom?n=2000'),
    run: async (s) => {
      const r = await s.view.cdp('LP.query', { locator, read: 'count' });
      assert(r.value === want, `${kind} count ${r.value} != ${want}`);
    },
    teardown: wvClose,
  })),

  // ---- TOS positions-close on the lightningpanda-test client (Bun only: the client is TypeScript) ----
  {
    name: 'tos.close.steps',
    group: 'tos',
    what: 'TOS positions-close, one LP.run per step: seed, new signed-in page, navigate to the overview, 8 steps, close',
    iterations: 15,
    setup: tosSetup(false),
    run: async (s, i, env) => tosClose(s, env, 'steps'),
    teardown: tosTeardown,
  },
  {
    name: 'tos.close.run.fresh',
    group: 'tos',
    what: 'tos.close.steps as one LP.run (navigation included), still on a page booted for the test',
    iterations: 15,
    setup: tosSetup(false),
    run: async (s, i, env) => tosClose(s, env, 'run'),
    teardown: tosTeardown,
  },
  {
    name: 'tos.close.run',
    group: 'tos',
    what: 'tos.close.run.fresh on a pre-booted signed-in page: lp.page({ as, route }) + one LP.run (untimed: wait for a booted page)',
    iterations: 15,
    setup: tosSetup(true),
    before: (s) => s.lp.warm('employee'),
    run: async (s, i, env) => tosClose(s, env, 'run'),
    teardown: tosTeardown,
  },
  {
    name: 'tos.close.run.busy',
    group: 'tos',
    what: 'tos.close.run back to back on one connection: its pool refills when the test closes its page, next to the next test',
    iterations: 15,
    setup: tosSetup(true),
    run: async (s, i, env) => tosClose(s, env, 'run'),
    teardown: tosTeardown,
  },
  {
    name: 'tos.close.run.conn4',
    group: 'tos',
    what: 'tos.close.run.busy over 4 connections (browser threads): each refills while the others run tests',
    iterations: 15,
    setup: tosSetup(true, 4),
    run: async (s, i, env) => tosClose(s, env, 'run'),
    teardown: tosTeardown,
  },

  // ---- Playwright (opt-in with --playwright; playwright-core pinned in bench/package.json) ----
  {
    name: 'pw.connectOverCDP',
    group: 'playwright',
    what: 'chromium.connectOverCDP + browser.close (Playwright attach handshake)',
    iterations: 10,
    playwright: true,
    run: async (s, i, env) => {
      const b = await env.playwright.chromium.connectOverCDP(env.wsUrl, { timeout: 10_000 });
      await b.close();
    },
  },
  {
    name: 'pw.newContext+newPage+close',
    group: 'playwright',
    what: 'browser.newContext + newPage + context.close',
    iterations: 20,
    playwright: true,
    setup: pwConnect,
    run: async (s) => {
      const ctx = await s.browser.newContext();
      await ctx.newPage();
      await ctx.close();
    },
    teardown: pwClose,
  },
  {
    name: 'pw.tos.close',
    group: 'tos',
    what: "TOS positions-close as TOS writes it: Playwright context with the token, goto, locators, waitForResponse",
    iterations: 15,
    playwright: true,
    setup: pwConnect,
    run: async (s, i, env) => {
      const t0 = now();
      const phases = {};
      const p = await tosSeed(env);
      phases.seed = now() - t0;
      const ctx = await s.browser.newContext({ viewport: { width: 1440, height: 900 } });
      await ctx.addInitScript(() => localStorage.setItem('auth-token', 'e2e-token'));
      const page = await ctx.newPage();
      phases.page = now() - t0;
      const isGet = (r) => r.request().method() === 'GET' && r.url().endsWith(`/tos-api/positions/${p.id}`);
      try {
        const details = page.waitForResponse(isGet);
        await page.goto(`${env.site}${tosOverview(p.id)}`);
        await details;
        await page.getByText(p.number).waitFor();
        phases.open = now() - t0;
        await page.getByRole('button', { name: 'Settings', exact: true }).press('Enter');
        await page.getByRole('button', { name: 'Close Position' }).filter({ visible: true }).first().click();
        const dialog = page.getByRole('dialog').filter({ hasText: 'This position will be closed' }).last();
        await dialog.waitFor();
        const closed = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/close') && r.status() === 200);
        const refresh = page.waitForResponse(isGet);
        await dialog.getByRole('button', { name: 'Close Position', exact: true }).click();
        await Promise.all([closed, refresh]);
        await page.getByText(/^Position Closed · .* · Manual · Closed by:/).filter({ visible: true }).waitFor();
        await page.getByRole('button', { name: 'Reopen Position' }).waitFor();
        phases.run = now() - t0;
      } finally {
        await ctx.close();
      }
      phases.close = now() - t0;
      return phases;
    },
    teardown: pwClose,
  },
  {
    name: 'pw.goto.simple',
    group: 'playwright',
    what: "page.goto of a small page, waitUntil 'load'",
    iterations: 30,
    playwright: true,
    setup: pwPage,
    run: async (s, i, env) => {
      const r = await s.page.goto(`${env.site}/simple`, { waitUntil: 'load' });
      assert(r?.status() === 200, 'goto status');
    },
    teardown: pwClose,
  },
  {
    name: 'pw.goto.scripts',
    group: 'playwright',
    what: "page.goto of the 10-script / 5-stylesheet page, waitUntil 'load'",
    iterations: 20,
    playwright: true,
    setup: pwPage,
    run: async (s, i, env) => {
      await s.page.goto(`${env.site}/scripts`, { waitUntil: 'load' });
    },
    teardown: pwClose,
  },
  {
    name: 'pw.evaluate',
    group: 'playwright',
    what: 'page.evaluate of a small function',
    iterations: 200,
    playwright: true,
    setup: async (env) => {
      const s = await pwPage(env);
      await s.page.goto(`${env.site}/simple`);
      return s;
    },
    run: async (s, i) => {
      assert((await s.page.evaluate((x) => x + 1, i)) === i + 1, 'evaluate');
    },
    teardown: pwClose,
  },
  {
    name: 'pw.locator.textContent.2k',
    group: 'playwright',
    what: 'locator(css).textContent on a 2000-row page (injected script + selector engine)',
    iterations: 100,
    playwright: true,
    setup: async (env) => {
      const s = await pwPage(env);
      await s.page.goto(`${env.site}/dom?n=2000`);
      return s;
    },
    run: async (s, i) => {
      const k = wrap(i * 37, 2000);
      assert((await s.page.locator(`#i${k} span`).textContent()) === `row ${k}`, 'textContent');
    },
    teardown: pwClose,
  },
  {
    name: 'pw.fill+click+response',
    group: 'playwright',
    what: 'locator.fill, click submit, wait for the fetch response and the DOM update',
    iterations: 30,
    playwright: true,
    setup: async (env) => {
      const s = await pwPage(env);
      await s.page.goto(`${env.site}/form`);
      return s;
    },
    run: async (s, i) => formRoundTrip(s.page, `n${i}`),
    teardown: pwClose,
  },
  {
    name: 'pw.route.fulfill',
    group: 'playwright',
    what: 'page.route fulfill of a fetch triggered by a click',
    iterations: 30,
    playwright: true,
    setup: async (env) => {
      const s = await pwPage(env);
      await s.page.goto(`${env.site}/form`);
      await s.page.route('**/api/routed.json', (route) =>
        route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ source: 'fulfilled' }) }),
      );
      return s;
    },
    run: async (s) => {
      await s.page.evaluate(() => {
        document.getElementById('routed-out').textContent = '';
      });
      await Promise.all([s.page.waitForResponse((r) => r.url().includes('/api/routed.json')), s.page.locator('#routed').click()]);
      await pollText(s.page, 'routed-out', 'fulfilled');
    },
    teardown: pwClose,
  },
  {
    name: 'pw.flow.e2e',
    group: 'playwright',
    what: 'Whole test: newContext, newPage, goto form, fill, click, wait for result, close context',
    iterations: 15,
    playwright: true,
    setup: pwConnect,
    run: async (s, i, env) => {
      const t0 = now();
      const phases = {};
      const lap = (k) => {
        phases[k] = now() - t0;
      };
      const ctx = await s.browser.newContext();
      const page = await ctx.newPage();
      page.setDefaultTimeout(10_000);
      lap('newPage');
      await page.goto(`${env.site}/form`, { waitUntil: 'load' });
      lap('goto');
      await formRoundTrip(page, `e${i}`);
      lap('formRoundTrip');
      await ctx.close();
      lap('close');
      return phases;
    },
    teardown: pwClose,
  },
];

function wvOpen(env) {
  return new env.WebView({ width: 1280, height: 720, backend: { type: 'chrome', url: env.wsUrl } });
}
// A view already on `path` (or about:blank); Bun.WebView needs one navigate before cdp() works.
function wvSetup(path) {
  return async (env) => {
    const view = wvOpen(env);
    await view.navigate(path === 'about:blank' ? path : `${env.site}${path}`);
    return { view };
  };
}
async function wvClose(s) {
  await s.view?.close();
}
// The form flow on LP.*: fill, click, then one LP.expect that the browser answers
// when the fetch response has updated the DOM (no client polling).
async function lpFlow(env, i, pipelined, semantic) {
  const t0 = now();
  const phases = {};
  const lap = (k) => {
    phases[k] = now() - t0;
  };
  const name = semantic ? { label: 'Name' } : { css: '#name' };
  const submit = semantic ? { role: 'button', name: 'Send' } : { css: '#submit' };
  const out = semantic ? { text: `Hello, w${i}`, exact: true } : { css: '#out' };
  const view = wvOpen(env);
  try {
    await view.navigate(`${env.site}/form`);
    lap('navigate');
    const fill = view.cdp('LP.act', { locator: name, action: 'fill', value: `w${i}` });
    if (!pipelined) await fill;
    const click = view.cdp('LP.act', { locator: submit, action: 'click' });
    if (!pipelined) await click;
    lap('input');
    const expect = semantic
      ? view.cdp('LP.expect', { locator: out, assert: 'visible' })
      : view.cdp('LP.expect', { locator: out, assert: 'text', expected: `Hello, w${i}` });
    await Promise.all([fill, click, expect]);
    lap('result');
  } finally {
    await view.close();
  }
  lap('close');
  return phases;
}
// lpFlow on the raw CDP client, where commands can be in flight together.
async function cdpLpFlow(env, i, pipelined) {
  const t0 = now();
  const phases = {};
  const lap = (k) => {
    phases[k] = now() - t0;
  };
  const c = env.cdp;
  const page = await newPage(c);
  try {
    await navigate(c, page.sessionId, `${env.site}/form`);
    lap('navigate');
    const send = (method, params) => c.send(method, params, page.sessionId);
    const fill = send('LP.act', { locator: { css: '#name' }, action: 'fill', value: `w${i}` });
    if (!pipelined) await fill;
    const click = send('LP.act', { locator: { css: '#submit' }, action: 'click' });
    if (!pipelined) await click;
    lap('input');
    const expect = send('LP.expect', { locator: { css: '#out' }, assert: 'text', expected: `Hello, w${i}` });
    await Promise.all([fill, click, expect]);
    lap('result');
  } finally {
    await closePage(c, page);
  }
  lap('close');
  return phases;
}
// ---------- TOS positions-close (lightningpanda-test) ----------
const TOS_AUTH = { authorization: 'Bearer e2e-token' };
const tosOverview = (id) => `/tos/platform/logistics/positions/${id}/overview`;
async function tosSeed(env) {
  const r = await fetch(`${env.site}/tos-api/positions`, { method: 'POST', headers: TOS_AUTH });
  assert(r.status === 201, `seed status ${r.status}`);
  return r.json();
}
// The steps of TOS's 'closes an open Position from the Position overview' (see
// packages/lightningpanda-test/examples/positions-close.test.ts).
function tosCloseSteps(t, p) {
  const { check, button, dialog, text } = t;
  return [
    check(text(p.number)).visible(),
    button('Settings', { exact: true }).press('Enter'),
    button('Close Position').visible().first().click(),
    dialog().hasText('This position will be closed').last().button('Close Position', { exact: true }).click(),
    check.request('POST', `/tos-api/positions/${p.id}/close`).status(200),
    check.request('GET', `/tos-api/positions/${p.id}`).status(200),
    check(text(/^Position Closed · .* · Manual · Closed by:/).visible()).count(1),
    check(button('Reopen Position')).count(1),
  ];
}
function tosSetup(prepared, connections = 1) {
  return async (env) => {
    if (!globalThis.Bun) throw new Error('needs bun (the lightningpanda-test client is TypeScript)');
    const t = await import('../../packages/lightningpanda-test/src/index.ts');
    const lp = await t.Lightningpanda.connect(env.wsUrl, { connections });
    await lp.profile('employee', {
      url: `${env.site}/tos/platform`,
      localStorage: { 'auth-token': 'e2e-token' },
      viewport: { width: 1440, height: 900 },
      pool: 2,
      prepare: prepared,
    });
    if (prepared) await sleep(300); // let the first pages boot
    return { t, lp, prepared };
  };
}
async function tosTeardown(s) {
  if (s.lp) console.error(`      lightningpanda-test pages: ${JSON.stringify(s.lp.stats)}`);
  s.lp?.close();
}
async function tosClose(s, env, mode) {
  const t0 = now();
  const phases = {};
  const lap = (k) => {
    phases[k] = now() - t0;
  };
  const p = await tosSeed(env);
  lap('seed');
  // A pre-booted page changes route in place; a fresh one navigates to the overview.
  const page = await s.lp.page(
    s.prepared ? { as: 'employee', route: tosOverview(p.id) } : { as: 'employee', url: `${env.site}${tosOverview(p.id)}` },
  );
  lap('page');
  const steps = tosCloseSteps(s.t, p);
  try {
    if (mode === 'steps') {
      await page.run([]); // the navigation
      lap('open');
      // A request check sees only its run's log, so it goes with the action that sent the request.
      const runs = [];
      for (const step of steps) {
        if (step.toStep().request && runs.length) runs.at(-1).push(step);
        else runs.push([step]);
      }
      for (const run of runs) await page.run(run);
    } else {
      const r = await page.run(steps);
      assert(r.requests.some((q) => q.method === 'POST' && q.url.endsWith('/close')), 'POST close in the log');
    }
    lap('run');
  } finally {
    page.close();
  }
  lap('close');
  return phases;
}

// The form flow as LP.run steps.
function formSteps(i) {
  return [
    { act: { locator: { css: '#name' }, action: 'fill', value: `w${i}` } },
    { act: { locator: { css: '#submit' }, action: 'click' } },
    { check: { locator: { css: '#out' }, assert: 'text', expected: `Hello, w${i}` } },
  ];
}
// The form flow as one LP.run on the raw CDP client, with or without the navigation in it.
async function cdpRunFlow(env, i, withGoto) {
  const t0 = now();
  const phases = {};
  const lap = (k) => {
    phases[k] = now() - t0;
  };
  const c = env.cdp;
  const page = await newPage(c);
  try {
    lap('page');
    let steps = formSteps(i);
    if (withGoto) steps = [{ goto: { url: `${env.site}/form` } }, ...steps];
    else {
      await navigate(c, page.sessionId, `${env.site}/form`);
      lap('navigate');
    }
    const r = await c.send('LP.run', { steps }, page.sessionId);
    assert(r.results.length === steps.length, `LP.run results ${JSON.stringify(r)}`);
    lap('result');
  } finally {
    await closePage(c, page);
  }
  lap('close');
  return phases;
}
async function wvPollText(view, id, want) {
  let txt = '';
  for (let k = 0; k < 200; k++) {
    txt = await view.evaluate(`document.getElementById(${JSON.stringify(id)}).textContent`);
    if (txt === want) return;
    await sleep(2);
  }
  throw new Error(`#${id} = ${JSON.stringify(txt)}, want ${JSON.stringify(want)}`);
}

async function pwConnect(env) {
  return { browser: await env.playwright.chromium.connectOverCDP(env.wsUrl, { timeout: 10_000 }) };
}
async function pwPage(env) {
  const s = await pwConnect(env);
  s.context = await s.browser.newContext();
  s.page = await s.context.newPage();
  s.page.setDefaultTimeout(10_000);
  return s;
}
async function pwClose(s) {
  await s.browser?.close();
}
// fetch().then(json) can land just after the response event, so poll the DOM briefly.
async function pollText(page, id, want) {
  let txt = '';
  for (let k = 0; k < 200; k++) {
    txt = await page.evaluate((id) => document.getElementById(id).textContent, id);
    if (txt === want) return;
    await sleep(2);
  }
  throw new Error(`#${id} = ${JSON.stringify(txt)}, want ${JSON.stringify(want)}`);
}
async function formRoundTrip(page, name) {
  await page.locator('#name').fill(name);
  await Promise.all([page.waitForResponse((r) => r.url().includes('/api/data.json')), page.locator('#submit').click()]);
  await pollText(page, 'out', `Hello, ${name}`);
}

// ---------- process management ----------
async function startSite() {
  const cp = track(spawn(process.execPath, [path.join(HERE, 'cdp-perf-site.mjs'), '0'], { stdio: ['ignore', 'pipe', 'inherit'], detached: true }));
  const port = await new Promise((res, rej) => {
    let buf = '';
    cp.stdout.on('data', (d) => {
      buf += d;
      const m = buf.match(/LISTENING (\d+)/);
      if (m) res(Number(m[1]));
    });
    cp.once('exit', (c) => rej(new Error(`fixture site exited ${c}`)));
  });
  return { cp, url: `http://127.0.0.1:${port}` };
}

async function startBrowser(bin) {
  const port = await freePort();
  const t = now();
  const cp = track(spawn(bin.path, ['serve', '--host', '127.0.0.1', '--port', String(port), ...(FORK ? ['--fork'] : []), ...LP_FLAGS], { stdio: ['ignore', 'ignore', 'pipe'], env: LP_ENV, detached: true }));
  let stderr = '';
  cp.stderr.on('data', (d) => {
    stderr = (stderr + d).slice(-4000);
  });
  for (;;) {
    if (cp.exitCode !== null) throw new Error(`${bin.label} exited ${cp.exitCode} during startup: ${stderr}`);
    try {
      const r = await getJson(`http://127.0.0.1:${port}/json/version`);
      if (r.status === 200) {
        return { cp, port, startupMs: now() - t, version: JSON.parse(r.body), wsUrl: JSON.parse(r.body).webSocketDebuggerUrl ?? `ws://127.0.0.1:${port}/`, stderr: () => stderr };
      }
    } catch {}
    if (now() - t > 15_000) throw new Error(`${bin.label} not ready after 15 s: ${stderr}`);
    await sleep(2);
  }
}

async function stopBrowser(b) {
  killGroup(b.cp, 'SIGTERM');
  await withTimeout(exited(b.cp), 3000, 'browser exit').catch(() => killGroup(b.cp, 'SIGKILL'));
  await exited(b.cp);
}

// Browser CPU (user+system ms, all threads) and resident KiB, or null where unsupported.
const SAMPLER = opt('--sampler') ?? [path.join(HERE, '../rss-sampler/target/release/rss-sampler')].find((p) => fs.existsSync(p));
function cpuSnapshot(pid) {
  if (process.platform === 'linux') {
    // Nanoseconds on CPU per live thread; threads that already exited are not counted.
    try {
      let ns = 0;
      for (const t of fs.readdirSync(`/proc/${pid}/task`)) ns += Number(fs.readFileSync(`/proc/${pid}/task/${t}/schedstat`, 'utf8').split(' ')[0]);
      const rssPages = Number(fs.readFileSync(`/proc/${pid}/statm`, 'utf8').split(' ')[1]);
      return Promise.resolve({ cpuMs: ns / 1e6, rssKiB: rssPages * 4 });
    } catch {
      return Promise.resolve(null);
    }
  }
  if (!SAMPLER) return Promise.resolve(null);
  return new Promise((res) =>
    execFile(SAMPLER, [String(pid), '-', '--once', '--instructions'], (err, out) => {
      if (err) return res(null);
      const c = out.trim().split(/\s+/).map(Number);
      res({ cpuMs: (c[3] + c[4]) / 1e6, rssKiB: c[1], instructions: c[6] });
    }),
  );
}

async function browserSnapshot(pid) {
  if (!FORK) return cpuSnapshot(pid);
  // serve -> template -> one child per connection.
  const childrenOf = (p) => new Promise((res) => execFile('pgrep', ['-P', String(p)], (_, out) => res(String(out).trim().split(/\s+/).filter(Boolean))));
  const pids = [];
  for (let level = [pid]; level.length; ) {
    level = (await Promise.all(level.map(childrenOf))).flat();
    pids.push(...level);
  }
  const all = (await Promise.all(pids.map(cpuSnapshot))).filter(Boolean);
  if (!all.length) return null;
  return all.reduce((a, c) => ({ cpuMs: a.cpuMs + c.cpuMs, rssKiB: a.rssKiB + c.rssKiB, instructions: a.instructions + c.instructions }));
}

// ---------- one scenario on one binary ----------
async function runScenario(sc, bin, site, playwright) {
  const iterations = Math.max(3, Math.round(sc.iterations * SCALE));
  const warmup = Math.max(2, Math.round(iterations / 10));
  const b = await startBrowser(bin);
  const result = { startupMs: b.startupMs, samples: [], phases: {} };
  let cdp;
  let perfCdp;
  let state = {};
  const env = { wsUrl: b.wsUrl, site, playwright, WebView: globalThis.Bun?.WebView };
  try {
    await withTimeout(
      (async () => {
        if (!sc.playwright && !sc.webview) env.cdp = cdp = await Cdp.open(b.wsUrl);
        if (sc.setup) state = await sc.setup(env);
        for (let i = 0; i < warmup; i++) {
          await sc.before?.(state, -1 - i, env);
          await sc.run(state, -1 - i, env);
        }
        // Stage timers are process-wide, so one control connection sees every client's commands.
        const control = PERF_TRACE ? (cdp ?? (perfCdp = await Cdp.open(b.wsUrl))) : null;
        if (control) await control.send('LP.perfReset');
        const before = await browserSnapshot(b.cp.pid);
        const t0 = now();
        for (let i = 0; i < iterations; i++) {
          // Untimed per-sample preparation (e.g. waiting for a pre-booted page).
          await sc.before?.(state, i, env);
          const t = now();
          const phases = await sc.run(state, i, env);
          result.samples.push(now() - t);
          for (const [k, v] of Object.entries(phases ?? {})) (result.phases[k] ??= []).push(v);
        }
        result.wallMs = now() - t0;
        const after = await browserSnapshot(b.cp.pid);
        if (control) result.perf = await control.send('LP.perfStats');
        if (before && after) {
          result.cpuMsPerOp = (after.cpuMs - before.cpuMs) / (iterations * (sc.opsPerSample ?? 1));
          result.rssKiB = after.rssKiB;
          if (after.instructions > 0) result.instructionsPerOp = (after.instructions - before.instructions) / (iterations * (sc.opsPerSample ?? 1));
        }
        if (sc.check) await sc.check(state, env);
      })(),
      Math.max(60_000, 120_000 * SCALE),
      sc.name,
    );
  } catch (e) {
    result.error = String(e?.message ?? e).split('\n').slice(0, 4).join('\n');
    const err = b.stderr().trim();
    if (err) result.stderr = err.slice(-1500);
  } finally {
    await Promise.resolve(sc.teardown?.(state, env)).catch(() => {});
    await cdp?.close().catch(() => {});
    await perfCdp?.close().catch(() => {});
    await stopBrowser(b);
  }
  return result;
}

// Per stage: inclusive ns per benchmark operation and per call, median over rounds.
function perfStages(rounds, ops) {
  const traced = rounds.filter((r) => r.perf);
  if (!traced.length) return null;
  const names = [...new Set(traced.flatMap((r) => r.perf.stages.map((st) => st.name)))];
  const stages = names.map((name) => {
    const per = traced.map((r) => r.perf.stages.find((st) => st.name === name)).filter(Boolean);
    return {
      name,
      nsPerOp: +median(per.map((st) => st.totalNs / ops)).toFixed(1),
      callsPerOp: +median(per.map((st) => st.count / ops)).toFixed(2),
      meanNs: +median(per.map((st) => st.meanNs)).toFixed(1),
      p50Ns: median(per.map((st) => st.p50Ns)),
      p99Ns: median(per.map((st) => st.p99Ns)),
    };
  });
  stages.sort((a, b) => b.nsPerOp - a.nsPerOp);
  return { clockNs: +median(traced.map((r) => r.perf.clockNs)).toFixed(1), recordNs: +median(traced.map((r) => r.perf.recordNs)).toFixed(1), stages };
}

// ---------- report ----------
const fmt = (x, d = 3) => (x == null || !Number.isFinite(x) ? '—' : x >= 100 ? x.toFixed(1) : x.toFixed(d));

function markdown(report) {
  const labels = report.bins.map((b) => b.label);
  const lines = [`# CDP performance: ${labels.join(' vs ')}`, ''];
  const m = report.meta;
  lines.push(`${m.startedAt} on ${m.host.cpuModel} (${m.host.cpus} CPUs, ${m.host.platform} ${m.host.release}), ${m.rounds} round(s), scale ${m.scale}.`);
  lines.push(`Load average at start ${m.loadavg.start.map((x) => x.toFixed(2)).join('/')}, at end ${m.loadavg.end.map((x) => x.toFixed(2)).join('/')}.`, '');
  for (const b of report.bins) lines.push(`- **${b.label}**: \`${b.path}\` sha256 \`${b.sha256.slice(0, 16)}\`, startup median ${fmt(b.startup.median)} ms`);
  lines.push('', 'Times are milliseconds per sample (median, p90 over all rounds); `rsd` is the robust relative spread; `cpu/op` is browser CPU per operation.', '');
  lines.push(`| scenario | ${labels.map((l) => `${l} median | p90 | rsd | ops/s | cpu/op`).join(' | ')} |`);
  lines.push(`|---|${labels.map(() => '---:|---:|---:|---:|---:').join('|')}|`);
  for (const sc of report.scenarios) {
    const cells = labels.map((l) => {
      const r = sc.results[l];
      if (!r) return '— | — | — | — | —';
      if (r.errors.length && !r.stats.n) return `**FAILED** | | | |`;
      const ops = r.stats.median ? (1000 * sc.opsPerSample) / r.stats.median : null;
      return `${fmt(r.stats.median)} | ${fmt(r.stats.p90)} | ${fmt(r.stats.rsd, 2)} | ${ops == null ? '—' : ops.toFixed(0)} | ${fmt(r.cpuMsPerOp)}`;
    });
    lines.push(`| ${sc.name} | ${cells.join(' | ')} |`);
  }
  const phased = report.scenarios.filter((sc) => labels.some((l) => Object.keys(sc.results[l]?.phases ?? {}).length));
  if (phased.length) {
    lines.push('', '## Phase breakdown (median ms since the step started)', '');
    for (const sc of phased) {
      for (const l of labels) {
        const ph = sc.results[l]?.phases ?? {};
        lines.push(`- ${sc.name} [${l}]: ${Object.entries(ph).map(([k, v]) => `${k} ${fmt(v.median)}`).join(', ')}`);
      }
    }
  }
  const traced = report.scenarios.filter((sc) => labels.some((l) => sc.results[l]?.stages));
  if (traced.length) {
    lines.push('', '## Inner CDP stages (perf-trace)', '');
    lines.push('Inclusive nanoseconds per benchmark operation (stages nest: `cdp.handle_text` contains `cdp.parse` and `cdp.dispatch ...`, which contains `js.*`, `remote.*`, `json.serialize`, `outbox.push`). Stages under 10 ns/op are left out. `io.*` stages are the socket I/O, done on the browser thread itself; `io.outbound_queue` is how long a frame waited for the socket.', '');
    for (const sc of traced) {
      for (const l of labels) {
        const st = sc.results[l]?.stages;
        if (!st) continue;
        lines.push(`### ${sc.name} [${l}]`, '', `Timer floor: clock ${st.clockNs} ns, one record ${st.recordNs} ns.`, '');
        lines.push('| stage | ns/op | calls/op | mean ns | p50 ns | p99 ns |', '|---|---:|---:|---:|---:|---:|');
        for (const x of st.stages.filter((x) => x.nsPerOp >= 10)) {
          lines.push(`| ${x.name} | ${x.nsPerOp} | ${x.callsPerOp} | ${x.meanNs} | ${x.p50Ns} | ${x.p99Ns} |`);
        }
        lines.push('');
      }
    }
  }
  const failed = report.scenarios.flatMap((sc) => labels.flatMap((l) => (sc.results[l]?.errors ?? []).map((e) => `- ${sc.name} [${l}] round ${e.round}: ${e.error.replace(/\n/g, ' ')}`)));
  if (failed.length) lines.push('', '## Failures', '', ...failed);
  return lines.join('\n') + '\n';
}

// ---------- main ----------
async function main() {
  const selected = SCENARIOS.filter((s) => !FILTER || FILTER.test(s.name) || FILTER.test(s.group));
  if (flag('--list')) {
    for (const s of SCENARIOS) console.log(`${s.name.padEnd(36)} ${s.group.padEnd(11)} ${s.what}`);
    return 0;
  }
  if (!BINS.length || !OUT) usage();
  if (new Set(BINS.map((b) => b.label)).size !== BINS.length) usage();
  for (const b of BINS) {
    if (!fs.existsSync(b.path)) throw new Error(`no such binary: ${b.path}`);
  }
  let playwright = null;
  if (flag('--playwright') && selected.some((s) => s.playwright)) {
    try {
      playwright = require(process.env.PLAYWRIGHT_CORE_PATH ?? 'playwright-core');
    } catch {
      console.error('cdp-perf: playwright-core not installed (run `bun install` in bench/); skipping Playwright scenarios');
    }
  }
  const webview = globalThis.Bun?.WebView;
  if (!webview && selected.some((s) => s.webview)) {
    console.error('cdp-perf: not running under Bun with Bun.WebView; skipping webview scenarios (run with `bun`)');
  }
  const scenarios = selected.filter((s) => (!s.playwright || playwright) && (!s.webview || webview));
  if (!scenarios.length) throw new Error('no scenarios selected');

  fs.mkdirSync(path.dirname(path.resolve(OUT)), { recursive: true });
  const site = await startSite();
  const meta = {
    startedAt: new Date().toISOString(),
    host: { platform: process.platform, release: os.release(), arch: process.arch, cpus: os.cpus().length, cpuModel: os.cpus()[0]?.model ?? '?', totalMemMiB: Math.round(os.totalmem() / 2 ** 20), hostname: os.hostname() },
    node: process.version,
    bun: globalThis.Bun?.version ?? null,
    playwright: playwright ? require(`${process.env.PLAYWRIGHT_CORE_PATH ?? 'playwright-core'}/package.json`).version : null,
    rounds: ROUNDS,
    scale: SCALE,
    filter: FILTER?.source ?? null,
    perfTrace: PERF_TRACE,
    harnessSha256: sha256(fileURLToPath(import.meta.url)),
    cpuSource: process.platform === 'linux' ? '/proc/<pid>/task/*/schedstat' : SAMPLER ? SAMPLER : null,
    loadavg: { start: os.loadavg(), perRound: [] },
  };
  if (meta.loadavg.start[0] > meta.host.cpus * 0.5) {
    console.error(`cdp-perf: warning: 1-minute load average ${meta.loadavg.start[0].toFixed(1)} on ${meta.host.cpus} CPUs; expect noisy timings`);
  }
  const bins = BINS.map((b) => ({ ...b, sha256: sha256(b.path), startupSamples: [] }));
  const raw = new Map(scenarios.map((s) => [s.name, new Map(bins.map((b) => [b.label, { rounds: [], errors: [] }]))]));

  for (let round = 0; round < ROUNDS; round++) {
    const order = round % 2 ? [...bins].reverse() : bins;
    for (const sc of scenarios) {
      for (const bin of order) {
        const r = await runScenario(sc, bin, site.url, playwright);
        bin.startupSamples.push(r.startupMs);
        const slot = raw.get(sc.name).get(bin.label);
        if (r.error) {
          slot.errors.push({ round, error: r.error, stderr: r.stderr });
          console.error(`  ${sc.name} [${bin.label}] round ${round + 1}: FAILED ${r.error.split('\n')[0]}`);
        } else {
          slot.rounds.push(r);
          console.error(`  ${sc.name.padEnd(34)} [${bin.label}] round ${round + 1}/${ROUNDS}: median ${fmt(median(r.samples))} ms`);
        }
      }
    }
    meta.loadavg.perRound.push(os.loadavg());
  }
  meta.loadavg.end = os.loadavg();
  killGroup(site.cp, 'SIGTERM');

  const report = {
    schema: SCHEMA,
    meta,
    bins: bins.map((b) => ({ label: b.label, path: b.path, sha256: b.sha256, startup: summarize(b.startupSamples), startupSamples: b.startupSamples.map((x) => +x.toFixed(3)) })),
    scenarios: scenarios.map((sc) => ({
      name: sc.name,
      group: sc.group,
      what: sc.what,
      unit: 'ms',
      opsPerSample: sc.opsPerSample ?? 1,
      results: Object.fromEntries(
        [...raw.get(sc.name)].map(([label, slot]) => {
          const samples = slot.rounds.flatMap((r) => r.samples);
          const phaseNames = [...new Set(slot.rounds.flatMap((r) => Object.keys(r.phases)))];
          const cpu = slot.rounds.map((r) => r.cpuMsPerOp).filter((x) => x != null);
          const ops = Math.max(3, Math.round(sc.iterations * SCALE)) * (sc.opsPerSample ?? 1);
          return [
            label,
            {
              stats: summarize(samples),
              roundMedians: slot.rounds.map((r) => +median(r.samples).toFixed(4)),
              cpuMsPerOp: cpu.length ? +median(cpu).toFixed(4) : null,
              instructionsPerOp: slot.rounds.map((r) => r.instructionsPerOp).filter((x) => x != null).map(Math.round),
              rssKiB: slot.rounds.map((r) => r.rssKiB).filter((x) => x != null),
              phases: Object.fromEntries(phaseNames.map((k) => [k, summarize(slot.rounds.flatMap((r) => r.phases[k] ?? []))])),
              stages: perfStages(slot.rounds, ops),
              samples: slot.rounds.map((r) => r.samples.map((x) => +x.toFixed(4))),
              errors: slot.errors,
            },
          ];
        }),
      ),
    })),
  };
  fs.writeFileSync(OUT, JSON.stringify(report, null, 1));
  const md = markdown(report);
  fs.writeFileSync(OUT.replace(/\.json$/, '') + '.md', md);
  process.stdout.write(md);
  const failures = report.scenarios.some((sc) => Object.values(sc.results).some((r) => r.errors.length));
  return failures ? 1 : 0;
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`cdp-perf: ${e?.stack ?? e}`);
    process.exit(1);
  },
);
