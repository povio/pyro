#!/usr/bin/env bun
// Instructions per DOM call from page script, for one or more browser binaries.
//
//   bun scripts/percall.mjs --bin base=/abs/pyro [--bin new=/abs/pyro]
//       --sampler /abs/rss-sampler [--rounds 5] [--calls 2000000] [--filter REGEX]
//       [--profile] [--profile-secs 4] [--out x.json] [--md x.md]
//
// Each case is a function compiled once in the page (`function (n) { for (...) { <one call> } }`), warmed
// up until the optimizing JIT has it, then called with `--calls` iterations per round. The browser's
// instructions retired (all threads, from the native sampler: macOS only) are read before and after the
// evaluate; the same loop without the call (`loop.empty`, or `loop.walk` for the node walks) is subtracted,
// so a row is what one call adds to a loop that already has its receiver. Receivers come from an array of
// 64 elements (`els[i & 63]`), so no tier can hoist the call out of the loop. The value reported is the
// median over the rounds (min next to it: helper threads only ever add).
//
// `--profile` also splits each case's time by where it is spent, from `/usr/bin/sample` on a loop of the
// same function: `jit` (machine code JSC generated: the loop and the inline cache's call sequence),
// `jsc-api` (JSC's C API and callback-object machinery: JSCallbackObject, JSObjectGetPrivate, JSValueMake*,
// OpaqueJSString, the API lock), `jsc-core` (the rest of JSC/WTF/bmalloc: cell and string allocation,
// structures, the collector), `ours` (Rust: pyro-js dispatch, pyro-webapi, pyro-dom) and `sys` (libsystem under
// none of them). A sample is attributed to the nearest frame at or above its top that falls in one of those
// classes, so a malloc called by Rust counts as ours and one called by JSC as JSC's. Shares are of time, not
// instructions; the instruction columns apply them to the per-call count as an estimate.
import { spawn, execFile } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const argv = process.argv.slice(2);
const opt = (name) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
const opts = (name) => argv.flatMap((a, i) => (a === name ? [argv[i + 1]] : []));
const BINS = opts('--bin').map((b) => {
  const eq = b.indexOf('=');
  return eq > 0 ? { name: b.slice(0, eq), path: b.slice(eq + 1) } : { name: path.basename(path.dirname(b)), path: b };
});
const SAMPLER = opt('--sampler');
const ROUNDS = Number(opt('--rounds') ?? 5);
const CALLS = Number(opt('--calls') ?? 2_000_000);
const FILTER = opt('--filter') ? new RegExp(opt('--filter')) : null;
const PROFILE = argv.includes('--profile');
const PROFILE_SECS = Number(opt('--profile-secs') ?? 4);
if (!BINS.length || !SAMPLER) {
  console.error('usage: percall.mjs --bin name=/abs/browser [--bin ...] --sampler /abs/rss-sampler [--rounds N] [--calls N]');
  console.error('       [--filter REGEX] [--profile] [--profile-secs N] [--out x.json] [--md x.md]');
  process.exit(2);
}

// ---------- the page ----------
// 64 sibling elements `els[k]` (`<div id=eK class=cK data-k=vK>` with a text node, a `<span>` and a
// `<b>`), `kids[k]` the span of `els[k]`, and a detached-then-refilled `walk` host for the node walks.
const PAGE_SETUP = `(() => {
  const host = document.createElement('section');
  host.id = 'host';
  let html = '';
  for (let k = 0; k < 64; k++) html += '<div id="e' + k + '" class="c' + k + '" data-k="v' + k + '">t' + k + '<span>s</span><b>b</b></div>';
  host.innerHTML = html;
  document.body.appendChild(host);
  const walk = document.createElement('section');
  walk.id = 'walk';
  document.body.appendChild(walk);
  window.__els = Array.from(host.children);
  window.__kids = window.__els.map((e) => e.children[0]);
  window.__ids = window.__els.map((e) => e.id);
  window.__h = () => {};
  window.__walk = walk;
  window.__walkHtml = '<i></i>'.repeat(WALK);
  // Touch every wrapper the cases expect to exist already.
  let n = 0;
  for (const e of window.__els) { n += e.firstChild.nodeType + e.parentNode.nodeType + e.lastChild.nodeType; }
  return n;
})()`;

