// wptserve's pipes (tools/wptserve/wptserve/pipes.py) for serve.mjs's static files. The last `pipe` query parameter
// names functions separated by `|`, each with arguments in parentheses separated by `,` (`\` escapes the next
// character; `\n`, `\r` and `\t` are control characters), applied in order. `sub` (substitution) is not applied:
// pages that need it are named `.sub.` and left out. trickle is the one pages in our sets depend on:
// html/dom/render-blocking loads every script and stylesheet with `trickle(d1)` so that it is still loading when the
// page looks; answered at once, whether such a load had finished was a race with the page's frames.
import http from 'node:http';
import zlib from 'node:zlib';

const int = (s) => {
  if (!/^\s*[+-]?\d+\s*$/.test(s)) throw new Error(`not an integer: ${s}`);
  return Number(s);
};
const float = (s) => {
  const n = Number(s);
  if (!s.trim() || Number.isNaN(n)) throw new Error(`not a number: ${s}`);
  return n;
};
const boolean = (s) => {
  if (['true', '1'].includes(s.toLowerCase())) return true;
  if (['false', '0'].includes(s.toLowerCase())) return false;
  throw new Error(`not a boolean: ${s}`);
};

// r: { status, headers (lower-case names), body (Buffer) }, changed in place.
const PIPES = {
  status: (r, code) => { r.status = int(code); },
  header: (r, name, value, append = 'false') => {
    const k = name.toLowerCase();
    r.headers[k] = boolean(append) && r.headers[k] != null ? [r.headers[k]].flat().concat(value) : value;
  },
  // Python slice semantics; "null" leaves that end open.
  slice: (r, start, end) => {
    const at = (s) => (s == null || s.toLowerCase() === 'null' ? undefined : int(s));
    r.body = r.body.subarray(at(start), at(end));
    r.headers['content-length'] = String(r.body.length);
  },
  // Python's GzipFile compresses at level 9.
  gzip: (r) => {
    r.body = zlib.gzipSync(r.body, { level: 9 });
    r.headers['content-encoding'] = 'gzip';
    r.headers['content-length'] = String(r.body.length);
  },
  // `d<seconds>` waits, `<n>` sends n bytes, and the rest of the body follows the last step; a last step
  // `r<even n>` instead repeats the n steps before it until the whole body is sent. Adjacent steps of one kind add
  // up (`d1:d2` is `d3`). Without a Cache-Control, Pragma or Expires header the response is marked uncacheable.
  // The headers go out at once (wptserve writes them before the body), with no Content-Length unless an earlier
  // pipe set one (gzip, slice).
  trickle: (r, spec) => {
    const steps = [];
    for (const item of spec.split(':')) {
      const kind = { d: 'delay', r: 'repeat' }[item[0]] ?? 'bytes';
      const value = kind === 'delay' ? float(item.slice(1)) : int(kind === 'bytes' ? item : item.slice(1));
      if (kind === 'repeat' && value % 2 !== 0) throw new Error(`trickle: odd repeat count ${item}`);
      if (steps.at(-1)?.[0] === kind) steps.at(-1)[1] += value;
      else steps.push([kind, value]);
    }
    r.trickle = steps;
    if (!['cache-control', 'pragma', 'expires'].some((k) => k in r.headers)) {
      Object.assign(r.headers, { 'cache-control': 'no-cache, no-store, must-revalidate', pragma: 'no-cache', expires: '0' });
    }
  },
};

// PipeTokenizer + Pipeline.parse: [[name, [args...]], ...].
export function parsePipes(s) {
  const fns = [];
  let i = 0;
  let inArgs = false;
  while (i < s.length || inArgs) {
    let tok = '';
    if (!inArgs) {
      for (; i < s.length; i++) {
        if (s[i] === '(') { inArgs = true; i++; break; }
        if (s[i] === '|') { if (tok) { i++; break; } continue; }
        tok += s[i];
      }
      if (tok || inArgs) fns.push([tok, []]);
      continue;
    }
    let last = true;
    for (; i < s.length; i++) {
      if (s[i] === '\\') { i++; if (i < s.length) tok += { n: '\n', r: '\r', t: '\t' }[s[i]] ?? s[i]; continue; }
      if (s[i] === ',') { last = false; i++; break; }
      if (s[i] === ')') { i++; break; }
      tok += s[i];
    }
    if (last) inArgs = false;
    fns.at(-1)[1].push(tok);
  }
  return fns;
}

