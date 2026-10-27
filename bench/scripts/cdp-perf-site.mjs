// Local HTTP fixture for cdp-perf.mjs. Every response is deterministic and `no-store`, so each navigation
// does the same network and parse work in every run.
// Usage: node cdp-perf-site.mjs [port]   (port 0 / omitted => ephemeral; prints "LISTENING <port>")
// Routes:
//   GET /simple            small static page
//   GET /dom?n=N           N `.item` rows (each a div with a span and a link), for parse and query work
//   GET /scripts           10 external scripts + 5 stylesheets + one inline script that checks them
//   GET /s/<i>.js          script that defines and runs a little code, then bumps window.__loaded
//   GET /c/<i>.css         stylesheet
//   GET /form              input, submit button and fetch() round trip (Playwright form flow)
//   GET /click             button that counts clicks, input that mirrors its value
//   GET /api/data.json     JSON {greeting: "Hello, <name>"}
//   GET /api/routed.json   JSON {source: "server"} (fulfilled by the harness in interception scenarios)
//   GET /tos/...           TOS-shaped single-page app (positions overview: settings menu, close / reopen
//                          dialogs); signed in when localStorage "auth-token" is set; boots /tos/app.js
//   GET /tos/app.js        the app plus ~200 KB of module code it evaluates at boot (a small bundle)
//   /tos-api/positions     POST creates; GET /:id; POST /:id/close and /:id/open (in memory, needs the token)
import http from 'node:http';

const html = (title, body) =>
  `<!doctype html>\n<html>\n<head><meta charset="utf-8"><title>${title}</title></head>\n<body>\n${body}\n</body>\n</html>`;

const SIMPLE = html('simple', '<h1>Simple</h1>\n<p>One paragraph of text.</p>');

const domPage = (n) => {
  const rows = [];
  for (let i = 0; i < n; i++) {
    rows.push(`<div class="item" id="i${i}" data-k="${i % 7}"><span>row ${i}</span> <a href="#r${i}">link</a></div>`);
  }
  return html(`dom ${n}`, `<main id="list">\n${rows.join('\n')}\n</main>`);
};
const domCache = new Map();

const SCRIPTS = html(
  'scripts',
  [
    ...Array.from({ length: 5 }, (_, i) => `<link rel="stylesheet" href="/c/${i}.css">`),
    '<div id="out"></div>',
    ...Array.from({ length: 10 }, (_, i) => `<script src="/s/${i}.js"></script>`),
    `<script>
      document.getElementById('out').textContent = 'loaded ' + window.__loaded;
    </script>`,
  ].join('\n'),
);

const script = (i) => `(function () {
  var acc = [];
  for (var j = 0; j < 200; j++) acc.push({ k: 'v' + j, n: j * ${i + 1} });
  window.__sum${i} = acc.reduce(function (a, b) { return a + b.n; }, 0);
  window.__loaded = (window.__loaded || 0) + 1;
})();\n`;

const css = (i) =>
  Array.from({ length: 50 }, (_, j) => `.c${i}-${j} { margin: ${j}px; color: #${String(j).padStart(6, '0')}; }`).join('\n');

const FORM = html(
  'form',
  `<form id="f">
  <label for="name">Name</label>
  <input id="name" name="name" type="text" value="">
  <button id="submit" type="submit">Send</button>
</form>
<button id="routed" type="button">Load routed</button>
<div id="out"></div>
<div id="routed-out"></div>
<script>
  document.getElementById('f').addEventListener('submit', async (e) => {
    e.preventDefault();
    const name = document.getElementById('name').value;
    const r = await fetch('/api/data.json?name=' + encodeURIComponent(name));
    const j = await r.json();
    document.getElementById('out').textContent = j.greeting;
  });
  document.getElementById('routed').addEventListener('click', async () => {
    const r = await fetch('/api/routed.json');
    const j = await r.json();
    document.getElementById('routed-out').textContent = j.source;
  });
</script>`,
);

const CLICK = html(
  'click',
  `<button id="b" type="button" style="position:absolute;left:10px;top:10px;width:100px;height:40px">Click</button>
<input id="t" type="text">
<div id="n">0</div>
<script>
  window.__clicks = 0;
  document.getElementById('b').addEventListener('click', () => {
    window.__clicks++;
    document.getElementById('n').textContent = String(window.__clicks);
  });
</script>`,
);

const send = (res, type, body) => {
  res.writeHead(200, { 'content-type': type, 'cache-control': 'no-store' });
  res.end(body);
};

