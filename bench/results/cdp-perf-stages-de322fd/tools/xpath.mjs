// usage: bun xpath.mjs trace.xml cpuUsPerOp leafRegex [depth] -> callers chain for matching leaf/first frames
import fs from 'node:fs';
import { demangle } from './demangle.mjs';
const [file, cpuUs, leafRe, depth = 6, mode = 'leaf'] = process.argv.slice(2);
const x = fs.readFileSync(file, 'utf8'); const re = new RegExp(leafRe);
const frames = new Map(), bts = new Map(); const agg = new Map(); let total = 0;
for (const r of x.split('<row>').slice(1)) {
  let st; const bt = r.match(/<tagged-backtrace id="(\d+)">([\s\S]*?)<\/tagged-backtrace>/);
  if (bt) { st = []; for (const f of bt[2].matchAll(/<frame (?:id="(\d+)"[^>]*?name="([^"]*)"|ref="(\d+)")/g)) { if (f[1]) { frames.set(f[1], f[2]); st.push(f[2]); } else st.push(frames.get(f[3])); } bts.set(bt[1], st); }
  else st = bts.get(r.match(/<tagged-backtrace ref="(\d+)"/)?.[1]);
  if (!st || !st.length) continue; total++;
  const d = st.map((s) => demangle(s ?? '?').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&').replace(/\(.*$/, '').slice(0, 60));
  const i = mode === 'leaf' ? (re.test(d[0]) ? 0 : -1) : d.findIndex((f) => re.test(f));
  if (i < 0) continue;
  const k = d.slice(i, i + Number(depth)).join(' < ');
  agg.set(k, (agg.get(k) ?? 0) + 1);
}
const ns = (Number(cpuUs) * 1000) / total;
for (const [k, v] of [...agg].sort((a, b) => b[1] - a[1]).slice(0, 12)) console.log(`${(v * ns).toFixed(0).padStart(7)} ns/op  ${k}`);