// Nodes per walk: every `nextSibling` of a walk right after `innerHTML` makes a wrapper.
const WALK = 20_000;

// `body` runs once per iteration with `e = els[i & 63]`, `c = kids[i & 63]`, `id = ids[i & 63]`, `i`,
// `doc`, `h` in scope and adds to `acc`. `calls`: DOM calls per iteration (default 1).
const CASES = [
  { name: 'loop.empty', what: 'the loop alone (subtracted from every row below)', body: 'acc += e !== null ? 1 : 0;', baseline: true },
  // --- the five the task names ---
  { name: 'getter.number', what: '`el.nodeType` (no attribute read, returns a number)', body: 'acc += e.nodeType;' },
  { name: 'getter.string', what: '`el.id` (returns an atom string the wrapper keeps)', body: 'acc += e.id.length;' },
  { name: 'getter.node.existing', what: '`el.parentNode` (wrapper exists)', body: 'acc += e.parentNode !== null ? 1 : 0;' },
  { name: 'getter.node.new', what: '`node.nextSibling` over nodes that have no wrapper yet', walk: 'fresh' },
  { name: 'method.string', what: "`el.hasAttribute('data-k')` (string in, boolean out)", body: "acc += e.hasAttribute('data-k') ? 1 : 0;" },
  { name: 'method.node', what: '`el.contains(child)` (node in, boolean out)', body: 'acc += e.contains(c) ? 1 : 0;' },
  // --- more of the same path ---
  { name: 'getter.number.count', what: '`el.childElementCount`', body: 'acc += e.childElementCount;' },
  { name: 'getter.bool', what: '`el.isConnected`', body: 'acc += e.isConnected ? 1 : 0;' },
  { name: 'getter.string.tagName', what: '`el.tagName` (shared name string)', body: 'acc += e.tagName.length;' },
  { name: 'getter.string.className', what: '`el.className`', body: 'acc += e.className.length;' },
  { name: 'getter.string.textContent', what: '`el.textContent` (a new 4-character string per call)', body: 'acc += e.textContent.length;' },
  { name: 'getter.node.firstChild', what: '`el.firstChild` (wrapper exists)', body: 'acc += e.firstChild !== null ? 1 : 0;' },
  { name: 'getter.node.walk', what: '`node.nextSibling` over the same nodes again (wrappers exist)', walk: 'again' },
  { name: 'method.noarg', what: '`el.hasChildNodes()`', body: 'acc += e.hasChildNodes() ? 1 : 0;' },
  { name: 'method.string.getAttribute', what: "`el.getAttribute('data-k')` (string in, string out)", body: "acc += e.getAttribute('data-k').length;" },
  { name: 'method.string.getElementById', what: '`document.getElementById(id)` (string in, node out)', body: 'acc += doc.getElementById(id) !== null ? 1 : 0;' },
  { name: 'method.string.setAttribute', what: "`el.setAttribute('data-j', 'v')` (two strings in, same value)", body: "e.setAttribute('data-j', 'v'); acc++;" },
  { name: 'method.string.matches', what: "`el.matches('div')`", body: "acc += e.matches('div') ? 1 : 0;" },
  { name: 'method.string.classList', what: "`el.classList.contains('c1')` (getter + method)", body: "acc += e.classList.contains('c1') ? 1 : 0;", calls: 2 },
  { name: 'method.node.compare', what: '`el.compareDocumentPosition(child)`', body: 'acc += e.compareDocumentPosition(c);' },
  { name: 'method.node.appendChild', what: '`el.appendChild(child)` (child is the first of three: a move)', body: 'e.appendChild(e.firstChild); acc++;', calls: 2 },
  { name: 'method.event', what: "`el.addEventListener('click', f)` + `removeEventListener`", body: "e.addEventListener('click', h); e.removeEventListener('click', h); acc++;", calls: 2 },
  { name: 'setter.string.className', what: "`el.className = 'c1'`", body: "e.className = 'c1'; acc++;" },
  { name: 'setter.string.textContent', what: "`span.textContent = 'p1'`", body: "c.textContent = 'p1'; acc++;" },
  { name: 'create.element', what: "`document.createElement('p')` (node + wrapper, collected later)", body: "acc += doc.createElement('p') !== null ? 1 : 0;" },
  {
    name: 'mutate.element',
    what: 'one element of `dom.mutate.1k`: createElement, className, textContent, appendChild (the box of 1000 is removed after each thousand)',
    mutate: true,
    calls: 4,
  },
  // The same loop cut short, to see what each step of it adds (per element, not per call).
  { name: 'mutate.step.create', what: 'the `mutate.element` loop with only `createElement`', mutate: true, steps: 1 },
  { name: 'mutate.step.class', what: '... and `className`', mutate: true, steps: 2 },
  { name: 'mutate.step.text', what: '... and `textContent`', mutate: true, steps: 3 },
].filter((c) => c.baseline || !FILTER || FILTER.test(c.name));

