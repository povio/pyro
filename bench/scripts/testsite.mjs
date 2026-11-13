// Throwaway static test site for the Lightpanda smoke test.
// Usage: node testsite.mjs [port]   (port 0 / omitted => ephemeral; prints "LISTENING <port>")
// Routes:
//   GET /                 HTML page: form + input + submit button + "routed" button, fetch() to JSON endpoints
//   GET /api/data.json    JSON {greeting: "Hello, <name>"}
//   GET /api/routed.json  JSON {source: "server"}  (smoke test overrides this with page.route fulfill)
import http from 'node:http';

const PAGE = `<!doctype html>
<html>
<head><meta charset="utf-8"><title>Pyro smoke page</title></head>
<body>
  <h1>Pyro smoke</h1>
  <form id="f">
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
  </script>
</body>
</html>`;

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  if (url.pathname === '/' || url.pathname === '/index.html') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    return res.end(PAGE);
  }
  if (url.pathname === '/api/data.json') {
    const name = url.searchParams.get('name') ?? '';
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ greeting: `Hello, ${name}` }));
  }
  if (url.pathname === '/api/routed.json') {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ source: 'server' }));
  }
  res.writeHead(404, { 'content-type': 'text/plain' });
  res.end('not found');
});

server.listen(Number(process.argv[2] ?? 0), '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
process.on('SIGTERM', () => server.close(() => process.exit(0)));
