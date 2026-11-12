#!/usr/bin/env bun
// Reads a dump of the sampling heap profiler (a `--features heap-profile` build run with
// PYRO_HEAP_PROFILE=<prefix>, dumped on SIGUSR2; see crates/pyro/src/heap_profile.rs)
// and prints who owns the live Rust heap.
//
//   bun scripts/heap-profile.mjs --bin /abs/pyro <file.heap> [--top 40] [--by owner|crate|leaf] [--stacks N]
//                                [--grep REGEX]
//
// owner  the innermost frame that is not allocator, std or a container library: the function that asked
// crate  that frame's crate
// --stacks N also prints the N largest call stacks (first 14 frames); --grep keeps only stacks with a frame
// matching REGEX.
// mem-states.mjs --heap <prefix> takes a dump of every process at every state.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';

const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const BIN = opt('--bin');
const file = argv.find((a, i) => !a.startsWith('--') && !argv[i - 1]?.startsWith('--'));
const TOP = Number(opt('--top', 40));
const BY = opt('--by', 'owner');
const STACKS = Number(opt('--stacks', 0));
if (!BIN || !file) { console.error('usage: heap-profile.mjs --bin /abs/binary file.heap [--top N] [--by owner|crate|leaf] [--stacks N]'); process.exit(2); }

const lines = fs.readFileSync(file, 'utf8').trim().split('\n');
const head = lines.shift().match(/load (0x[0-9a-f]+) interval (\d+)/);
const load = head[1];
const rows = lines.map((l) => { const [bytes, samples, ...frames] = l.split(' '); return { bytes: Number(bytes), samples: Number(samples), frames }; });
const addrs = [...new Set(rows.flatMap((r) => r.frames))];
const names = new Map();
for (let i = 0; i < addrs.length; i += 2000) {
  const chunk = addrs.slice(i, i + 2000);
  const out = execFileSync('atos', ['-o', BIN, '-l', load, ...chunk], { encoding: 'utf8', maxBuffer: 1 << 28 }).trimEnd().split('\n');
  chunk.forEach((a, j) => names.set(a, (out[j] ?? a).replace(/ \(in [^)]+\)/, '').replace(/ \+ \d+$/, '').replace(/ \([^)]*:\d+\)$/, '')));
}
// atos leaves Rust's v0 names mangled: `rustfilt` (cargo install rustfilt; on PATH or --rustfilt /abs/rustfilt)
// makes them readable. Without it the names stay mangled and the owner guess is poor.
const RUSTFILT = opt('--rustfilt', 'rustfilt');
try {
  const keys = [...names.keys()];
  const out = execFileSync(RUSTFILT, [], { input: keys.map((k) => names.get(k)).join('\n') + '\n', encoding: 'utf8', maxBuffer: 1 << 28 }).split('\n');
  keys.forEach((k, i) => names.set(k, (out[i] ?? names.get(k)).replace(/\[[0-9a-f]{16}\]/g, '')));
} catch { console.error('heap-profile: no rustfilt, names stay mangled'); }
// Frames that only pass an allocation on.
const GENERIC = /^(_?_?rust_|__rdl_|__rg_|alloc::|core::|std::|<alloc::|<core::|<std::|hashbrown::|<hashbrown::|smallvec::|<smallvec::|indexmap::|<indexmap::|mi_|_mi_|pyro::heap_profile|<pyro::heap_profile|<T as |<&T as |<\[|<\(|bytes::|<bytes::|thin_vec::|<thin_vec::|servo_arc::|<servo_arc::|slab::|<slab::|<&mut |<&|<Box|<Vec|<String|0x)/;
const crateOf = (name) => {
  const m = name.replace(/^<+/, '').match(/^([a-zA-Z_][a-zA-Z0-9_]*)::/);
  return m ? m[1] : name.startsWith('JSC') || name.startsWith('WTF') ? 'JavaScriptCore' : '(other)';
};
const GREP = opt('--grep') ? new RegExp(opt('--grep')) : null;
if (GREP) for (let i = rows.length - 1; i >= 0; i--) if (!rows[i].frames.some((f) => GREP.test(names.get(f)))) rows.splice(i, 1);
const total = rows.reduce((a, r) => a + r.bytes, 0);
const sum = new Map();
for (const r of rows) {
  const sym = r.frames.map((f) => names.get(f));
  const owner = sym.find((s) => !GENERIC.test(s)) ?? sym[0] ?? '?';
  const key = BY === 'crate' ? crateOf(owner) : BY === 'leaf' ? sym[0] : owner;
  const e = sum.get(key) ?? { bytes: 0, samples: 0 };
  e.bytes += r.bytes; e.samples += r.samples; sum.set(key, e);
  r.sym = sym;
}
const MB = (b) => (b / 1048576).toFixed(2).padStart(8);
console.log(`# ${file}: ${MB(total).trim()} MB live in ${rows.reduce((a, r) => a + r.samples, 0)} samples (interval ${head[2]} bytes), by ${BY}`);
for (const [k, e] of [...sum].sort((a, b) => b[1].bytes - a[1].bytes).slice(0, TOP)) console.log(`${MB(e.bytes)} MB ${(100 * e.bytes / total).toFixed(1).padStart(5)}%  ${k.slice(0, 150)}`);
if (STACKS) {
  for (const r of [...rows].sort((a, b) => b.bytes - a.bytes).slice(0, STACKS)) {
    console.log(`\n${MB(r.bytes).trim()} MB in ${r.samples} samples`);
    for (const s of r.sym.slice(0, 14)) console.log('    ' + s.slice(0, 160));
  }
}