function caseSource(c) {
  if (c.walk) {
    // `n` is ignored: one walk of WALK nodes. `fresh` refills the host first (unmeasured, in `prepare`).
    return `function (n) { let acc = 0; for (let x = window.__walk.firstChild; x !== null; x = x.nextSibling) acc++; return acc; }`;
  }
  if (c.mutate) {
    const steps = c.steps ?? 4;
    return `function (n) { const doc = document; let acc = 0; let box = null;
      for (let i = 0; i < n; i++) {
        if (i % 1000 === 0) { if (box) box.remove(); box = doc.createElement('div'); doc.body.appendChild(box); }
        const a = 'x' + (i % 10), b = 'p' + (i % 1000);
        const e = doc.createElement('p');${steps > 1 ? ' e.className = a;' : ''}${steps > 2 ? ' e.textContent = b;' : ''}${steps > 3 ? ' box.appendChild(e);' : ''} acc += a.length + b.length > 0 ? 1 : 0;
      }
      if (box) box.remove();
      return acc; }`;
  }
  return `function (n) { const els = window.__els, kids = window.__kids, ids = window.__ids, doc = document, h = window.__h; let acc = 0;
    for (let i = 0; i < n; i++) { const e = els[i & 63], c = kids[i & 63], id = ids[i & 63]; ${c.body} }
    return acc; }`;
}
// The mutate loop's own baseline: the same string building and counters without the DOM.
const MUTATE_BASE = `function (n) { let acc = 0; let box = null;
  for (let i = 0; i < n; i++) { if (i % 1000 === 0) { box = i; } const a = 'x' + (i % 10), b = 'p' + (i % 1000); acc += a.length + b.length > 0 ? 1 : 0; }
  return acc + (box === null ? 0 : 0); }`;

// ---------- browser + CDP ----------
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function startBrowser(bin) {
  const port = 41000 + Math.floor(Math.random() * 2000);
  const cp = spawn(bin, ['serve', '--host', '127.0.0.1', '--port', String(port), '--log-level', 'error'], { stdio: 'ignore' });
  let v;
  for (let t = 0; ; t++) {
    try {
      v = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      break;
    } catch {
      if (t > 2000) throw new Error(`browser did not start: ${bin}`);
      await sleep(5);
    }
  }
  const ws = new WebSocket(v.webSocketDebuggerUrl);
  await new Promise((r, j) => ((ws.onopen = r), (ws.onerror = j)));
  let id = 0;
  const pend = new Map();
  ws.onmessage = (m) => {
    const j = JSON.parse(m.data);
    if (j.id && pend.has(j.id)) {
      pend.get(j.id)(j);
      pend.delete(j.id);
    }
  };
  const send = (method, params = {}, sessionId) =>
    new Promise((res, rej) => {
      const i = ++id;
      pend.set(i, (j) => (j.error ? rej(new Error(`${method}: ${JSON.stringify(j.error)}`)) : res(j.result)));
      ws.send(JSON.stringify({ id: i, method, params, sessionId }));
    });
  const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  const ev = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true }, sessionId);
    if (r.exceptionDetails) throw new Error(`evaluate failed: ${JSON.stringify(r.exceptionDetails).slice(0, 400)}\n${expression.slice(0, 200)}`);
    return r.result.value;
  };
  return { cp, pid: cp.pid, ev, stop: () => cp.kill('SIGKILL') };
}

