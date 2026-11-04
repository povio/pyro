// Real Playwright 1.59.1 (connectOverCDP) against `lightningpanda serve` on JavaScriptCore.
// Covers about:blank plus real HTTP navigation, redirects, response bodies and explicit network failures.
//
// Usage: node tests/e2e/about-blank.mjs /abs/path/to/lightningpanda
// Requires `bun install` (or npm install) in bench/ for the pinned playwright-core.
// Exit status: 0 when every check passed, 1 on any failure, 2 for usage errors.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import http from 'node:http';
import https from 'node:https';
import http2 from 'node:http2';
import { X509Certificate, createPrivateKey } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');

const bin = process.argv[2];
if (!bin) {
  console.error('usage: node tests/e2e/about-blank.mjs /abs/path/to/lightningpanda');
  process.exit(2);
}

const deadline = setTimeout(() => {
  console.error(`about-blank: overall deadline exceeded; completed ${JSON.stringify(results)}`);
  process.exit(1);
}, 60_000);

const port = await new Promise((resolve, reject) => {
  const s = net.createServer();
  s.on('error', reject);
  s.listen(0, '127.0.0.1', () => {
    const { port } = s.address();
    s.close(() => resolve(port));
  });
});

// Exact flags TOS passes (apps/fe/e2e/fixtures/lightpanda.ts).
const child = spawn(
  bin,
  ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error',
    '--experimental-features', 'cors', '--load-resources', 'stylesheet', '--load-resources', 'iframe'],
  { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true' } },
);
let stderr = '';
child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} });

const results = [];
async function check(name, fn) {
  if (process.env.E2E_VERBOSE) console.error(`check: ${name}`);
  const t = performance.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: +(performance.now() - t).toFixed(1) });
  } catch (e) {
    results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') });
  }
}

const started = performance.now();
for (;;) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) });
    if (r.ok) break;
  } catch {}
  if (child.exitCode !== null) {
    console.error(`lightningpanda exited early (${child.exitCode}): ${stderr}`);
    process.exit(1);
  }
  if (performance.now() - started > 10_000) {
    console.error(`startup timed out: ${stderr}`);
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 10));
}
const startupMs = +(performance.now() - started).toFixed(1);

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const context = await browser.newContext({ locale: undefined });
const page = await context.newPage();
const consoleMessages = [];
const pageErrors = [];
page.on('console', (m) => consoleMessages.push(`${m.type()}:${m.text()}`));
page.on('pageerror', (e) => pageErrors.push(`${e.name}:${e.message}`));

await check('initial url', async () => assert.equal(page.url(), 'about:blank'));
await check('evaluate', async () => assert.equal(await page.evaluate(() => 1 + 2), 3));
await check('evaluate with argument', async () =>
  assert.deepEqual(await page.evaluate((x) => [x, x === null], null), [null, true]));
await check('awaited promise across a timer', async () =>
  assert.equal(await page.evaluate(() => new Promise((r) => setTimeout(() => r('late'), 10))), 'late'));
