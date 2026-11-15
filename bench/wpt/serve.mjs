// Minimal static server for a WPT checkout: plain files plus generated .any.html / .window.html wrappers
// (the part of wptserve testharness tests need when they use no pipes, substitutions or Python handlers other than
// those in HANDLERS).
// Usage: bun serve.mjs <wpt-dir> [port]       (WPT_SERVE_LOG=FILE appends "status /path" per request)
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(process.argv[2]);
const PORT = Number(process.argv[3] ?? 9415);
// The report shim run.mjs reads (window.report), served in place of the checkout's own copy so a plain
// upstream checkout works.
const REPORT = fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), 'testharnessreport.js'));
const LOG = process.env.WPT_SERVE_LOG && fs.createWriteStream(process.env.WPT_SERVE_LOG, { flags: 'a' });
const TYPES = {
  '.html': 'text/html', '.htm': 'text/html', '.xhtml': 'application/xhtml+xml', '.xht': 'application/xhtml+xml', '.xml': 'application/xml',
  '.xsl': 'application/xslt+xml', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.json': 'application/json',
  '.txt': 'text/plain', '.png': 'image/png', '.gif': 'image/gif', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.svg': 'image/svg+xml',
  '.webp': 'image/webp', '.avif': 'image/avif', '.bmp': 'image/bmp', '.ico': 'image/x-icon', '.woff': 'font/woff', '.woff2': 'font/woff2',
  '.ttf': 'font/ttf', '.otf': 'font/otf',
};

// The Python handlers this server answers as wptserve would, by path. Support files of pages in the subset request
// them, which discovery does not see (it excludes a page only when the page itself names a .py file).
const HANDLERS = {
  // xhr/resources/delay.py: a text/plain response after `ms` milliseconds (500 without it). Served as a file, the
  // response arrived at once: the XHR timeout pages never saw a timeout, and whether a request was still pending
  // when its frame navigated away was a race.
  'xhr/resources/delay.py': (u, res) => {
    const ms = Number(u.searchParams.get('ms') ?? 500);
    const timer = setTimeout(() => {
      res.writeHead(200, { 'access-control-allow-origin': '*', 'access-control-allow-methods': 'YO', 'content-type': 'text/plain' });
      res.end('TEST_DELAY');
    }, Number.isFinite(ms) ? ms : 500);
    // The page was closed or navigated before the answer was due.
    res.on('close', () => clearTimeout(timer));
  },
};

// Paths wptserve routes to another file (tools/serve/serve.py): the idlharness pages load the WebIDL parser by this
// name.
const ALIASES = { 'resources/WebIDLParser.js': 'resources/webidl2/lib/webidl2.js' };

function wrapper(jsRel, src) {
  const metas = [...src.matchAll(/^\/\/\s*META:\s*(\w+)=(.*)$/gm)].map((m) => [m[1], m[2].trim()]);
  let out = '<!doctype html><meta charset=utf-8>\n';
  for (const [k, v] of metas) if (k === 'title') out += `<title>${v}</title>\n`;
  for (const [k, v] of metas) if (k === 'timeout' && v === 'long') out += '<meta name=timeout content=long>\n';
  out += '<script>self.GLOBAL={isWindow:()=>true,isWorker:()=>false,isShadowRealm:()=>false};</script>\n';
  out += '<script src="/resources/testharness.js"></script>\n<script src="/resources/testharnessreport.js"></script>\n';
  for (const [k, v] of metas) if (k === 'script') out += `<script src="${v}"></script>\n`;
  out += `<div id=log></div>\n<script src="/${jsRel}"></script>\n`;
  return out;
}

const server = http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  let rel; try { rel = decodeURIComponent(u.pathname).replace(/^\/+/, ''); } catch { res.writeHead(400); res.end(); return; }
  if (LOG) res.on('finish', () => LOG.write(`${res.statusCode} /${rel}\n`));
  if (rel === 'resources/testharnessreport.js') { res.writeHead(200, { 'content-type': 'text/javascript' }); res.end(REPORT); return; }
  if (Object.hasOwn(HANDLERS, rel)) { HANDLERS[rel](u, res); return; }
  const m = rel.match(/^(.*)\.(any|window)\.html$/);
  if (m) {
    const js = path.join(ROOT, `${m[1]}.${m[2]}.js`);
    if (js.startsWith(ROOT) && fs.existsSync(js)) {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(wrapper(`${m[1]}.${m[2]}.js`, fs.readFileSync(js, 'utf8')));
      return;
    }
  }
  let file = path.join(ROOT, ALIASES[rel] ?? rel);
  if (!file.startsWith(ROOT)) { res.writeHead(403); res.end(); return; }
  try { if (fs.statSync(file).isDirectory()) file = path.join(file, 'index.html'); } catch {}
  fs.readFile(file, (err, buf) => {
    if (err) { res.writeHead(404, { 'content-type': 'text/plain' }); res.end('not found'); return; }
    const type = TYPES[path.extname(file)] ?? 'application/octet-stream';
    // .headers sidecar files (static header overrides wptserve applies)
    const headers = { 'content-type': type };
    try {
      for (const line of fs.readFileSync(file + '.headers', 'utf8').split('\n')) {
        const i = line.indexOf(':');
        if (i > 0) headers[line.slice(0, i).trim().toLowerCase()] = line.slice(i + 1).trim();
      }
    } catch {}
    res.writeHead(200, headers);
    res.end(buf);
  });
});
// A port already in use is fatal (the runner would otherwise talk to whatever holds it).
server.on('error', (e) => { console.error(`wpt static: ${e.message}`); process.exit(1); });
// Port 0 picks a free port; the first stdout line names the one in use.
server.listen(PORT, '127.0.0.1', () => console.log(`wpt static on ${server.address().port}`));
process.on('uncaughtException', (e) => console.error('uncaught', e.message));