function snapshot(pid) {
  return new Promise((res, rej) =>
    execFile(SAMPLER, [String(pid), '-', '--once', '--instructions'], (err, out) => {
      if (err) return rej(err);
      const c = out.trim().split(/\s+/).map(Number);
      res({ cpuNs: c[3] + c[4], instructions: c[6] });
    }),
  );
}

const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2;
};

// ---------- `sample` call graph -> time by class ----------
const CLASSES = ['jit', 'jsc-api', 'jsc-core', 'ours', 'sys'];
const WAITING =
  /^(__psynch_cvwait|__psynch_mutexwait|mach_msg2_trap|mach_msg_trap|kevent|kevent64|__workq_kernreturn|__select|poll|__semwait_signal|semaphore_wait_trap|semaphore_timedwait_trap|__ulock_wait|__ulock_wait2|read|__recvfrom|recvfrom|start_wqthread|_pthread_wqthread|__accept|accept)$/;
// A readable name for a Rust v0 symbol (`sample` does not demangle them): its identifiers, in order.
function readable(symbol) {
  if (!symbol.startsWith('_R')) return symbol;
  const ids = [];
  const re = /(\d+)_?([A-Za-z][A-Za-z0-9_]*)/g;
  symbol = symbol.replace(/Cs[0-9A-Za-z]+_/g, 'C'); // crate disambiguators
  for (let m; (m = re.exec(symbol)); ) {
    const len = Number(m[1]);
    if (len > 0 && len <= m[2].length) {
      ids.push(m[2].slice(0, len));
      re.lastIndex = m.index + m[1].length + (symbol[m.index + m[1].length] === '_' ? 1 : 0) + len;
    }
  }
  return ids.length ? ids.join('::') : symbol;
}
function classify(symbol, image) {
  if (symbol === '???' || /^0x[0-9a-f]+$/.test(symbol) || image === '<unknown binary>') return 'jit';
  if (/^(JSC::JSCallback|JSC::APICallbackShim|JSC::JSAPI|JSC::JSLock|JSC::APIEntryShim|OpaqueJS|JSObject[A-Z]|JSValue[A-Z]|JSString[A-Z]|JSContext[A-Z]|JSClass[A-Z]|JSWeak|JSGlobalContext|toJS\(|toRef\()/.test(symbol)) return 'jsc-api';
  if (/JSCallbackObject|JSCallbackFunction|JSCustomGetter|JSCustomSetter|OpaqueJSClass|OpaqueJSString/.test(symbol)) return 'jsc-api';
  if (/^(JSC::|WTF::|bmalloc|pas_|bmalloc_|llint_|vmEntry|operation[A-Z]|Inspector::|jsc_|iso_|jit_|slow_path|g_jsc|ic[A-Z]|handleHostCall|wasm)/.test(symbol)) return 'jsc-core';
  if (/^(void |bool |int |unsigned |auto |std::|decltype|non-virtual thunk|JSC |WTF )/.test(symbol) || /JSC::|WTF::/.test(symbol)) return 'jsc-core';
  if (/^(lp_|pyro|<|core::|alloc::|std::|hashbrown|markup5ever|html5ever|string_cache|selectors|style::|stylo|servo|taffy|parley|tokio|serde|rustc_hash|smallvec|_R|__rust|__rdl|__rg_|rust_|tendril|cssparser|url::|idna|percent_encoding|encoding_rs|bytes::|futures|mio|parking_lot|indexmap|ahash|once_cell|memchr|unicode|regex|blitz|anyrender|peniko|kurbo|fontique|skrifa|read_fonts|swash|icu_|tungstenite|http|hyper|h2::|rustls|ring|webpki|log::|tracing|time::|chrono|rand|getrandom|crossbeam|rayon|dashmap|arc_swap|thin_vec|euclid|app_units|atomic_refcell|to_shmem|malloc_size_of|precomputed_hash|phf|fxhash|servo_arc|static_assertions|bitflags|itoa|ryu|new_debug_unreachable|mac::|utf8|xml5ever|web_atoms|stylo_atoms|dom::|derive_more|num_|byteorder|simd|base64|flate2|miniz|brotli|zstd|image::|png|jpeg|gif|webp|tiny_skia|resvg|usvg|krilla|pdf_writer|subsetter|ttf_parser|rustybuzz|harfrust|yoke|zerovec|zerofrom|tinystr|writeable|litemap|icu::)/.test(symbol)) return 'ours';
  return null; // libsystem and friends: attributed to the caller's class
}
// Returns { total, byClass, top: [[label, samples]...] } for the busiest thread of the sample report.
function parseSample(text, image) {
  const lines = text.split('\n');
  const start = lines.findIndex((l) => l.startsWith('Call graph:'));
  if (start < 0) return null;
  const threads = [];
  let cur = null;
  const re = /^([ +!:|]*)(\d+) (.*?)(?:  \(in ([^)]*)\))?(?: \+ [\d,.]+)?\s*(?:\[[^\]]*\])?\s*$/;
  for (let i = start + 1; i < lines.length; i++) {
    const l = lines[i];
    if (!l.trim()) break;
    const m = re.exec(l);
    if (!m) continue;
    const depth = m[1].length;
    const node = { depth, count: Number(m[2]), symbol: m[3].trim(), image: m[4], kids: [] };
    if (/^Thread_\w+/.test(node.symbol)) {
      cur = { root: node, stack: [node] };
      threads.push(cur);
      continue;
    }
    if (!cur) continue;
    while (cur.stack.length > 1 && cur.stack[cur.stack.length - 1].depth >= depth) cur.stack.pop();
    cur.stack[cur.stack.length - 1].kids.push(node);
    cur.stack.push(node);
  }
  const results = threads.map((t) => {
    const byClass = Object.fromEntries(CLASSES.map((c) => [c, 0]));
    const top = new Map();
    let total = 0;
    const visit = (n, inherited) => {
      const own = classify(n.symbol, n.image);
      const cls = own ?? inherited;
      const self = n.count - n.kids.reduce((a, k) => a + k.count, 0);
      if (self > 0 && !WAITING.test(n.symbol)) {
        const c = cls ?? 'sys';
        byClass[c] += self;
        total += self;
        const label = `${c}  ${readable(n.symbol).slice(0, 120)}`;
        top.set(label, (top.get(label) ?? 0) + self);
      }
      for (const k of n.kids) visit(k, cls);
    };
    for (const k of t.root.kids) visit(k, null);
    return { total, byClass, top: [...top.entries()].sort((a, b) => b[1] - a[1]).slice(0, 14) };
  });
  results.sort((a, b) => b.total - a.total);
  return results[0] ?? null;
}

