// Hammers one CDP workload on a running serve endpoint for SECONDS; prints ops count.
// usage: bun load.mjs ws://127.0.0.1:PORT/ SITE WORKLOAD SECONDS
const [ws, site, workload, secs] = process.argv.slice(2);
const sock = new WebSocket(ws);
await new Promise((r) => (sock.onopen = r));
let id = 0; const pend = new Map(); const waiters = [];
sock.onmessage = (e) => { const m = JSON.parse(e.data);
  if (m.id !== undefined) { const p = pend.get(m.id); pend.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); }
  else for (let i = waiters.length - 1; i >= 0; i--) if (waiters[i].method === m.method) { waiters[i].res(m.params); waiters.splice(i, 1); } };
const send = (method, params = {}, sessionId) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); sock.send(JSON.stringify(sessionId ? { id: i, method, params, sessionId } : { id: i, method, params })); });
const wait = (method) => new Promise((res) => waiters.push({ method, res }));
async function page(url) {
  const { browserContextId } = await send('Target.createBrowserContext');
  const { targetId } = await send('Target.createTarget', { url: 'about:blank', browserContextId });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  await send('Page.enable', {}, sessionId); await send('Runtime.enable', {}, sessionId);
  if (url) { const l = wait('Page.loadEventFired'); await send('Page.navigate', { url }, sessionId); await l; }
  return { sessionId, targetId, browserContextId };
}
let op;
if (workload === 'getVersion') op = () => send('Browser.getVersion');
else if (workload === 'evaluate') { const p = await page(); op = () => send('Runtime.evaluate', { expression: '1+1', returnByValue: true }, p.sessionId); }
else if (workload === 'callFunctionOn') { const p = await page(); const g = (await send('Runtime.evaluate', { expression: 'globalThis' }, p.sessionId)).result.objectId;
  op = () => send('Runtime.callFunctionOn', { functionDeclaration: 'function(a){return a+1}', objectId: g, arguments: [{ value: 1 }], returnByValue: true }, p.sessionId); }
else if (workload === 'describe+quads') { const p = await page(site + '/dom?n=2000'); let k = 0;
  op = async () => { const o = (await send('Runtime.evaluate', { expression: `document.getElementById('i${k++ % 2000}')` }, p.sessionId)).result.objectId;
    await send('DOM.describeNode', { objectId: o }, p.sessionId); await send('DOM.getContentQuads', { objectId: o }, p.sessionId); await send('Runtime.releaseObject', { objectId: o }, p.sessionId); }; }
else if (workload === 'mutate') { const p = await page(site + '/simple');
  op = () => send('Runtime.evaluate', { expression: `(() => { const box = document.createElement('div'); document.body.appendChild(box); for (let i = 0; i < 1000; i++) { const e = document.createElement('p'); e.className = 'x' + (i % 10); e.textContent = 'p' + i; box.appendChild(e); } const n = box.childNodes.length; box.remove(); return n; })()`, returnByValue: true }, p.sessionId); }
else if (workload === 'qsa') { const p = await page(site + '/dom?n=2000');
  op = () => send('Runtime.evaluate', { expression: `document.querySelectorAll('.item[data-k="3"] span').length`, returnByValue: true }, p.sessionId); }
else if (workload === 'navigate') { const p = await page(); op = async () => { const l = wait('Page.loadEventFired'); await send('Page.navigate', { url: 'about:blank' }, p.sessionId); await l; }; }
else if (workload === 'lifecycle') op = async () => { const p = await page(); await send('Target.closeTarget', { targetId: p.targetId }); await send('Target.disposeBrowserContext', { browserContextId: p.browserContextId }); };
else if (workload === 'largeValue') { const p = await page(); op = () => send('Runtime.evaluate', { expression: 'Array.from({length:1000},(_, i)=>({i, s:"item"+i, f:i/3}))', returnByValue: true }, p.sessionId); }
else if (workload === 'navSimple') { const p = await page(); op = async () => { const l = wait('Page.loadEventFired'); await send('Page.navigate', { url: site + '/simple' }, p.sessionId); await l; }; }
else if (workload === 'cfo38') { const p = await page(); const g = (await send('Runtime.evaluate', { expression: 'globalThis' }, p.sessionId)).result.objectId; const BIG = `function(a) { ${Array.from({ length: 2000 }, (_, i) => `const v${i} = ${i};`).join('\n')} return a + 1; }`;
  op = () => send('Runtime.callFunctionOn', { functionDeclaration: BIG, objectId: g, arguments: [{ value: 1 }], returnByValue: true }, p.sessionId); }
else throw new Error('unknown workload ' + workload);
for (let i = 0; i < 50; i++) await op();
console.log('READY'); 
const end = performance.now() + Number(secs) * 1000; let n = 0; const t0 = performance.now();
while (performance.now() < end) { await op(); n++; }
console.log(`OPS ${n} ${((performance.now() - t0) / n * 1000).toFixed(1)}us/op`);
process.exit(0);
