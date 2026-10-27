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
  res.writeHead(404, { 'content-type': 'text/plain' });
  res.end('not found');
});

server.keepAliveTimeout = 60_000;
server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
process.on('SIGTERM', () => server.close(() => process.exit(0)));