async function profileCase(b, c, callsPerEval, image) {
  const file = path.join(os.tmpdir(), `percall-${process.pid}-${c.name}.sample`);
  let done = false;
  const sampler = new Promise((res) =>
    execFile('/usr/bin/sample', [String(b.pid), String(PROFILE_SECS), '1', '-mayDie', '-file', file], { timeout: 120_000 }, () => {
      done = true;
      res();
    }),
  );
  while (!done) {
    if (c.walk === 'fresh') await b.ev(`window.__walk.innerHTML = window.__walkHtml; 0`);
    await b.ev(`window.__f[${JSON.stringify(c.name)}](${callsPerEval})`);
  }
  await sampler;
  let parsed = null;
  try {
    parsed = parseSample(fs.readFileSync(file, 'utf8'), image);
    if (argv.includes('--keep-samples')) console.error(`  sample kept: ${file}`);
    else fs.unlinkSync(file);
  } catch {}
  return parsed;
}

// ---------- run ----------
async function runBin(bin) {
  const b = await startBrowser(bin.path);
  const out = {};
  try {
    await b.ev(PAGE_SETUP.replace('WALK', String(WALK)));
    await b.ev(`window.__f = {}; window.__f['mutate.base'] = ${MUTATE_BASE}; 0`);
    for (const c of CASES) await b.ev(`window.__f[${JSON.stringify(c.name)}] = ${caseSource(c)}; 0`);
    const measure = async (name, n, prepare) => {
      if (prepare) await b.ev(prepare);
      const before = await snapshot(b.pid);
      await b.ev(`window.__f[${JSON.stringify(name)}](${n})`);
      const after = await snapshot(b.pid);
      return { instructions: after.instructions - before.instructions, cpuNs: after.cpuNs - before.cpuNs };
    };
    const series = async (name, n, prepare, warm) => {
      // Warm-up: the function reaches the top tier and its inline caches settle.
      for (let w = 0; w < warm; w++) {
        if (prepare) await b.ev(prepare);
        await b.ev(`window.__f[${JSON.stringify(name)}](${n})`);
      }
      const rounds = [];
      for (let r = 0; r < ROUNDS; r++) rounds.push(await measure(name, n, prepare));
      return rounds;
    };
    const refill = `window.__walk.innerHTML = window.__walkHtml; 0`;
    // Fixed cost of one measured evaluate (for the walks, whose loop is the call itself).
    const evalOnly = await series('loop.empty', 0, null, 3);
    const mutateBase = await series('mutate.base', CALLS / 4, null, 4);
    let empty = null;
    for (const c of CASES) {
      const perIter = (rounds, n, base) => rounds.map((r, i) => (r.instructions - (base ? base[i] ?? median(base) : 0)) / n);
      let n, rounds, baseTotals, cpu;
      if (c.walk) {
        n = WALK;
        if (c.walk === 'again') await b.ev(`if (!window.__walk.firstChild) window.__walk.innerHTML = window.__walkHtml; 0`);
        rounds = await series(c.name, 0, c.walk === 'fresh' ? refill : null, c.walk === 'fresh' ? 6 : 3);
        baseTotals = evalOnly.map((r) => r.instructions);
      } else if (c.mutate) {
        n = CALLS / 4;
        rounds = await series(c.name, n, null, 4);
        baseTotals = mutateBase.map((r) => r.instructions);
      } else {
        n = CALLS;
        rounds = await series(c.name, n, null, 4);
        baseTotals = c.baseline ? null : empty.map((r) => r.instructions);
      }
      if (c.baseline) empty = rounds;
      const per = perIter(rounds, n, baseTotals && [median(baseTotals)]).map((x) => x / (c.calls ?? 1));
      cpu = median(rounds.map((r) => r.cpuNs)) - (baseTotals ? median((c.walk ? evalOnly : c.mutate ? mutateBase : empty).map((r) => r.cpuNs)) : 0);
      const row = {
        what: c.what,
        calls: c.calls ?? 1,
        instructions: Math.round(median(per) * 10) / 10,
        min: Math.round(Math.min(...per) * 10) / 10,
        max: Math.round(Math.max(...per) * 10) / 10,
        cpuNs: Math.round((cpu / n / (c.calls ?? 1)) * 10) / 10,
      };
      if (PROFILE && !c.baseline) {
        const p = await profileCase(b, c, c.walk ? 0 : Math.max(1000, Math.round(n / 4)), path.basename(bin.path));
        if (p && p.total > 0) {
          row.samples = p.total;
          row.share = Object.fromEntries(CLASSES.map((k) => [k, Math.round((p.byClass[k] / p.total) * 1000) / 10]));
          row.top = p.top;
        }
      }
      out[c.name] = row;
      const share = row.share ? '  ' + CLASSES.map((k) => `${k} ${row.share[k]}%`).join(' ') : '';
      console.error(`  ${bin.name.padEnd(8)} ${c.name.padEnd(30)} ${String(row.instructions).padStart(8)} instr  (${row.min}..${row.max})  ${row.cpuNs} ns${share}`);
    }
  } finally {
    b.stop();
  }
  return out;
}

