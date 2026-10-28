// `LP.run` steps added for the TOS conversion, through a Playwright CDP
// session against `lightningpanda serve --static`: network mocks (json,
// delay for a loading state, abort for an error state, times, unmock),
// `url` waits, `reload`, hover, dblclick, blur, type, and css / focused /
// accessible-name checks and reads.
//
// Usage: node tests/e2e/lp-run-steps.mjs /abs/path/to/lightningpanda
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.join(here, '..', '..', 'bench', 'package.json'));
const { chromium } = require('playwright-core');
const bin = process.argv[2];
if (!bin) { console.error('usage: node tests/e2e/lp-run-steps.mjs /abs/path/to/lightningpanda'); process.exit(2); }
const results = [];
const deadline = setTimeout(() => { console.error(`lp-run-steps: deadline exceeded; ${JSON.stringify(results)}`); process.exit(1); }, 60_000);
const freePort = () => new Promise((resolve, reject) => { const s = net.createServer(); s.on('error', reject); s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => resolve(port)); }); });

const appPort = await freePort();
const origin = `http://127.0.0.1:${appPort}`;
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'lp-run-'));
const dist = path.join(tmp, 'dist');
fs.mkdirSync(dist, { recursive: true });
fs.writeFileSync(path.join(dist, 'index.html'), `<!doctype html><html><head><style>
#tip{display:none} #cell:hover + #tip{display:block} #box{color:rgb(1, 2, 3);width:120px;height:40px}
</style></head><body>
<div id="cell">Cell</div><div id="tip">Tooltip</div><div id="box"></div>
<input id="t" aria-label="Title"><button id="dbl">Twice</button><div id="dbl-out"></div><div id="blur-out"></div>
<button id="load" aria-label="Load items">Load</button> <span id="status">idle</span> <ul id="items"></ul>
<a id="nav" href="/dashboard/settings">Settings</a>
<iframe id="fr" srcdoc="<button id='inner'>Inner</button><p id='inner-out'></p><script>document.getElementById('inner').addEventListener('click', () => { document.getElementById('inner-out').textContent = 'inner clicked'; });</script>"></iframe>
<input id="file" type="file" multiple><p id="file-out"></p>
<div id="src" draggable="true">Drag me</div><div id="dst">Drop here</div><p id="drag-out"></p>
<div id="psrc">Pointer</div><div id="pdst">Pointer target</div><p id="pointer-out"></p>
<a id="dl" href="/report.csv" download="report.csv">Export</a>
<a id="tab" href="/other" target="_blank">Open tab</a>
<script>
  const t = document.getElementById('t');
  t.addEventListener('blur', () => { document.getElementById('blur-out').textContent = 'blurred:' + t.value; });
  document.getElementById('dbl').addEventListener('dblclick', (e) => { document.getElementById('dbl-out').textContent = 'dbl:' + e.detail; });
  document.getElementById('nav').addEventListener('click', (e) => { e.preventDefault(); history.pushState(null, '', '/dashboard/settings'); });
  const f = document.getElementById('file');
  f.addEventListener('change', () => { document.getElementById('file-out').textContent = [...f.files].map((x) => x.name + ':' + x.type + ':' + x.size).join(','); });
  const src = document.getElementById('src'), dst = document.getElementById('dst');
  src.addEventListener('dragstart', (e) => { e.dataTransfer.setData('text/plain', 'payload'); });
  dst.addEventListener('dragover', (e) => e.preventDefault());
  dst.addEventListener('drop', (e) => { e.preventDefault(); document.getElementById('drag-out').textContent = 'dropped:' + e.dataTransfer.getData('text/plain'); });
  const psrc = document.getElementById('psrc'); let down = false;
  psrc.addEventListener('mousedown', () => { down = true; });
  document.addEventListener('mouseup', (e) => { if (down) { down = false; document.getElementById('pointer-out').textContent = 'released on ' + e.target.id; } });
  document.getElementById('load').addEventListener('click', async () => {
    const status = document.getElementById('status'); status.textContent = 'loading';
    try {
      const r = await fetch('/api/items?page=1');
      if (!r.ok) { status.textContent = 'error ' + r.status; return; }
      const items = await r.json();
      document.getElementById('items').innerHTML = items.map((i) => '<li>' + i.name + '</li>').join('');
      status.textContent = 'done';
    } catch (e) { status.textContent = 'failed'; }
  });
</script></body></html>`);

