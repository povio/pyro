#!/usr/bin/env bun
// What the allocator holds, from a heap profile's `.areas` dump (one line per allocator page; written next
// to each `.heap` by a `--features heap-profile` build, see crates/lightningpanda/src/heap_profile.rs):
// committed and in-use bytes by page kind, by owning thread, and by block size. Older dumps with one line
// per size class are read too:
//   per size class: <block size> <pages> <reserved> <committed> <in use>
//   per page:       <block size> <reserved> <committed> <in use> <thread>
//   bun scripts/heap-areas.mjs <file.areas> [--threads] [--classes]
import fs from 'node:fs';
const [file, ...flags] = process.argv.slice(2);
const mb = (n) => (n / 1048576).toFixed(2).padStart(7);
const kind = (b) => (b <= 8192 ? 'small  (<=8K)' : b <= 65536 ? 'medium (<=64K)' : b <= 524288 ? 'large  (<=512K)' : 'huge   (>512K)');
let profiler = 0, me = null, perPage = false;
const rows = [];
for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
  if (!line) continue;
  if (line.startsWith('# profiler')) { profiler = Number(line.split(' ')[2]); continue; }
  if (line.startsWith('# thread')) { me = line.split(' ')[2]; perPage = true; continue; }
  if (line.startsWith('#')) continue;
  const f = line.trim().split(/\s+/).map(Number);
  if (perPage) rows.push({ block: f[0], pages: 1, reserved: f[1], committed: f[2], used: f[3], thread: String(f[4]) });
  else rows.push({ block: f[0], pages: f[1], reserved: f[2], committed: f[3], used: f[4], thread: '-' });
}
const sum = (rs, k) => rs.reduce((a, r) => a + r[k], 0);
const group = (key) => { const m = new Map(); for (const r of rows) { const k = key(r); (m.get(k) ?? m.set(k, []).get(k)).push(r); } return m; };
console.log(`# ${file}: ${sum(rows, 'pages')} pages, reserved ${mb(sum(rows, 'reserved'))} MB, committed ${mb(sum(rows, 'committed'))} MB, in use ${mb(sum(rows, 'used'))} MB (profiler ${mb(profiler)} MB)`);
const table = (title, m, order) => {
  console.log(title);
  const keys = [...m.keys()].sort(order ?? ((a, b) => sum(m.get(b), 'committed') - sum(m.get(a), 'committed')));
  for (const k of keys) { const rs = m.get(k); console.log(`  ${String(k).padEnd(24)} pages ${String(sum(rs, 'pages')).padStart(4)}  committed ${mb(sum(rs, 'committed'))}  in use ${mb(sum(rs, 'used'))}  free ${mb(sum(rs, 'committed') - sum(rs, 'used'))}`); }
};
table('by page kind:', group((r) => kind(r.block)), (a, b) => a.localeCompare(b));
if (perPage) {
  table(`by owning thread (dumping thread ${me}; 0 = abandoned):`, group((r) => (r.thread === me ? `${r.thread} (dumper)` : r.thread)));
  if (flags.includes('--threads')) for (const [t, rs] of group((r) => r.thread)) { const m = new Map(); for (const r of rs) { const k = kind(r.block); (m.get(k) ?? m.set(k, []).get(k)).push(r); } table(`thread ${t}:`, m, (a, b) => a.localeCompare(b)); }
}
if (flags.includes('--classes')) table('by block size:', group((r) => r.block), (a, b) => a - b);