const results = {};
for (const bin of BINS) {
  console.error(`== ${bin.name}: ${bin.path}`);
  results[bin.name] = await runBin(bin);
}

// ---------- report ----------
const names = CASES.map((c) => c.name);
const md = [];
const fmt = (x) => (x == null ? '' : x.toFixed(1));
md.push(`Instructions per call (median of ${ROUNDS} rounds, ${CALLS.toLocaleString('en-US')} iterations each; the empty loop is subtracted from every other row). CPU ns per call next to it as context.`);
md.push('');
md.push(`| case | what | ${BINS.map((b) => `${b.name} instr | ${b.name} ns`).join(' | ')}${BINS.length === 2 ? ' | delta' : ''} |`);
md.push(`|---|---|${BINS.map(() => '---:|---:').join('|')}|${BINS.length === 2 ? '---:|' : ''}`);
for (const n of names) {
  const rows = BINS.map((b) => results[b.name][n]);
  const cells = rows.map((r) => `${fmt(r.instructions)} | ${fmt(r.cpuNs)}`).join(' | ');
  let delta = '';
  if (BINS.length === 2) {
    const [a, z] = rows;
    delta = ` | ${a.instructions > 0 ? (((z.instructions - a.instructions) / a.instructions) * 100).toFixed(1) + '%' : ''}`;
  }
  md.push(`| \`${n}\` | ${rows[0].what} | ${cells}${delta} |`);
}
if (PROFILE) {
  for (const b of BINS) {
    md.push('');
    md.push(`Where the time of each case goes in \`${b.name}\` (\`sample\`, ${PROFILE_SECS} s per case, the busiest thread; shares of time, with the loop itself inside \`jit\`), and the same shares applied to the loop's instructions per iteration (empty loop included, so the columns add up to row + \`loop.empty\`):`);
    md.push('');
    md.push(`| case | samples | ${CLASSES.map((c) => `${c} %`).join(' | ')} | ${CLASSES.map((c) => `${c} instr`).join(' | ')} |`);
    md.push(`|---|---:|${CLASSES.map(() => '---:').join('|')}|${CLASSES.map(() => '---:').join('|')}|`);
    const emptyRow = results[b.name]['loop.empty'];
    for (const n of names) {
      const r = results[b.name][n];
      if (!r.share) continue;
      const c = CASES.find((x) => x.name === n);
      const loopInstr = r.instructions * r.calls + (c.walk || c.mutate ? 0 : emptyRow.instructions);
      md.push(`| \`${n}\` | ${r.samples} | ${CLASSES.map((k) => fmt(r.share[k])).join(' | ')} | ${CLASSES.map((k) => fmt((r.share[k] / 100) * loopInstr)).join(' | ')} |`);
    }
    md.push('');
    md.push('<details><summary>Top frames per case</summary>');
    md.push('');
    for (const n of names) {
      const r = results[b.name][n];
      if (!r.top) continue;
      md.push(`\`${n}\``);
      md.push('');
      md.push('```');
      for (const [label, count] of r.top) md.push(`${((count / r.samples) * 100).toFixed(1).padStart(5)}%  ${label}`);
      md.push('```');
      md.push('');
    }
    md.push('</details>');
  }
}
const text = md.join('\n') + '\n';
console.log(text);
if (opt('--md')) fs.writeFileSync(opt('--md'), text);
if (opt('--out')) fs.writeFileSync(opt('--out'), JSON.stringify({ rounds: ROUNDS, calls: CALLS, walk: WALK, bins: BINS, results }, null, 2));
process.exit(0);
