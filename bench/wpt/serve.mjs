// Minimal static server for a WPT checkout: plain files plus generated .any.html / .window.html wrappers
// (the part of wptserve testharness tests need when they use no Python handlers, pipes or substitutions).
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(process.argv[2]);
const PORT = Number(process.argv[3] ?? 9415);
const TYPES = { '.html': 'text/html', '.htm': 'text/html', '.xhtml': 'application/xhtml+xml', '.xml': 'application/xml', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png', '.gif': 'image/gif', '.jpg': 'image/jpeg', '.svg': 'image/svg+xml', '.txt': 'text/plain', '.woff': 'font/woff', '.ttf': 'font/ttf' };

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

http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  let rel; try { rel = decodeURIComponent(u.pathname).replace(/^\/+/, ''); } catch { res.writeHead(400); res.end(); return; }
  const m = rel.match(/^(.*)\.(any|window)\.html$/);
  if (m) {
    const js = path.join(ROOT, `${m[1]}.${m[2]}.js`);
    if (js.startsWith(ROOT) && fs.existsSync(js)) {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(wrapper(`${m[1]}.${m[2]}.js`, fs.readFileSync(js, 'utf8')));
      return;
    }
  }
  let file = path.join(ROOT, rel);
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
}).listen(PORT, '127.0.0.1', () => console.log(`wpt static on ${PORT}`));
process.on('uncaughtException', (e) => console.error('uncaught', e.message));