fs.writeFileSync(path.join(dist, 'report.csv'), 'a,b\n1,2\n');
fs.writeFileSync(path.join(dist, 'other.html'), '<!doctype html><title>other</title>');
fs.writeFileSync(path.join(tmp, 'notes.txt'), 'hello');
const port = await freePort();
const child = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error', '--experimental-features', 'cors', '--static', `${origin}=${dist}`], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: 'true', LIGHTNINGPANDA_GEOMETRY: 'flow' } });
let stderr = ''; child.stderr.on('data', (d) => { stderr = (stderr + d).slice(-4000); });
process.on('exit', () => { try { child.kill('SIGKILL'); } catch {} try { fs.rmSync(tmp, { recursive: true, force: true }); } catch {} });
async function check(name, fn) { const t0 = performance.now(); try { await fn(); results.push({ name, ok: true, ms: +(performance.now() - t0).toFixed(1) }); } catch (e) { results.push({ name, ok: false, error: String(e?.stack ?? e).split('\n').slice(0, 4).join('\n') }); } }
const started = performance.now();
for (;;) { try { if ((await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(500) })).ok) break; } catch {} if (child.exitCode !== null) { console.error(`exited early: ${stderr}`); process.exit(1); } if (performance.now() - started > 10_000) { console.error(`startup timed out: ${stderr}`); process.exit(1); } await new Promise((r) => setTimeout(r, 10)); }

const browser = await chromium.connectOverCDP(`ws://127.0.0.1:${port}`);
const page = await (await browser.newContext()).newPage();
const cdp = await page.context().newCDPSession(page);
const run = (steps, extra = {}) => cdp.send('LP.run', { steps, ...extra });

await check('element steps: hover, css, type, focused, name, blur, dblclick', async () => {
  const r = await run([
    { goto: { url: origin + '/' } },
    { act: { locator: { css: '#cell' }, action: 'hover' } },
    { check: { locator: { css: '#tip' }, assert: 'visible' } },
    { check: { locator: { css: '#box' }, assert: 'css', property: 'color', expected: 'rgb(1, 2, 3)' } },
    { read: { locator: { css: '#box' }, read: 'css', property: 'width', as: 'width' } },
    { act: { locator: { label: 'Title' }, action: 'type', value: 'Hi 2' } },
    { check: { locator: { label: 'Title' }, assert: 'focused' } },
    { check: { locator: { label: 'Title' }, assert: 'name', expected: 'Title' } },
    { act: { locator: { label: 'Title' }, action: 'blur' } },
    { check: { locator: { css: '#blur-out' }, assert: 'text', expected: 'blurred:Hi 2' } },
    { act: { locator: { css: '#dbl' }, action: 'dblclick' } },
    { check: { locator: { css: '#dbl-out' }, assert: 'text', expected: 'dbl:2' } },
    { read: { locator: { role: 'button', name: 'Load items' }, read: 'name', as: 'name' } },
  ]);
  assert.equal(r.reads.width, '120px'); assert.equal(r.reads.name, 'Load items');
});
await check('mock json, loading state through delay, error state through status and abort', async () => {
  const r = await run([
    { mock: { url: '/api/items', json: [{ name: 'XLF' }, { name: 'XLE' }] } },
    { act: { locator: { role: 'button', name: 'Load items' }, action: 'click' } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'done' } },
    { check: { locator: { css: '#items li' }, assert: 'count', expected: 2 } },
    { request: { path: '/api/items', status: [200, 304] } },
    { unmock: { url: '/api/items' } },
    { mock: { url: '/api/items', json: [], delay: 400 } },
    { act: { locator: { role: 'button', name: 'Load items' }, action: 'click', settle: false } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'loading' } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'done' } },
    { unmock: {} },
    { mock: { url: '/api/items', status: 503, body: 'down', times: 1 } },
    { act: { locator: { role: 'button', name: 'Load items' }, action: 'click' } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'error 503' } },
    { mock: { url: '/api/items', abort: 'ConnectionRefused' } },
    { act: { locator: { role: 'button', name: 'Load items' }, action: 'click' } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'failed' } },
    { unmock: {} },
  ]);
  assert.equal(r.results[4].status, 200);
  assert.ok(r.requests.some((q) => q.error && q.error.includes('CONNECTION_REFUSED')), JSON.stringify(r.requests));
});
await check('url wait after a client-side route, then reload', async () => {
  const r = await run([
    { act: { locator: { role: 'link', name: 'Settings' }, action: 'click' } },
    { url: { url: { regex: '/dashboard/settings$' } } },
    { reload: {} },
    { url: { url: '/dashboard/settings' } },
    { check: { locator: { css: '#status' }, assert: 'text', expected: 'idle' } },
  ]);
  assert.ok(r.results[1].url.endsWith('/dashboard/settings'));
  await assert.rejects(run([{ url: { url: '/nowhere' } }], { timeout: 100 }), /url is/);
});

