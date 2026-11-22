// Usage: blob-controls.cjs <streams.js>
// Ordinary valid-input checks of the byte-stream helpers Blob.stream() (fromBytes)
// and Response.clone() (cloneBody) use, plus byte tee with default and BYOB reads.
const fs = require('node:fs');
const vm = require('node:vm');
const factory = vm.runInThisContext(fs.readFileSync(process.argv[2], 'utf8'), { filename: process.argv[2] });
const S = factory({ slot() { const m = new WeakMap(); return { has: m.has.bind(m), get: m.get.bind(m), set: m.set.bind(m) }; } });
const bytes = (n, k = 0) => Uint8Array.from({ length: n }, (_, i) => (i + k) & 255);
const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
const readAll = async (rs) => {
  const r = rs.getReader(); const out = [];
  for (;;) { const { value, done } = await r.read(); if (done) break; out.push(...value); }
  return Uint8Array.from(out);
};
const readByob = async (rs, size, min) => {
  const r = rs.getReader({ mode: 'byob' }); const out = [];
  for (;;) {
    const { value, done } = await r.read(new Uint8Array(size), min ? { min } : undefined);
    if (value) out.push(...value);
    if (done) break;
  }
  return Uint8Array.from(out);
};
const cases = {
  'fromBytes default read': async () => eq(await readAll(S.fromBytes(bytes(100))), bytes(100)),
  'fromBytes BYOB read': async () => eq(await readByob(S.fromBytes(bytes(100)), 16), bytes(100)),
  'fromBytes BYOB read({min})': async () => eq(await readByob(S.fromBytes(bytes(100)), 32, 4), bytes(100)),
  'fromBytes empty': async () => (await readAll(S.fromBytes(new Uint8Array(0)))).length === 0,
  'fromBytes tee default+default': async () => {
    const [a, b] = S.fromBytes(bytes(64)).tee();
    const [x, y] = await Promise.all([readAll(a), readAll(b)]);
    return eq(x, bytes(64)) && eq(y, bytes(64));
  },
  'fromBytes tee BYOB+default': async () => {
    const [a, b] = S.fromBytes(bytes(64)).tee();
    const [x, y] = await Promise.all([readByob(a, 10), readAll(b)]);
    return eq(x, bytes(64)) && eq(y, bytes(64));
  },
  'fromBytes tee BYOB min+BYOB': async () => {
    const [a, b] = S.fromBytes(bytes(64)).tee();
    const [x, y] = await Promise.all([readByob(a, 16, 4), readByob(b, 7)]);
    return eq(x, bytes(64)) && eq(y, bytes(64));
  },
  'fromBytes tee, cancel branch 1, read branch 2': async () => {
    const [a, b] = S.fromBytes(bytes(64)).tee();
    const canceled = a.cancel('x');
    const y = await readAll(b);
    await canceled;
    return eq(y, bytes(64));
  },
  'cloneBody of fromBytes': async () => {
    const [a, b] = S.cloneBody(S.fromBytes(bytes(50)), (c) => c.slice());
    const [x, y] = await Promise.all([readAll(a), readAll(b)]);
    return eq(x, bytes(50)) && eq(y, bytes(50));
  },
  'cloneBody of fromBytes, branch chunks independent': async () => {
    const [a, b] = S.cloneBody(S.fromBytes(bytes(50)), (c) => c.slice());
    const ra = a.getReader(); const rb = b.getReader();
    const [x, y] = await Promise.all([ra.read(), rb.read()]);
    x.value[0] = 99;
    return y.value[0] === 0 && x.value.buffer !== y.value.buffer;
  },
  'multi-chunk byte source tee, BYOB min across chunks': async () => {
    let i = 0;
    const rs = new S.ReadableStream({ type: 'bytes', pull(c) { if (i < 4) c.enqueue(bytes(3, 3 * i++)); else c.close(); } });
    const [a, b] = rs.tee();
    const r = a.getReader({ mode: 'byob' });
    const first = await r.read(new Uint8Array(8), { min: 7 });
    r.releaseLock();
    const rest = await readAll(a);
    const all = Uint8Array.from([...first.value, ...rest]);
    return first.value.length >= 7 && eq(all, bytes(12)) && eq(await readAll(b), bytes(12));
  },
  'default stream cloneBody still uses cloneChunk': async () => {
    const rs = new S.ReadableStream({ start(c) { c.enqueue({ n: 1 }); c.close(); } });
    const [a, b] = S.cloneBody(rs, (v) => ({ n: v.n + 1 }));
    const [x, y] = await Promise.all([a.getReader().read(), b.getReader().read()]);
    return x.value.n === 1 && y.value.n === 2;
  },
};
setTimeout(() => { console.log(JSON.stringify({ harness: 'timeout' })); process.exit(2); }, 10000).unref?.();
(async () => {
  let pass = 0;
  for (const [name, f] of Object.entries(cases)) {
    let ok = false, err;
    try { ok = await f(); } catch (e) { err = String(e); }
    if (ok) pass++;
    console.log(JSON.stringify({ name, status: ok ? 'Pass' : 'Fail', err }));
  }
  console.log(JSON.stringify({ total: Object.keys(cases).length, pass }));
})();