await check('evaluateHandle + getProperties + jsonValue', async () => {
  const h = await page.evaluateHandle(() => ({ a: 1, b: [1, 2] }));
  assert.deepEqual([...(await h.getProperties()).keys()].sort(), ['a', 'b']);
  assert.deepEqual(await h.jsonValue(), { a: 1, b: [1, 2] });
  await h.dispose();
});
await check('console event', async () => {
  await page.evaluate(() => console.log('hello', 42));
  await page.waitForTimeout(20);
  assert.ok(consoleMessages.includes('log:hello 42'), JSON.stringify(consoleMessages));
});
await check('pageerror from a timer', async () => {
  await page.evaluate(() => { setTimeout(() => { throw new TypeError('boom'); }, 0); });
  await page.waitForTimeout(50);
  assert.ok(pageErrors.includes('TypeError:boom'), JSON.stringify(pageErrors));
});
await check('evaluate exception', async () => {
  await assert.rejects(page.evaluate(() => { throw new RangeError('bad range'); }), /RangeError: bad range/);
});
await check('goto about:blank', async () => {
  await page.goto('about:blank');
  assert.equal(await page.evaluate(() => typeof window), 'object');
});
await check('goto http fails explicitly', async () => {
  await assert.rejects(page.goto('http://127.0.0.1:1/x', { timeout: 5000 }), /ERR_CONNECTION_REFUSED/);
});
await check('Fetch fulfill and abort', async () => {
  const body = '<script>globalThis.fulfilledFromRoute=43</script>';
  await page.route('**/fulfilled', (route) => route.fulfill({ status: 202, contentType: 'text/html', body }));
  try {
    const response = await page.goto('http://127.0.0.1:1/fulfilled', { timeout: 5000 });
    assert.equal(response.status(), 202);
    assert.equal(await response.text(), body);
    assert.equal(await page.evaluate(() => globalThis.fulfilledFromRoute), 43);
  } finally {
    await page.unroute('**/fulfilled');
  }
  await page.route('**/aborted', (route) => route.abort('aborted'));
  try {
    await assert.rejects(page.goto('http://127.0.0.1:1/aborted', { timeout: 5000 }), /ERR_ABORTED/);
  } finally {
    await page.unroute('**/aborted');
  }
});
await check('Fetch binary response headers through CDP', async () => {
  const session = await context.newCDPSession(page);
  let decisionError;
  session.on('Fetch.requestPaused', event => {
    session.send('Fetch.fulfillRequest', {
      requestId: event.requestId, responseCode: 203,
      binaryResponseHeaders: Buffer.from('content-type: text/html\0x-route: binary\0').toString('base64'),
      body: Buffer.from('<script>globalThis.binaryRoute=46</script>').toString('base64'),
    }).catch(error => { decisionError = error; });
  });
  try {
    await session.send('Fetch.enable', { patterns: [{ urlPattern: '**/binary-headers' }] });
    const response = await page.goto('http://127.0.0.1:1/binary-headers', { timeout: 5000 });
    assert.equal(decisionError, undefined);
    assert.equal(response.status(), 203);
    assert.equal(await response.headerValue('x-route'), 'binary');
    assert.equal(await page.evaluate(() => globalThis.binaryRoute), 46);
  } finally {
    await session.detach();
  }
});
await check('HTTP redirect, response body and inline script', async () => {
  const body = '<!doctype html><script>globalThis.loadedFromHttp=42</script>';
  let continuedHeader = null;
  const server = http.createServer((req, res) => {
    if (req.url === '/redirect') {
      res.writeHead(302, { location: '/document' });
      res.end();
    } else {
      continuedHeader = req.headers['x-route-check'];
      res.writeHead(201, { 'content-type': 'text/html' });
      res.end(body);
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  try {
    const origin = `http://127.0.0.1:${server.address().port}`;
    await page.route('**/redirect', (route) => route.continue({ headers: { ...route.request().headers(), 'x-route-check': 'continued' } }));
    const response = await page.goto(`${origin}/redirect`, { timeout: 5000 });
    assert.ok(response, 'navigation exposes a Network response');
    assert.equal(response.status(), 201);
    assert.equal(response.url(), `${origin}/document`);
    assert.equal(await response.text(), body);
    assert.equal(response.request().redirectedFrom().url(), `${origin}/redirect`);
    assert.equal(await page.evaluate(() => globalThis.loadedFromHttp), 42);
    assert.equal(continuedHeader, 'continued');
  } finally {
    await page.unroute('**/redirect');
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
await check('Fetch Basic authentication challenge', async () => {
  let requests = 0;
  const authorization = `Basic ${Buffer.from('fixture:local-test').toString('base64')}`;
  const server = http.createServer((req, res) => {
    requests++;
    if (req.headers.authorization !== authorization) {
      res.writeHead(401, { 'www-authenticate': 'Basic realm="fixture"', 'content-length': '0' });
      res.end();
    } else {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<script>globalThis.authenticatedFixture=44</script>');
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  let authenticated;
  try {
    authenticated = await browser.newContext({ httpCredentials: { username: 'fixture', password: 'local-test' } });
    const authPage = await authenticated.newPage();
    const response = await authPage.goto(`http://127.0.0.1:${server.address().port}/auth`, { timeout: 5000 });
    assert.equal(response.status(), 200);
    assert.equal(await authPage.evaluate(() => globalThis.authenticatedFixture), 44);
    assert.equal(requests, 2, 'one challenge followed by one credentialed retry');
  } finally {
    if (authenticated) await authenticated.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
await check('context cookies reach HTTP and stay isolated', async () => {
  const received = [];
  const server = http.createServer((req, res) => {
    received.push(req.headers.cookie ?? '');
    res.writeHead(200, { 'content-type': 'text/html', 'set-cookie': 'fromServer=2; Path=/; HttpOnly; SameSite=Lax' });
    res.end('<script>globalThis.cookieFixture=45</script>');
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const url = `http://127.0.0.1:${server.address().port}/cookie`;
  let first, second;
  try {
    first = await browser.newContext(); second = await browser.newContext();
    await first.addCookies([{ name: 'injected', value: '1', url, httpOnly: true, sameSite: 'Lax' }]);
    assert.deepEqual(await second.cookies(), []);
    await (await first.newPage()).goto(url, { timeout: 5000 });
    assert.equal(received[0], 'injected=1');
    const cookies = await first.cookies(url);
    assert.deepEqual(cookies.map(c => c.name).sort(), ['fromServer', 'injected']);
    assert.ok(cookies.every(c => c.httpOnly && c.sameSite === 'Lax' && c.expires === -1));
    await (await second.newPage()).goto(url, { timeout: 5000 });
    assert.equal(received[1], '');
    await first.clearCookies();
    assert.deepEqual(await first.cookies(), []);
    assert.equal((await second.cookies())[0].name, 'fromServer');
  } finally {
    if (first) await first.close(); if (second) await second.close();
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
});
// Browser binding integration: local HTTP only, without claiming DOM/Input or TOS parity.
const identityHeaders = [];
const cacheableHits = [];
const fixture = http.createServer((req, res) => {
  identityHeaders.push({ url: req.url, agent: req.headers['user-agent'], language: req.headers['accept-language'], extra: req.headers['x-extra'] ?? null, chUa: req.headers['sec-ch-ua'] ?? null, chMobile: req.headers['sec-ch-ua-mobile'] ?? null, chPlatform: req.headers['sec-ch-ua-platform'] ?? null });
  if (req.url.startsWith('/cacheable')) {
    cacheableHits.push(req.url);
    res.writeHead(200, { 'content-type': 'text/javascript', 'cache-control': 'public, max-age=3600' });
    res.end('1;');
    return;
  }
  if (req.url === '/extra-headers') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ extra: req.headers['x-extra'] ?? null, cache: req.headers['cache-control'] ?? null }));
    return;
  }
  if (req.url.startsWith('/spa')) {
    // A minimal SPA router: renders the route from history and re-renders on popstate.
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><div id="view"></div><script>' +
      'const render = () => { document.querySelector("#view").textContent = location.pathname + "|" + JSON.stringify(history.state); };' +
      'addEventListener("popstate", render); globalThis.go = path => { history.pushState({ path }, "", path); render(); }; render();</script>');
    return;
  }
  if (req.url === '/frames-parsed') {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><iframe name="parsed" src="/frame-inner"></iframe>' +
      '<iframe name="inline" srcdoc="&lt;p&gt;inline child&lt;/p&gt;"></iframe>');
    return;
  }
  if (req.url === '/checkpoint.css') {
    res.writeHead(200, { 'content-type': 'text/css' });
    res.end('#message { display: none }');
  } else if (req.url === '/identity') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ agent: req.headers['user-agent'], language: req.headers['accept-language'] }));
  } else if (req.url === '/json') {
    res.writeHead(201, { 'content-type': 'application/json', 'x-fixture': 'native-fetch' });
    res.end(JSON.stringify({ answer: 47 }));
  } else if (req.url.startsWith('/inttra-credentials')) {
    // Mirrors TOS offices-inttra-credentials: the app POSTs/PATCHes JSON and
    // the test matches requests by their JSON body.
    const chunks = [];
    req.on('data', chunk => chunks.push(chunk));
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ method: req.method, bytes: Buffer.concat(chunks).length }));
    });
    return;
  } else if (req.url === '/slow') {
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.flushHeaders();
    res.write('partial'); // Deliberately pending until the caller aborts/disposes it.
  } else {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><title>Binding fixture</title><div id="message">native DOM</div>' +
      '<script>globalThis.inlineSawInit=globalThis.initBindingTypes;' +
      'globalThis.lifecycle=[];document.addEventListener("DOMContentLoaded",()=>lifecycle.push(document.readyState));' +
      'window.addEventListener("load",()=>lifecycle.push(document.readyState));</script>');
  }
});
await new Promise((resolve, reject) => { fixture.once('error', reject); fixture.listen(0, '127.0.0.1', resolve); });
const fixtureOrigin = `http://127.0.0.1:${fixture.address().port}`;
try {
  await check('DOM installed before init scripts and shared across isolated worlds', async () => {
    await page.addInitScript(() => { globalThis.initBindingTypes = [typeof document, typeof fetch, typeof localStorage]; });
    await page.goto(`${fixtureOrigin}/document`, { timeout: 5000 });
    assert.deepEqual(await page.evaluate(() => inlineSawInit), ['object', 'function', 'object']);
    assert.deepEqual(await page.evaluate(() => lifecycle), ['interactive', 'complete']);
    assert.equal(await page.title(), 'Binding fixture');
    await page.evaluate(() => { document.querySelector('#message').textContent = 'changed in main'; });
    const session = await context.newCDPSession(page);
    try {
      const { frameTree } = await session.send('Page.getFrameTree');
      const { executionContextId } = await session.send('Page.createIsolatedWorld', { frameId: frameTree.frame.id, worldName: 'binding-check' });
      const { result, exceptionDetails } = await session.send('Runtime.evaluate', {
        contextId: executionContextId, returnByValue: true,
        expression: '[document.querySelector("#message").textContent, typeof globalThis.inlineSawInit]',
      });
      assert.equal(exceptionDetails, undefined);
      assert.deepEqual(result.value, ['changed in main', 'undefined']);
    } finally { await session.detach(); }
  });
  await check('page fetch uses current frame and loader in Network events', async () => {
    const session = await context.newCDPSession(page);
    const requests = [];
    session.on('Network.requestWillBeSent', event => requests.push(event));
    try {
      await session.send('Network.enable');
      const { frameTree } = await session.send('Page.getFrameTree');
      const responseEvent = page.waitForResponse(`${fixtureOrigin}/json`, { timeout: 5000 });
      const value = await page.evaluate(async () => {
        const response = await fetch('/json');
        return [response.status, response.headers.get('x-fixture'), await response.json()];
      });
      assert.deepEqual(value, [201, 'native-fetch', { answer: 47 }]);
      const response = await responseEvent;
      assert.equal(response.request().resourceType(), 'fetch');
      assert.deepEqual(await response.json(), { answer: 47 });
      const event = requests.find(event => event.request.url === `${fixtureOrigin}/json`);
      assert.ok(event, 'native fetch request is observed through CDP');
      assert.equal(event.frameId, frameTree.frame.id);
      assert.equal(event.loaderId, frameTree.frame.loaderId);
      assert.notEqual(event.requestId, event.loaderId, 'subresource retains native request identity');
    } finally { await session.detach(); }
  });
  await check('request bodies reach postDataJSON in waitForRequest, waitForResponse and routes (TOS INTTRA case)', async () => {
    const credentials = { credentialType: 'CLIENT', notificationEmail: 'ops@example.test' };
    const requested = page.waitForRequest(r => r.url().endsWith('/inttra-credentials') && r.postDataJSON()?.credentialType === 'CLIENT', { timeout: 5000 });
    const responded = page.waitForResponse(r => r.url().endsWith('/inttra-credentials') && r.request().postDataJSON()?.notificationEmail === 'ops@example.test', { timeout: 5000 });
    const status = await page.evaluate(async body => (await fetch('/inttra-credentials', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })).status, credentials);
    assert.equal(status, 200);
    // Await both together so a failure of either stays inside this check.
    const [request, response] = await Promise.all([requested, responded]);
    assert.deepEqual(request.postDataJSON(), credentials);
    assert.deepEqual(response.request().postDataJSON(), credentials);
    // Intercepted requests carry the body too (Fetch.requestPaused), including non-UTF-8 bytes.
    const seen = [];
    await page.route('**/inttra-credentials/7', route => { seen.push([route.request().method(), route.request().postDataJSON()]); return route.continue(); });
    await page.route('**/inttra-credentials/binary', route => { seen.push([route.request().method(), [...route.request().postDataBuffer()]]); return route.continue(); });
    try {
      const patched = await page.evaluate(async () => {
        const json = await fetch('/inttra-credentials/7', { method: 'PATCH', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ notificationEmail: 'new@example.test' }) });
        const binary = await fetch('/inttra-credentials/binary', { method: 'POST', body: new Uint8Array([0xff, 0x00, 0xfe]) });
        return [await json.json(), await binary.json()];
      });
      assert.deepEqual(patched, [{ method: 'PATCH', bytes: JSON.stringify({ notificationEmail: 'new@example.test' }).length }, { method: 'POST', bytes: 3 }]);
      assert.deepEqual(seen, [['PATCH', { notificationEmail: 'new@example.test' }], ['POST', [0xff, 0x00, 0xfe]]]);
    } finally {
      await page.unroute('**/inttra-credentials/7');
      await page.unroute('**/inttra-credentials/binary');
    }
  });
  await check('keyboard.press arrows, selection, select-all and Backspace with macOS editing commands (TOS docubox case)', async () => {
    // On macOS Playwright attaches editing commands (ArrowRight -> moveRight), which are advisory:
    // the key's default action runs as on Linux, where Playwright sends none.
    await page.evaluate(() => {
      document.body.innerHTML = '<input id="caret" value="abcd"><button id="tree">tree</button>';
      globalThis.treeKeys = [];
      document.querySelector('#tree').addEventListener('keydown', e => treeKeys.push(e.key));
      const input = document.querySelector('#caret');
      input.focus(); input.setSelectionRange(0, 0);
    });
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Shift+ArrowRight');
    assert.deepEqual(await page.evaluate(() => { const c = document.querySelector('#caret'); return [c.selectionStart, c.selectionEnd]; }), [1, 2]);
    await page.keyboard.press('ArrowLeft');
    await page.keyboard.press('Backspace');
    assert.deepEqual(await page.evaluate(() => { const c = document.querySelector('#caret'); return [c.value, c.selectionStart]; }), ['bcd', 0]);
    await page.keyboard.press('Meta+a');
    assert.deepEqual(await page.evaluate(() => { const c = document.querySelector('#caret'); return [c.selectionStart, c.selectionEnd]; }), [0, 3]);
    await page.focus('#tree');
    await page.keyboard.press('ArrowRight');
    assert.deepEqual(await page.evaluate(() => treeKeys), ['ArrowRight']);
    // TOS W3-08 quotes-finances closes a QuerySelect with Escape (macOS command cancelOperation).
    await page.keyboard.press('Escape');
    assert.deepEqual(await page.evaluate(() => treeKeys), ['ArrowRight', 'Escape']);
    await page.focus('#caret');
    await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(() => document.querySelector('#caret').value), 'bcd');
    // TOS W3-08 QuerySelect opener presses ArrowDown (macOS command moveDown).
    await page.evaluate(() => document.querySelector('#caret').setSelectionRange(1, 1));
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Shift+ArrowUp');
    assert.deepEqual(await page.evaluate(() => { const c = document.querySelector('#caret'); return [c.selectionStart, c.selectionEnd]; }), [0, 3]);
  });
  await check('page fetch route fulfillment and abort after headers', async () => {
    await page.route('**/routed-fetch', route => route.fulfill({ status: 202, contentType: 'application/json', body: '{"routed":48}' }));
    try {
      assert.deepEqual(await page.evaluate(async () => {
        const response = await fetch('/routed-fetch');
        return [response.status, await response.json()];
      }), [202, { routed: 48 }]);
    } finally { await page.unroute('**/routed-fetch'); }
    assert.equal(await page.evaluate(async () => {
      const controller = new AbortController();
      const response = await fetch('/slow', { signal: controller.signal });
      const reading = response.text();
      controller.abort('body stopped');
      try { await reading; return 'unexpected success'; } catch (error) { return error; }
    }), 'body stopped');
  });
  await check('native keyboard text insertion and mouse activation', async () => {
    await page.evaluate(() => {
      document.body.innerHTML = '<input id="typing"><input id="toggle" type="checkbox" style="position:absolute;left:20px;top:80px;width:40px;height:40px">';
      globalThis.inputEvents = [];
      const field = document.querySelector('#typing');
      field.addEventListener('input', event => inputEvents.push(event.isTrusted));
      field.focus();
    });
    await page.keyboard.type('abc');
    await page.keyboard.insertText('β');
    assert.deepEqual(await page.evaluate(() => [document.querySelector('#typing').value, inputEvents]), ['abcβ', [true, true, true, true]]);
    const box = await page.evaluate(() => {
      const rect = document.querySelector('#toggle').getBoundingClientRect();
      const x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
      return { x, y, width: rect.width, height: rect.height,
        atPoint: document.elementFromPoint(x,y)?.id };
    });
    assert.ok(box.width > 0 && box.height > 0, JSON.stringify(box));
    assert.equal(box.atPoint, 'toggle', JSON.stringify(box));
    await page.mouse.click(box.x, box.y);
    assert.equal(await page.evaluate(() => document.querySelector('#toggle').checked), true);
  });
  await check('DOM handles expose owner frame and native geometry', async () => {
    await page.evaluate(() => {
      document.body.innerHTML = '<input id="dom-field" aria-label="Name" style="width:80px;height:20px"><button id="dom-button" style="width:80px;height:20px">Commit</button>';
      globalThis.domClicks = 0;
      document.querySelector('#dom-button').addEventListener('click', () => domClicks++);
    });
    const handle = await page.evaluateHandle(() => document.querySelector('#dom-button'));
    assert.ok(handle);
    try {
      assert.equal(await handle.ownerFrame(), page.mainFrame());
      const box = await handle.boundingBox();
      assert.ok(box && box.width > 0 && box.height > 0, JSON.stringify(box));
    } finally { await handle.dispose(); }
  });
  await check('real locator fill and click', async () => {
    await page.locator('#dom-field').fill('typed through locator', { timeout: 5000 });
    assert.equal(await page.locator('#dom-field').inputValue(), 'typed through locator');
    await page.locator('#dom-button').click({ timeout: 5000 });
    assert.equal(await page.evaluate(() => domClicks), 1);
  });
  await check('Accessibility AX trees over CDP agree with getByRole', async () => {
    await page.setContent('<title>AX</title><nav aria-label="Main"><a href="#a">Home</a></nav><h2>Title <b>here</b></h2><label for="ax-n">Name</label><input id="ax-n" value="v"><button aria-hidden="true">Hidden</button><button>Save</button>');
    const session = await context.newCDPSession(page);
    try {
      const { nodes } = await session.send('Accessibility.getFullAXTree');
      assert.equal(nodes[0].role.value, 'RootWebArea');
      assert.equal(nodes[0].name.value, 'AX');
      const byId = new Map(nodes.map((n) => [n.nodeId, n]));
      for (const n of nodes) for (const c of n.childIds) assert.equal(byId.get(c)?.parentId, n.nodeId, `child ${c} of ${n.nodeId}`);
      const named = nodes.filter((n) => !n.ignored && n.role.type === 'role').map((n) => `${n.role.value}:${n.name.value}`);
      assert.deepEqual(named.sort(), ['button:Save', 'heading:Title here', 'link:Home', 'navigation:Main', 'textbox:Name']);
      const root = await session.send('Runtime.evaluate', { expression: 'document.body' });
      const { nodes: found } = await session.send('Accessibility.queryAXTree', { objectId: root.result.objectId, role: 'button' });
      assert.deepEqual(found.map((n) => [n.name.value, n.ignored]), [['', true], ['Save', false]]);
      const save = found[1];
      const { nodes: partial } = await session.send('Accessibility.getPartialAXTree', { backendNodeId: save.backendDOMNodeId });
      assert.equal(partial[0].nodeId, save.nodeId);
      assert.equal(partial.at(-1).role.value, 'RootWebArea');
      // getByRole (injected script) sees the same roles and names.
      await page.getByRole('button', { name: 'Save' }).click({ timeout: 5000 });
      assert.equal(await page.getByRole('heading', { name: 'Title here' }).count(), 1);
      assert.equal(await page.getByRole('textbox', { name: 'Name' }).inputValue(), 'v');
    } finally { await session.detach(); }
  });
  await check('synchronous CDP DOM insertion starts stylesheet loading while idle', async () => {
    const isolated = await context.newPage();
    let timeout;
    try {
      await isolated.goto(`${fixtureOrigin}/checkpoint-page`, { timeout: 5000 });
      // Let navigation work settle; the page schedules no timer or follow-up command.
      await new Promise(resolve => setTimeout(resolve, 50));
      const loaded = isolated.evaluate(() => new Promise((resolve, reject) => {
        const link = document.createElement('link');
        link.rel = 'stylesheet'; link.href = '/checkpoint.css';
        link.addEventListener('load', () => resolve(getComputedStyle(document.querySelector('#message')).display));
        link.addEventListener('error', () => reject(new Error('stylesheet failed')));
        document.head.appendChild(link);
      }));
      assert.equal(await Promise.race([loaded, new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('stylesheet did not start after CDP evaluation')), 5000);
      })]), 'none');
    } finally { clearTimeout(timeout); await isolated.close(); }
  });
  await check('page viewport changes native dimensions, resize events and navigation state', async () => {
    const sized = await browser.newContext({ viewport: { width: 640, height: 480 }, screen: { width: 1024, height: 768 }, deviceScaleFactor: 2 });
    let timeout;
    try {
      const a = await sized.newPage();
      const b = await sized.newPage();
      assert.deepEqual(await a.evaluate(() => [innerWidth, innerHeight, devicePixelRatio, screen.width, screen.height]), [640, 480, 2, 1024, 768]);
      await a.evaluate(() => {
        globalThis.widthQuery = matchMedia('(min-width: 700px)');
        globalThis.resizeSeen = new Promise(resolve => addEventListener('resize', () => resolve([innerWidth, innerHeight]), { once: true }));
      });
      assert.equal(await a.evaluate(() => widthQuery.matches), false);
      await a.setViewportSize({ width: 800, height: 600 });
      assert.deepEqual(await Promise.race([a.evaluate(() => resizeSeen), new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error('native resize event missing')), 5000);
      })]), [800, 600]);
      assert.equal(await a.evaluate(() => widthQuery.matches), true);
      assert.deepEqual(await b.evaluate(() => [innerWidth, innerHeight, devicePixelRatio]), [640, 480, 2]);
      await a.addInitScript(() => { globalThis.initDimensions = [innerWidth, innerHeight, devicePixelRatio]; });
      await a.goto(`${fixtureOrigin}/viewport-page`, { timeout: 5000 });
      assert.deepEqual(await a.evaluate(() => initDimensions), [800, 600, 2]);
      assert.deepEqual(await a.evaluate(() => [innerWidth, innerHeight, devicePixelRatio]), [800, 600, 2]);
    } finally { clearTimeout(timeout); await sized.close(); }
  });
  await check('portrait setViewportSize keeps Playwright screen orientation (TOS notification-settings mobile case)', async () => {
    // Mirrors e2e/chat/notification-settings.e2e.ts: a 1440x900 context resized to 390x844.
    // Playwright sends screenOrientation landscapePrimary/0 for non-mobile viewports.
    const desktop = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    try {
      const page = await desktop.newPage();
      await page.setViewportSize({ width: 390, height: 844 });
      assert.deepEqual(
        await page.evaluate(() => [innerWidth, innerHeight, screen.width, screen.height, screen.orientation.type, screen.orientation.angle, matchMedia('(orientation: portrait)').matches]),
        [390, 844, 390, 844, 'landscape-primary', 0, true],
      );
      await page.setViewportSize({ width: 1440, height: 900 });
      assert.deepEqual(await page.evaluate(() => [innerWidth, innerHeight, matchMedia('(orientation: portrait)').matches]), [1440, 900, false]);
    } finally { await desktop.close(); }
  });
  await check('page-scoped user agent and locale persist across navigation and requests', async () => {
    // Exact TOS locale-only context requests userAgent:"", acceptLanguage:"en-US".
    const identities = await browser.newContext({ locale: 'en-US' });
    try {
      const a = await identities.newPage();
      const b = await identities.newPage();
      const defaultAgent = await b.evaluate(() => navigator.userAgent);
      const session = await identities.newCDPSession(a);
      try {
        await session.send('Emulation.setUserAgentOverride', { userAgent: 'ProtocolFixture/A', acceptLanguage: 'fr-FR,fr;q=0.8', platform: 'FixturePlatform' });
        await assert.rejects(session.send('Emulation.setUserAgentOverride', { userAgent: 'bad\r\nheader' }), /Invalid|invalid|header/i);
        await a.goto(`${fixtureOrigin}/identity-page-a`, { timeout: 5000 });
        await b.goto(`${fixtureOrigin}/identity-page-b`, { timeout: 5000 });
        assert.deepEqual(await a.evaluate(() => [navigator.userAgent, navigator.platform, navigator.language, [...navigator.languages]]), ['ProtocolFixture/A', 'FixturePlatform', 'fr-FR', ['fr-FR', 'fr']]);
        assert.equal(await b.evaluate(() => navigator.userAgent), defaultAgent);
        assert.equal(identityHeaders.find(r => r.url === '/identity-page-a').agent, 'ProtocolFixture/A');
        assert.equal(identityHeaders.find(r => r.url === '/identity-page-a').language, 'fr-FR,fr;q=0.8');
        assert.equal(identityHeaders.find(r => r.url === '/identity-page-b').agent, defaultAgent);
        assert.deepEqual(await a.evaluate(async () => (await fetch('/identity')).json()), { agent: 'ProtocolFixture/A', language: 'fr-FR,fr;q=0.8' });
        assert.deepEqual(await b.evaluate(() => new Promise((resolve, reject) => {
          const xhr = new XMLHttpRequest(); xhr.open('GET', '/identity');
          xhr.onload = () => resolve(JSON.parse(xhr.responseText)); xhr.onerror = () => reject(new Error('XHR failed')); xhr.send();
        })), { agent: defaultAgent, language: 'en-US' });
        await session.send('Emulation.setUserAgentOverride', { userAgent: '' });
        await a.goto(`${fixtureOrigin}/identity-reset`, { timeout: 5000 });
        assert.equal(await a.evaluate(() => navigator.userAgent), defaultAgent);
        assert.equal(identityHeaders.find(r => r.url === '/identity-reset').agent, defaultAgent);
      } finally { await session.detach(); }
    } finally { await identities.close(); }
  });
  await check('client hints from Playwright userAgent, grantPermissions clipboard round trip and hasTouch tap', async () => {
    // Playwright derives userAgentMetadata (no brands, mobile from isMobile)
    // from the agent string; Chrome fills the brands with its own, which
    // for lightningpanda's own agent are none.
    const agent = 'Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Mobile Safari/537.36';
    const touchContext = await browser.newContext({ hasTouch: true, userAgent: agent });
    try {
      const p = await touchContext.newPage();
      await p.goto(`${fixtureOrigin}/tier3`, { timeout: 5000 });
      const hints = await p.evaluate(() => JSON.parse(JSON.stringify(navigator.userAgentData)));
      assert.equal(hints.platform, 'Android');
      assert.equal(hints.mobile, false);
      assert.ok(Array.isArray(hints.brands), JSON.stringify(hints));
      const high = await p.evaluate(() => navigator.userAgentData.getHighEntropyValues(['architecture', 'platformVersion']));
      assert.deepEqual([high.architecture, high.platformVersion], ['arm', '14']);
      assert.equal(await p.evaluate(() => navigator.platform), 'Linux armv8l');
      const session = await touchContext.newCDPSession(p);
      try {
        await session.send('Emulation.setUserAgentOverride', { userAgent: 'Plain/1', userAgentMetadata: { brands: [{ brand: 'X', version: '1' }], platform: 'Linux', platformVersion: '6', architecture: 'x86', model: '', mobile: false } });
        assert.deepEqual(await p.evaluate(() => navigator.userAgentData.getHighEntropyValues(['architecture', 'bitness'])), { architecture: 'x86', bitness: '64', brands: [{ brand: 'X', version: '1' }], mobile: false, platform: 'Linux' });
        await p.goto(fixtureOrigin + '/tier3-hints', { timeout: 5000 });
        const sent = identityHeaders.find(r => r.url === '/tier3-hints');
        assert.deepEqual([sent.chUa, sent.chMobile, sent.chPlatform], ['"X";v="1"', '?0', '"Linux"']);
      } finally { await session.detach(); }

      await assert.rejects(p.evaluate(() => navigator.clipboard.readText()), /Read permission denied/);
      await touchContext.grantPermissions(['clipboard-read', 'clipboard-write']);
      assert.deepEqual(await p.evaluate(() => Promise.all(['clipboard-read', 'clipboard-write'].map(name => navigator.permissions.query({ name }).then(s => s.state)))), ['granted', 'granted']);
      assert.equal(await p.evaluate(async () => { await navigator.clipboard.writeText('round trip'); return navigator.clipboard.readText(); }), 'round trip');
      const second = await touchContext.newPage();
      assert.equal(await second.evaluate(() => navigator.clipboard.readText()), 'round trip', 'the clipboard is per context');
      await second.close();
      await touchContext.clearPermissions();
      await assert.rejects(p.evaluate(() => navigator.clipboard.readText()), /Read permission denied/);

      await p.setContent('<button id="b" style="width:120px;height:40px">tap</button><script>window.log = [];' +
        'for (const t of ["pointerdown", "touchstart", "pointerup", "touchend", "mousedown", "mouseup", "click"]) document.addEventListener(t, e => log.push(t + ":" + (e.pointerType ?? "")), true);</script>');
      assert.equal(await p.evaluate(() => navigator.maxTouchPoints), 1);
      await p.locator('#b').tap({ timeout: 5000 });
      assert.deepEqual(await p.evaluate(() => log), ['pointerdown:touch', 'touchstart:', 'pointerup:touch', 'touchend:', 'mousedown:', 'mouseup:', 'click:touch']);
    } finally { await touchContext.close(); }
  });
  await check('goBack/goForward traverse SPA and document history (TOS email-logs Back/Forward case)', async () => {
    const spa = await context.newPage();
    try {
      const view = () => spa.locator('#view').textContent();
      await spa.goto(`${fixtureOrigin}/spa`, { timeout: 5000 });
      await spa.evaluate(() => go('/spa/one'));
      await spa.evaluate(() => go('/spa/two'));
      assert.equal(await view(), '/spa/two|{"path":"/spa/two"}');
      // Same-document entries: Playwright resolves with null (no new document).
      assert.equal(await spa.goBack({ timeout: 5000 }), null);
      assert.equal(spa.url(), `${fixtureOrigin}/spa/one`);
      assert.equal(await view(), '/spa/one|{"path":"/spa/one"}');
      assert.equal(await spa.goForward({ timeout: 5000 }), null);
      assert.equal(await view(), '/spa/two|{"path":"/spa/two"}');
      // Cross-document entry: the document is fetched again and the response returned.
      await spa.goto(`${fixtureOrigin}/spa-next`, { timeout: 5000 });
      const back = await spa.goBack({ timeout: 5000 });
      assert.equal(back?.url(), `${fixtureOrigin}/spa/two`);
      assert.equal(await view(), '/spa/two|{"path":"/spa/two"}');
      assert.equal((await spa.goForward({ timeout: 5000 }))?.url(), `${fixtureOrigin}/spa-next`);
      // Nothing further forward.
      assert.equal(await spa.goForward({ timeout: 5000 }), null);
      assert.equal(spa.url(), `${fixtureOrigin}/spa-next`);
    } finally { await spa.close(); }
  });
  await check('dialogs auto-dismiss, first-navigation history, offline and context cookies', async () => {
    const p = await context.newPage();
    try {
      await p.goto(`${fixtureOrigin}/document`, { timeout: 5000 });
      // The initial about:blank entry was replaced: nothing to go back to.
      assert.equal(await p.goBack({ timeout: 2000 }), null);
      assert.equal(p.url(), `${fixtureOrigin}/document`);
      // Without a listener Playwright dismisses dialogs; with one it sees them.
      assert.deepEqual(await p.evaluate(() => [confirm('ok?'), prompt('name', 'x')]), [false, null]);
      const seen = [];
      p.on('dialog', d => { seen.push([d.type(), d.message(), d.defaultValue()]); d.dismiss().catch(() => {}); });
      await p.evaluate(() => alert('hello'));
      await p.waitForTimeout(50);
      assert.deepEqual(seen, [['alert', 'hello', '']]);
      await p.context().setOffline(true);
      assert.equal(await p.evaluate(() => navigator.onLine), false);
      assert.equal(await p.evaluate(() => fetch('/document').then(() => 'ok', e => e.name)), 'TypeError');
      await p.context().setOffline(false);
      assert.equal(await p.evaluate(() => navigator.onLine), true);
    } finally { await p.close(); }
  });
  await check('blocking dialogs, rotated boundingBox, terminateExecution and page.close runBeforeUnload', async () => {
    const p = await context.newPage();
    try {
      // The script waits for the answer (Chrome's results for each).
      p.once('dialog', d => d.accept('typed'));
      assert.equal(await p.evaluate(() => prompt('name', 'def')), 'typed');
      p.once('dialog', d => d.dismiss());
      assert.equal(await p.evaluate(() => confirm('sure?')), false);
      p.once('dialog', d => d.accept());
      assert.deepEqual(await p.evaluate(() => [confirm('sure?'), 'after']), [true, 'after']);
      p.once('dialog', d => d.accept());
      assert.equal(await p.evaluate(() => prompt('empty')), '');
      // Opened from a timer task.
      p.once('dialog', d => d.accept('later'));
      assert.equal(await p.evaluate(() => new Promise(r => setTimeout(() => r(prompt('q')), 10))), 'later');
      // Console messages carry their location (Runtime enabled is enough).
      await p.setContent('<script>\nfunction logIt() { console.log("here") }\n</script>');
      const logged = p.waitForEvent('console');
      await p.evaluate(() => logIt());
      const location = (await logged).location();
      assert.equal(location.lineNumber, 1, JSON.stringify(location));
      // setContent is unaffected; a rotated box's quad (Chrome: x 196.967,
      // y -18.033, 106.066 x 106.066).
      await p.setContent('<style>body{margin:0}</style><div id=r style="position:absolute;left:200px;top:10px;width:100px;height:50px;transform:rotate(45deg)"></div>');
      const box = await p.locator('#r').boundingBox();
      for (const [k, v] of [['x', 196.967], ['y', -18.033], ['width', 106.066], ['height', 106.066]])
        assert.ok(Math.abs(box[k] - v) < 0.05, `${k}: ${JSON.stringify(box)}`);
      const cdp = await context.newCDPSession(p);
      const quads = await cdp.send('DOM.getContentQuads', { objectId: (await cdp.send('Runtime.evaluate', { expression: 'document.getElementById("r")' })).result.objectId });
      assert.ok(Math.abs(quads.quads[0][0] - 232.322) < 0.05, JSON.stringify(quads));
      // Runtime.terminateExecution stops a running script.
      const looping = cdp.send('Runtime.evaluate', { expression: 'for (;;) {}' }).then(() => 'finished', e => e.message);
      await new Promise(r => setTimeout(r, 100));
      await cdp.send('Runtime.terminateExecution');
      assert.match(await looping, /Execution was terminated/);
      assert.equal(await p.evaluate(() => 1 + 1), 2);
      await cdp.detach();
      // beforeunload: dismissing keeps the page, accepting closes it.
      await p.evaluate(() => addEventListener('beforeunload', e => { e.preventDefault(); e.returnValue = 'x'; }));
      const seen = [];
      p.once('dialog', d => { seen.push(d.type()); d.dismiss(); });
      await p.close({ runBeforeUnload: true });
      await p.waitForTimeout(200);
      assert.deepEqual(seen, ['beforeunload']);
      assert.equal(p.isClosed(), false);
      const closed = p.waitForEvent('close', { timeout: 5000 });
      p.once('dialog', d => d.accept());
      await p.close({ runBeforeUnload: true });
      await closed;
      assert.equal(p.isClosed(), true);
    } finally { if (!p.isClosed()) await p.close(); }
  });
  await check('page.setExtraHTTPHeaders is page-scoped, replaces and clears (TOS positions-overview case)', async () => {
    const shared = await browser.newContext();
    try {
      const a = await shared.newPage();
      const b = await shared.newPage();
      const read = p => p.evaluate(() => fetch('/extra-headers').then(r => r.json()));
      await a.setExtraHTTPHeaders({ 'X-Extra': 'a', 'Cache-Control': 'no-cache' });
      await a.goto(`${fixtureOrigin}/extra-nav-a`, { timeout: 5000 });
      await b.goto(`${fixtureOrigin}/extra-nav-b`, { timeout: 5000 });
      assert.equal(identityHeaders.find(h => h.url === '/extra-nav-a')?.extra, 'a');
      assert.equal(identityHeaders.find(h => h.url === '/extra-nav-b')?.extra, null, 'no leak to a sibling page');
      assert.deepEqual(await read(a), { extra: 'a', cache: 'no-cache' });
      assert.deepEqual(await read(b), { extra: null, cache: null });
      await a.setExtraHTTPHeaders({ 'X-Extra': 'replaced' });
      assert.deepEqual(await read(a), { extra: 'replaced', cache: null });
      await a.setExtraHTTPHeaders({});
      assert.deepEqual(await read(a), { extra: null, cache: null });
    } finally { await shared.close(); }
  });
  await check('HTTP cache: hits report fromDiskCache, setCacheDisabled is page-scoped, clearBrowserCache empties the context', async () => {
    const shared = await browser.newContext();
    try {
      const a = await shared.newPage();
      const b = await shared.newPage();
      await a.goto(`${fixtureOrigin}/cache-a`, { timeout: 5000 });
      await b.goto(`${fixtureOrigin}/cache-b`, { timeout: 5000 });
      const cdpA = await shared.newCDPSession(a);
      await cdpA.send('Network.enable');
      const fromDisk = [];
      cdpA.on('Network.responseReceived', e => { if (e.response.url.includes('/cacheable')) fromDisk.push(e.response.fromDiskCache); });
      const load = p => p.evaluate(() => fetch('/cacheable').then(r => r.text()));
      const before = cacheableHits.length;
      await load(a);
      await load(a);
      await load(b);
      assert.equal(cacheableHits.length - before, 1, 'one network request, then hits in both pages');
      await cdpA.send('Network.setCacheDisabled', { cacheDisabled: true });
      await load(a);
      await load(b);
      assert.equal(cacheableHits.length - before, 2, 'A bypasses, sibling B still hits');
      await cdpA.send('Network.setCacheDisabled', { cacheDisabled: false });
      await cdpA.send('Network.clearBrowserCache');
      await load(b);
      assert.equal(cacheableHits.length - before, 3, 'cleared for the whole context');
      assert.deepEqual(fromDisk, [false, true, false]);
    } finally { await shared.close(); }
  });
  await check('Web Storage follows context, page and navigation boundaries', async () => {
    await page.evaluate(() => { localStorage.setItem('local-marker', 'shared'); sessionStorage.setItem('session-marker', 'page'); });
    const sibling = await context.newPage();
    const other = await browser.newContext();
    try {
      await sibling.goto(`${fixtureOrigin}/sibling`, { timeout: 5000 });
      assert.deepEqual(await sibling.evaluate(() => [localStorage.getItem('local-marker'), sessionStorage.getItem('session-marker')]), ['shared', null]);
      const isolated = await other.newPage();
      await isolated.goto(`${fixtureOrigin}/isolated`, { timeout: 5000 });
      assert.deepEqual(await isolated.evaluate(() => [localStorage.getItem('local-marker'), sessionStorage.getItem('session-marker')]), [null, null]);
      await page.goto(`${fixtureOrigin}/next`, { timeout: 5000 });
      assert.deepEqual(await page.evaluate(() => [localStorage.getItem('local-marker'), sessionStorage.getItem('session-marker')]), ['shared', 'page']);
    } finally { await sibling.close(); await other.close(); }
  });
  await check('parser-created frames load with the parent and are replaced on parent reload', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frames-parsed`, { timeout: 5000 });
      assert.equal(p.frames().length, 3);
      const old = p.frame({ name: 'parsed' });
      assert.equal(await old.locator('#message').textContent(), 'native DOM');
      assert.equal(await p.frameLocator('iframe[name="inline"]').locator('p').textContent(), 'inline child');
      await p.reload({ timeout: 5000 });
      assert.equal(old.isDetached(), true);
      assert.equal(p.frames().length, 3);
      assert.notEqual(p.frame({ name: 'parsed' }), old);
      assert.equal(await p.frameLocator('iframe[name="parsed"]').locator('#message').textContent(), 'native DOM');
    } finally { await p.close(); }
  });
  await check('sandbox srcdoc frameLocator uses real child worlds and native owner identity', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
      await p.evaluate(() => {
        const f = document.createElement('iframe'); f.id = 'sandbox'; f.name = 'sandbox';
        f.setAttribute('sandbox', 'allow-downloads');
        f.srcdoc = '<p>sandbox content</p><script>globalThis.authorRan = true</script>';
        document.body.appendChild(f);
      });
      assert.equal(await p.frameLocator('#sandbox').locator('p').textContent(), 'sandbox content');
      const owner = await p.locator('#sandbox').elementHandle();
      const f = await owner.contentFrame();
      assert.ok(f && f !== p.mainFrame());
      assert.equal(f.parentFrame(), p.mainFrame());
      assert.equal(await f.evaluate(() => typeof authorRan), 'undefined');
      assert.equal(await p.evaluate(() => document.querySelector('iframe').contentDocument), null);
      const resolvedOwner = await f.frameElement();
      assert.equal(await resolvedOwner.evaluate(el => el.id), 'sandbox');
      await resolvedOwner.dispose(); await owner.dispose();
      // Child-frame geometry is in main-frame viewport coordinates (measured
      // in Chrome: iframe at 8,26 304x154; <p> 284x18 at 16,42). The child
      // viewport starts inside the iframe's 2px border, at 18,44 for a
      // same-process frame (the field check below); Chrome runs this opaque
      // sandboxed frame out of process and reports it 2px up and left.
      const frameBox = await p.locator('#sandbox').boundingBox();
      assert.deepEqual(frameBox, { x: 8, y: 26, width: 304, height: 154 });
      const pBox = await f.locator('p').boundingBox();
      assert.ok(pBox, 'child geometry is reported');
      assert.deepEqual([pBox.x, pBox.y, Math.round(pBox.height)], [18, 44, 18]);
      assert.ok(Math.abs(pBox.width - 284) <= 1, JSON.stringify(pBox));
      assert.deepEqual(await f.evaluate(() => [innerWidth, innerHeight, matchMedia('(max-width: 300px)').matches]),
        [300, 150, true]);
      // Chrome: restyling the iframe resizes the child viewport (500x80,
      // the <p> 484 wide); padding and borders are outside it.
      await p.evaluate(() => { document.getElementById('sandbox').style.cssText = 'width: 500px; height: 80px; padding: 3px; border: 4px solid'; });
      assert.deepEqual(await f.evaluate(() => [innerWidth, innerHeight]), [500, 80]);
      assert.ok(Math.abs((await f.locator('p').boundingBox()).width - 484) <= 1);
    } finally { await p.close(); }
  });
  await check('child window resize follows its iframe box like Chrome', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
      await p.evaluate(() => {
        const f = document.createElement('iframe'); f.id = 'resized'; f.name = 'resized'; f.srcdoc = '<p>x</p>';
        document.body.appendChild(f);
      });
      assert.equal(await p.frameLocator('#resized').locator('p').textContent(), 'x');
      const f = p.frame({ name: 'resized' });
      await f.evaluate(() => { globalThis.seen = []; onresize = () => seen.push([innerWidth, innerHeight]); });
      // Chrome: [[500,150]] after the restyle, nothing more after a 900px
      // viewport (the box is unchanged), then [442,150] and [392,150].
      await p.evaluate(() => { document.getElementById('resized').style.width = '500px'; });
      await f.waitForFunction(() => seen.length === 1);
      await p.setViewportSize({ width: 900, height: 600 });
      await p.waitForTimeout(100);
      assert.deepEqual(await f.evaluate(() => seen), [[500, 150]]);
      await p.evaluate(() => { document.getElementById('resized').style.width = '50%'; });
      await f.waitForFunction(() => seen.length === 2);
      await p.setViewportSize({ width: 800, height: 600 });
      await f.waitForFunction(() => seen.length === 3);
      assert.deepEqual(await f.evaluate(() => seen), [[500, 150], [442, 150], [392, 150]]);
    } finally { await p.close(); }
  });
  await check('child HTTP navigation, response body, fetch and route retain frame identity', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
      await p.evaluate(() => {
        const f = document.createElement('iframe'); f.name = 'http-child'; f.srcdoc = '<p>initial</p>';
        document.body.appendChild(f);
      });
      assert.equal(await p.frameLocator('iframe').locator('p').textContent(), 'initial');
      const f = p.frame({ name: 'http-child' });
      assert.ok(f);
      const response = await f.goto(`${fixtureOrigin}/frame-http`, { timeout: 5000 });
      assert.equal(response.request().frame(), f);
      assert.match(await response.text(), /Binding fixture/);
      assert.equal(await f.locator('#message').textContent(), 'native DOM');
      const responseWait = p.waitForResponse(`${fixtureOrigin}/json`, { timeout: 5000 });
      assert.deepEqual(await f.evaluate(() => fetch('/json').then(r => r.json())), { answer: 47 });
      const fetched = await responseWait;
      assert.equal(fetched.request().frame(), f);
      assert.deepEqual(await fetched.json(), { answer: 47 });
      await p.route('**/frame-route', async route => {
        assert.equal(route.request().frame(), f);
        await route.fulfill({ status: 200, contentType: 'text/html', body: '<p>routed child</p>' });
      });
      const routed = await f.goto(`${fixtureOrigin}/frame-route`, { timeout: 5000 });
      assert.equal(routed.request().frame(), f);
      assert.equal(await f.locator('p').textContent(), 'routed child');
      assert.equal(p.url(), `${fixtureOrigin}/frame-parent`);
    } finally { await p.close(); }
  });
  await check('parent and child locator fill and keyboard input follow native focus across sandbox modes', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      for (const sandbox of [null, 'allow-scripts', '']) {
        await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
        await p.evaluate(sandbox => {
          const root = document.createElement('input'); root.id = 'root'; document.body.appendChild(root);
          for (const name of ['a', 'b']) {
            const f = document.createElement('iframe'); f.id = name; f.name = name;
            if (sandbox !== null) f.setAttribute('sandbox', sandbox);
            f.srcdoc = '<input id="field">'; document.body.appendChild(f);
          }
        }, sandbox);
        const a = p.frameLocator('#a').locator('#field');
        const b = p.frameLocator('#b').locator('#field');
        await p.locator('#root').fill('parent');
        await a.fill('child');
        await a.press('x');
        assert.equal(await a.inputValue(), 'childx');
        assert.equal(await p.locator('#root').inputValue(), 'parent');
        await b.fill('sibling');
        await p.keyboard.insertText('!');
        assert.equal(await b.inputValue(), 'sibling!');
        await p.locator('#root').focus();
        await p.keyboard.insertText('R');
        assert.equal(await p.locator('#root').inputValue(), 'parentR');
        assert.equal(await a.inputValue(), 'childx');
        assert.equal(await b.inputValue(), 'sibling!');
        // Child-frame geometry in main-frame coordinates: Chrome puts #a's
        // field at 171,36 (153x21): iframe at 161,26 + 2px border + 8px body
        // margin. #b's is 304px further right.
        const [boxA, boxB] = [await a.boundingBox(), await b.boundingBox()];
        assert.ok(boxA && boxB, 'child geometry is reported');
        assert.ok(Math.abs(boxA.x - 171) <= 1 && Math.abs(boxA.y - 36) <= 1, JSON.stringify(boxA));
        assert.ok(Math.abs(boxA.width - 153) <= 2 && Math.abs(boxA.height - 21) <= 2, JSON.stringify(boxA));
        assert.equal(boxB.x - boxA.x, 304, 'the second iframe is one iframe width (300 + 2 x 2 border) further');
      }
    } finally { await p.close(); }
  });
  await check('raw key focus switch suppresses one char and nested detach returns input to root body', async () => {
    const p = await context.newPage(); p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
      await p.evaluate(() => {
        const root = document.createElement('input'); root.id = 'root'; document.body.appendChild(root);
        const f = document.createElement('iframe'); f.id = 'outer'; f.name = 'outer';
        f.srcdoc = '<input id="field"><iframe id="inner" name="inner"></iframe>'; document.body.appendChild(f);
      });
      const outer = p.frameLocator('#outer');
      await outer.locator('#field').waitFor();
      await p.evaluate(() => {
        const root = document.querySelector('#root');
        root.addEventListener('keydown', () => document.querySelector('iframe').contentDocument.querySelector('#field').focus(), { once: true });
        root.focus();
      });
      const cdp = await context.newCDPSession(p);
      await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'x', code: 'KeyX' });
      await cdp.send('Input.dispatchKeyEvent', { type: 'char', key: 'x', text: 'x' });
      assert.equal(await outer.locator('#field').inputValue(), '');
      await cdp.send('Input.dispatchKeyEvent', { type: 'char', key: 'x', text: 'x' });
      assert.equal(await outer.locator('#field').inputValue(), 'x');
      assert.equal(await p.locator('#root').inputValue(), '');
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'x', code: 'KeyX' });
      const f = p.frame({ name: 'outer' });
      await f.evaluate(() => { document.querySelector('#inner').srcdoc = '<input id="nested">'; });
      await outer.frameLocator('#inner').locator('#nested').fill('nested');
      await f.evaluate(() => document.querySelector('#inner').remove());
      await p.evaluate(() => { globalThis.keyTargets = []; document.addEventListener('keydown', e => keyTargets.push(e.target.tagName)); });
      await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'z', code: 'KeyZ' });
      assert.deepEqual(await p.evaluate(() => keyTargets), ['BODY']);
      assert.equal(await p.evaluate(() => document.activeElement.tagName), 'BODY');
      assert.equal(await f.evaluate(() => document.activeElement.tagName), 'BODY');
      await cdp.detach();
    } finally { await p.close(); }
  });
  await check('page.exposeFunction and exposeBinding survive navigation (Runtime.addBinding)', async () => {
    const p = await context.newPage();
    try {
      const calls = [];
      await p.exposeFunction('compatSum', (a, b) => { calls.push([a, b]); return a + b; });
      await p.exposeBinding('compatFrame', ({ frame }) => frame === p.mainFrame());
      assert.equal(await p.evaluate(() => compatSum(2, 3)), 5, 'exposed on the open page');
      await p.goto(`${fixtureOrigin}/document`, { timeout: 5000 });
      assert.equal(await p.evaluate(() => compatSum(10, 20)), 30, 'kept across navigation');
      assert.equal(await p.evaluate(() => compatFrame()), true);
      assert.deepEqual(calls, [[2, 3], [10, 20]]);
    } finally { await p.close(); }
  });
  await check('nested child removal preserves sibling worlds and invalidates old handles', async () => {
    const p = await context.newPage();
    p.setDefaultTimeout(5000);
    try {
      await p.goto(`${fixtureOrigin}/frame-parent`, { timeout: 5000 });
      await p.evaluate(() => {
        for (const name of ['outer', 'sibling']) {
          const f = document.createElement('iframe'); f.id = name; f.name = name; f.srcdoc = '<p>' + name + '</p>';
          document.body.appendChild(f);
        }
      });
      assert.equal(await p.frameLocator('#outer').locator('p').textContent(), 'outer');
      assert.equal(await p.frameLocator('#sibling').locator('p').textContent(), 'sibling');
      const outer = p.frame({ name: 'outer' }); const sibling = p.frame({ name: 'sibling' });
      await sibling.evaluate(() => { globalThis.marker = 23; });
      await outer.evaluate(() => {
        const f = document.createElement('iframe'); f.id = 'nested'; f.name = 'nested'; f.srcdoc = '<p>nested</p>';
        document.body.appendChild(f);
      });
      assert.equal(await p.frameLocator('#outer').frameLocator('#nested').locator('p').textContent(), 'nested');
      const nested = p.frame({ name: 'nested' });
      assert.equal(nested.parentFrame(), outer);
      const old = await nested.evaluateHandle(() => document.body);
      const detached = p.waitForEvent('framedetached', { predicate: f => f === outer, timeout: 5000 });
      await p.evaluate(() => document.getElementById('outer').remove());
      await detached;
      assert.equal(nested.isDetached(), true);
      assert.equal(outer.isDetached(), true);
      await assert.rejects(() => old.evaluate(el => el.textContent));
      await old.dispose();
      assert.equal(await sibling.evaluate(() => marker), 23);
      assert.equal(p.frames().length, 2);
    } finally { await p.close(); }
  });
} finally {
  fixture.closeAllConnections();
  await new Promise(resolve => fixture.close(resolve));
}
// A local forward proxy: absolute-form requests answered here, CONNECT tunnelled
// to a local TLS server whatever the host asked for.
{
  const data = path.join(here, '..', '..', 'crates', 'lp-net', 'tests', 'data');
  const certificate = {
    cert: new X509Certificate(readFileSync(path.join(data, 'self-signed.der'))).toString(),
    key: createPrivateKey({ key: readFileSync(path.join(data, 'self-signed.key.der')), format: 'der', type: 'pkcs8' })
      .export({ format: 'pem', type: 'pkcs8' }),
  };
  const h2 = http2.createSecureServer(certificate, (req, res) => {
    res.setHeader('content-type', 'text/html');
    res.end(`<title>${req.httpVersion}</title>`);
  });
  await new Promise(resolve => h2.listen(0, '127.0.0.1', resolve));
  const tls = https.createServer(certificate, (req, res) => {
    res.setHeader('content-type', 'text/html');
    res.end(`<title>tunnelled ${req.headers.host}</title>`);
  });
  await new Promise(resolve => tls.listen(0, '127.0.0.1', resolve));
  const seen = [];
  const allowed = (header) => header === `Basic ${Buffer.from('user:pass').toString('base64')}`;
  const proxy = (auth) => {
    const server = http.createServer((req, res) => {
      seen.push(`${req.method} ${req.url}`);
      if (auth && !allowed(req.headers['proxy-authorization'])) {
        res.writeHead(407, { 'proxy-authenticate': 'Basic realm="e2e"' }).end();
        return;
      }
      res.setHeader('content-type', 'text/html');
      res.end(`<title>proxied ${req.url}</title>`);
    });
    server.on('connect', (req, client) => {
      seen.push(`CONNECT ${req.url}`);
      if (auth && !allowed(req.headers['proxy-authorization'])) {
        client.end('HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm="e2e"\r\n\r\n');
        return;
      }
      const upstream = net.connect(tls.address().port, '127.0.0.1', () => {
        client.write('HTTP/1.1 200 Connection established\r\n\r\n');
        upstream.pipe(client);
        client.pipe(upstream);
      });
      upstream.on('error', () => client.destroy());
      client.on('error', () => upstream.destroy());
    });
    return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve(server)));
  };
  const open = await proxy(false);
  const guarded = await proxy(true);
  const address = (s) => `http://127.0.0.1:${s.address().port}`;
  const titleVia = async (options, url) => {
    const c = await browser.newContext(options);
    try {
      const p = await c.newPage();
      await p.goto(url);
      return await p.title();
    } finally { await c.close(); }
  };
  try {
    await check('newContext proxy: http goes absolute-form through it', async () => {
      assert.equal(await titleVia({ proxy: { server: address(open) } }, 'http://site.invalid/x?y=1'),
        'proxied http://site.invalid/x?y=1');
      assert.ok(seen.includes('GET http://site.invalid/x?y=1'), seen.join());
    });
    await check('newContext proxy: https goes through a CONNECT tunnel', async () => {
      assert.equal(await titleVia({ proxy: { server: address(open) }, ignoreHTTPSErrors: true },
        'https://site.invalid/'), 'tunnelled site.invalid');
      assert.ok(seen.includes('CONNECT site.invalid:443'), seen.join());
    });
    await check('newContext proxy: username and password answer both kinds of 407', async () => {
      const options = { proxy: { server: address(guarded), username: 'user', password: 'pass' }, ignoreHTTPSErrors: true };
      assert.equal(await titleVia(options, 'http://auth.invalid/'), 'proxied http://auth.invalid/');
      assert.equal(await titleVia(options, 'https://auth.invalid/'), 'tunnelled auth.invalid');
    });
    await check('newContext proxy: bypass goes direct, failures name the proxy', async () => {
      const before = seen.length;
      await assert.rejects(titleVia({ proxy: { server: address(open), bypass: '*.invalid' } }, 'http://site.invalid/'),
        /ERR_NAME_NOT_RESOLVED/);
      assert.equal(seen.length, before);
      await assert.rejects(titleVia({ proxy: { server: address(guarded) } }, 'https://auth.invalid/'),
        /ERR_TUNNEL_CONNECTION_FAILED/);
      await assert.rejects(titleVia({ proxy: { server: 'http://127.0.0.1:9' } }, 'http://site.invalid/'),
        /ERR_PROXY_CONNECTION_FAILED/);
    });
    await check('https negotiates HTTP/2 and reports it in Network', async () => {
      const c = await browser.newContext({ ignoreHTTPSErrors: true });
      try {
        const p = await c.newPage();
        const session = await c.newCDPSession(p);
        await session.send('Network.enable');
        const received = new Promise(resolve => session.once('Network.responseReceived', e => resolve(e.response)));
        const response = await p.goto(`https://127.0.0.1:${h2.address().port}/`);
        assert.equal(await p.title(), '2.0');
        assert.equal(response.statusText(), '');
        assert.equal((await received).protocol, 'h2');
      } finally { await c.close(); }
    });
    await check('newContext proxy: SOCKS is refused explicitly', async () => {
      await assert.rejects(browser.newContext({ proxy: { server: 'socks5://127.0.0.1:1' } }), /SOCKS proxies are not supported/);
    });
  } finally {
    for (const s of [open, guarded, tls]) {
      s.closeAllConnections();
      s.close();
    }
    h2.close();
  }
}
await check('second context is independent', async () => {
  const other = await browser.newContext();
  const p2 = await other.newPage();
  await p2.evaluate(() => { globalThis.marker = 'second'; });
  assert.equal(await page.evaluate(() => typeof globalThis.marker), 'undefined');
  await other.close();
});
await check('close', async () => {
  await page.close();
  await context.close();
  await browser.close();
});

clearTimeout(deadline);
child.kill('SIGTERM');
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ startupMs, passed: results.length - failed.length, total: results.length, results }, null, 2));
process.exit(failed.length ? 1 : 0);