await check('iframe-scoped locators, upload (inline and path), drag (native and pointer)', async () => {
  const r = await run([
    { goto: { url: origin + '/' } },
    { act: { locator: { css: '#inner', frame: { css: '#fr' } }, action: 'click' } },
    { check: { locator: { css: '#inner-out', frame: { css: '#fr' } }, assert: 'text', expected: 'inner clicked' } },
    { act: { locator: { css: '#file' }, action: 'upload', files: [{ name: 'a.csv', text: 'x,y' }] } },
    { check: { locator: { css: '#file-out' }, assert: 'text', expected: 'a.csv:text/csv:3' } },
    { act: { locator: { css: '#file' }, action: 'upload', value: path.join(tmp, 'notes.txt') } },
    { check: { locator: { css: '#file-out' }, assert: 'text', expected: 'notes.txt:text/plain:5' } },
    { act: { locator: { css: '#src' }, action: 'drag', to: { css: '#dst' } } },
    { check: { locator: { css: '#drag-out' }, assert: 'text', expected: 'dropped:payload' } },
    { act: { locator: { css: '#psrc' }, action: 'drag', to: { css: '#pdst' } } },
    { check: { locator: { css: '#pointer-out' }, assert: 'text', expected: 'released on pdst' } },
  ]);
  assert.equal(r.results[7].native, true); assert.equal(r.results[9].native, false);
});
await check('download saved under its suggested name; popup from a target=_blank link', async () => {
  const [popupPage, r] = await Promise.all([
    page.context().waitForEvent('page'),
    run([
      { act: { locator: { role: 'link', name: 'Export' }, action: 'click', settle: false } },
      { download: { saveTo: path.join(tmp, 'downloads'), as: 'file' } },
      { act: { locator: { role: 'link', name: 'Open tab' }, action: 'click', settle: false } },
      { popup: { as: 'popup' } },
    ]),
  ]);
  assert.equal(r.reads.file.filename, 'report.csv');
  assert.equal(r.reads.file.text, 'a,b\n1,2\n');
  assert.equal(fs.readFileSync(path.join(tmp, 'downloads', 'report.csv'), 'utf8'), 'a,b\n1,2\n');
  assert.equal(r.reads.popup.targetId.length, 32);
  // Playwright sees the same target: the page it reports is the popup.
  const session = await popupPage.context().newCDPSession(popupPage);
  const info = await session.send('Target.getTargetInfo');
  assert.equal(info.targetInfo.targetId, r.reads.popup.targetId);
  await popupPage.close();
});

await browser.close(); clearTimeout(deadline);
const failed = results.filter((r) => !r.ok);
console.log(JSON.stringify({ ok: failed.length === 0, results }, null, 2));
if (failed.length) console.error(`stderr tail:\n${stderr}`);
process.exit(failed.length ? 1 : 0);