// ---- TOS-shaped app (positions-close E2E) ----
const TOS_SHELL = html('TOS', '<div id="app"><p>Loading…</p></div>\n<script src="/tos/app.js"></script>');
const TOS_FILLER = Array.from(
  { length: 1500 },
  (_, i) => `__m[${i}] = function (x) { var o = { id: ${i}, k: 'm${i}', v: [x, ${i}, x * ${i}] }; return o.v[2] + o.id; };`,
).join('\n');
const TOS_APP = `var __m = []; ${TOS_FILLER}
for (var __i = 0; __i < __m.length; __i++) __m[__i](__i);
(function () {
  var token = localStorage.getItem('auth-token');
  var app = document.getElementById('app');
  var current = null;
  function api(path, method) {
    return fetch('/tos-api' + path, { method: method || 'GET', headers: { authorization: 'Bearer ' + token } })
      .then(function (r) { if (!r.ok) throw new Error('HTTP ' + r.status); return r.json(); });
  }
  function el(id) { return document.getElementById(id); }
  function draw(toast) {
    var p = current;
    app.innerHTML =
      '<h1>Position <span>' + p.number + '</span></h1>' +
      (p.closed ? '<div class="banner"><span>Position Closed · ' + p.closedAt + ' · Manual · Closed by: E2E Employee</span> <button id="reopen">Reopen Position</button></div>' : '') +
      (toast ? '<div role="status">' + toast + '</div>' : '') +
      '<button id="settings" aria-haspopup="menu">Settings</button>' +
      '<div id="menu" role="menu" style="display:none">' + (p.closed ? '' : '<button id="close">Close Position</button>') + '</div>' +
      '<div id="modal"></div>';
    el('settings').addEventListener('click', function () { el('menu').style.display = 'block'; });
    if (el('close')) el('close').addEventListener('click', function () {
      modal('This position will be closed', '', 'Close Position', function () { return api('/positions/' + p.id + '/close', 'POST'); }, '');
    });
    if (el('reopen')) el('reopen').addEventListener('click', function () {
      modal('This position will be reopened', '<label>Keep open for days <input id="days"></label>', 'Reopen Position',
        function () { return api('/positions/' + p.id + '/open', 'POST'); }, 'Position ' + p.number + ' has been reopened.');
    });
  }
  function modal(message, extra, confirm, action, toast) {
    el('modal').innerHTML = '<div role="dialog" aria-modal="true"><p>' + message + '</p>' + extra +
      '<button id="cancel">Cancel</button> <button id="confirm">' + confirm + '</button></div>';
    el('cancel').addEventListener('click', function () { el('modal').innerHTML = ''; });
    el('confirm').addEventListener('click', function () {
      action().then(function () { return api('/positions/' + current.id); }).then(function (p) { current = p; draw(toast); });
    });
  }
  function render() {
    if (!token) { app.innerHTML = '<h1>Sign in</h1>'; return; }
    var m = location.pathname.match(/^\\/tos\\/platform\\/logistics\\/positions\\/([^/]+)\\/overview$/);
    if (!m) { app.innerHTML = '<h1>Dashboard</h1>'; return; }
    app.innerHTML = '<p>Loading…</p>';
    api('/positions/' + m[1]).then(function (p) { current = p; draw(); });
  }
  window.addEventListener('popstate', render);
  render();
})();
`;
const tosPositions = new Map();
let tosNext = 1000;
function tosApi(req, res, p) {
  if (req.headers.authorization !== 'Bearer e2e-token') return json(res, 401, { error: 'unauthorized' });
  if (req.method === 'POST' && p === '/tos-api/positions') {
    const id = `pos-${++tosNext}`;
    const position = { id, number: `EXP-${tosNext}`, closed: false, closedAt: null };
    tosPositions.set(id, position);
    return json(res, 201, position);
  }
  const m = p.match(/^\/tos-api\/positions\/([^/]+)(\/(close|open))?$/);
  const position = m && tosPositions.get(m[1]);
  if (!position) return json(res, 404, { error: 'not found' });
  if (req.method === 'GET' && !m[2]) return json(res, 200, position);
  if (req.method === 'POST' && m[3] === 'close') Object.assign(position, { closed: true, closedAt: '2026-12-15' });
  else if (req.method === 'POST' && m[3] === 'open') Object.assign(position, { closed: false, closedAt: null });
  else return json(res, 405, { error: 'method' });
  return json(res, 200, position);
}
const json = (res, status, body) => {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify(body));
};

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  const p = url.pathname;
  if (p === '/simple' || p === '/') return send(res, 'text/html; charset=utf-8', SIMPLE);
  if (p === '/dom') {
    const n = Math.min(Number(url.searchParams.get('n') ?? 1000) | 0, 100_000);
    if (!domCache.has(n)) domCache.set(n, domPage(n));
    return send(res, 'text/html; charset=utf-8', domCache.get(n));
  }
  if (p === '/scripts') return send(res, 'text/html; charset=utf-8', SCRIPTS);
  let m = p.match(/^\/s\/(\d+)\.js$/);
  if (m) return send(res, 'text/javascript', script(Number(m[1])));
  m = p.match(/^\/c\/(\d+)\.css$/);
  if (m) return send(res, 'text/css', css(Number(m[1])));
  if (p === '/form') return send(res, 'text/html; charset=utf-8', FORM);
  if (p === '/click') return send(res, 'text/html; charset=utf-8', CLICK);
  if (p === '/api/data.json') {
    return send(res, 'application/json', JSON.stringify({ greeting: `Hello, ${url.searchParams.get('name') ?? ''}` }));
  }
  if (p === '/api/routed.json') return send(res, 'application/json', JSON.stringify({ source: 'server' }));
  if (p === '/tos/app.js') return send(res, 'text/javascript', TOS_APP);
  if (p.startsWith('/tos/')) return send(res, 'text/html; charset=utf-8', TOS_SHELL);
  if (p.startsWith('/tos-api/')) return tosApi(req, res, p);
  res.writeHead(404, { 'content-type': 'text/plain' });
  res.end('not found');
});

server.keepAliveTimeout = 60_000;
server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
process.on('SIGTERM', () => server.close(() => process.exit(0)));