// The response `r` after the pipes of request URL `u` (a new object; `r` is left as it was). Throws where wptserve
// answers with a server error (an argument it cannot convert). A name it does not know is skipped. As parse_qs,
// blank parameters do not count.
export function pipeline(u, r) {
  const spec = u.searchParams.getAll('pipe').filter(Boolean).at(-1);
  if (!spec) return r;
  r = { ...r, headers: { ...r.headers } };
  for (const [name, args] of parsePipes(spec)) if (Object.hasOwn(PIPES, name)) PIPES[name](r, ...args);
  return r;
}

// trickle's add_content for a body of `size` bytes: ['bytes', from, to] and ['delay', seconds] in order.
export function* trickleSteps(steps, size) {
  let at = 0;
  function* run(list, repeat) {
    for (const [i, [kind, value]] of list.entries()) {
      if (kind === 'bytes') { yield ['bytes', at, at + value]; at += value; }
      else if (kind === 'delay') yield ['delay', value];
      else if (i === list.length - 1) {
        const loop = list.slice(-(value + 1), -1);
        // wptserve loops forever when the repeated steps send nothing; here the rest follows.
        if (!loop.some(([k, v]) => k === 'bytes' && v > 0)) break;
        while (at < size) yield* run(loop, true);
      }
    }
    if (!repeat && at < size) { yield ['bytes', at, size]; at = size; }
  }
  yield* run(steps, false);
}

// Writes `r` to the node:http response `res`, trickled if a trickle pipe ran.
export function send(res, r) {
  try {
    res.writeHead(r.status, r.headers);
  } catch {
    // Node refuses header values wptserve sends as they are (a `header()` pipe's control or
    // non-Latin-1 characters): answer with the same bytes, written by hand, instead of leaving
    // the request unanswered (text-module/ignore-content-type.any.html timed out).
    sendRaw(res, r);
    return;
  }
  if (!r.trickle) { res.end(r.body); return; }
  res.flushHeaders();
  const steps = trickleSteps(r.trickle, r.body.length);
  let timer = null;
  res.on('close', () => clearTimeout(timer));
  const next = () => {
    for (let s = steps.next(); !s.done; s = steps.next()) {
      if (res.destroyed) return;
      const [kind, a, b] = s.value;
      if (kind === 'delay') { timer = setTimeout(next, a * 1000); return; }
      if (b > a && a < r.body.length) res.write(r.body.subarray(a, b));
    }
    res.end();
  };
  next();
}

/** The response `r` written straight to the socket, its header values as they are (UTF-8). */
function sendRaw(res, r) {
  const socket = res.socket;
  if (!socket || socket.destroyed) return;
  const lines = [`HTTP/1.1 ${r.status} ${http.STATUS_CODES[r.status] ?? ''}`];
  let length = false;
  for (const [name, value] of Object.entries(r.headers)) {
    if (value == null) continue;
    length ||= name.toLowerCase() === 'content-length';
    for (const v of [value].flat()) lines.push(`${name}: ${v}`);
  }
  if (!length) lines.push(`content-length: ${r.body.length}`);
  lines.push('connection: close', '', '');
  // As Node's own path: no body for HEAD, 204 or 304.
  const bodiless = res.req?.method === 'HEAD' || r.status === 204 || r.status === 304;
  const head = Buffer.from(lines.join('\r\n'), 'utf8');
  socket.end(bodiless ? head : Buffer.concat([head, r.body]));
}
