// Linux and macOS testharness baselines, page by page: which pages differ (status, subtest count, passes) and which
// subtests pass on one platform only. The testharness gate runs on Linux (wpt.yml `wpt-linux`, baseline-linux.json);
// baseline.json was recorded on macOS. A page that passes less on Linux must be explained in platform-split.json
// ({ "<url>": "<reason>" }), so coverage never drops silently when the gate moves platforms.
//
//   bun bench/wpt/platform-diff.mjs MAC.json LINUX.json [--split platform-split.json] [--flaky flaky-linux.json]
//       [--summary FILE] [--out FILE] [--strict]
//
// --flaky: the Linux-only flaky list. gate.mjs update leaves a flaky subtest out of the baseline, so such a page passes
// less in baseline-linux.json by design; its flaky entry (with the evidence) is the explanation.
//
// --strict exits 1 when a page passes fewer subtests on Linux than on macOS and platform-split.json does not list it.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readBaselineAt, passCount } from './lib.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const files = argv.filter((a, i) => !a.startsWith('--') && !(argv[i - 1]?.startsWith('--') && argv[i - 1] !== '--strict'));
if (files.length !== 2) { console.error('usage: platform-diff.mjs MAC.json LINUX.json [--split FILE] [--summary FILE] [--out FILE] [--strict]'); process.exit(2); }
const [mac, linux] = files.map((f) => readBaselineAt(f));
if (!mac || !linux) { console.error(`platform-diff: missing baseline (${!mac ? files[0] : files[1]})`); process.exit(2); }
const splitFile = opt('--split', path.join(HERE, 'platform-split.json'));
const split = fs.existsSync(splitFile) ? JSON.parse(fs.readFileSync(splitFile, 'utf8')) : {};
const flakyFile = opt('--flaky');
const flaky = flakyFile && fs.existsSync(flakyFile) ? JSON.parse(fs.readFileSync(flakyFile, 'utf8')).tests ?? {} : {};

// The subtests an entry names as passing, when it can be told by name: a `pass` list, or a `fail` list whose names
// hash (h) equals the other side's (the same subtests reported). null when only counts compare.
function passNames(e, other) {
  if (e.pass) return new Set(e.pass);
  if (other?.fail && other.h === e.h) return null; // both fail-form, same names: compare the fail lists instead
  return null;
}
const rows = [];
for (const url of [...new Set([...Object.keys(mac.tests), ...Object.keys(linux.tests)])].sort()) {
  const m = mac.tests[url], l = linux.tests[url];
  const row = { url, mac: m && { status: m.status, n: m.n, pass: passCount(m) }, linux: l && { status: l.status, n: l.n, pass: passCount(l) } };
  if (m && l) {
    if (m.fail && l.fail && m.h === l.h) {
      const mf = new Set(m.fail), lf = new Set(l.fail);
      row.macOnly = l.fail.filter((x) => !mf.has(x));
      row.linuxOnly = m.fail.filter((x) => !lf.has(x));
    } else if (m.pass && l.pass) {
      const mp = passNames(m), lp = passNames(l);
      row.macOnly = [...mp].filter((x) => !lp.has(x));
      row.linuxOnly = [...lp].filter((x) => !mp.has(x));
    }
    const same = m.status === l.status && m.n === l.n && row.mac.pass === row.linux.pass && !row.macOnly?.length && !row.linuxOnly?.length;
    if (same) continue;
  }
  row.lessOnLinux = !l || (m && (row.linux.pass < row.mac.pass || (m.status === 'complete' && l.status !== 'complete') || row.macOnly?.length > 0));
  row.reason = split[url] ?? (flaky[url] ? `${path.basename(flakyFile)}: ${flaky[url].reason}` : null);
  rows.push(row);
}
const less = rows.filter((r) => r.lessOnLinux), unexplained = less.filter((r) => !r.reason);
const sum = (b) => Object.values(b.tests).reduce((a, e) => a + passCount(e), 0);
let md = `## Linux and macOS testharness results\n\nPassing subtests: macOS ${sum(mac)} (${files[0]}), Linux ${sum(linux)} (${files[1]}). ` +
  `${rows.length} page(s) differ; ${less.length} pass less on Linux, ${unexplained.length} of them not explained in ${path.basename(splitFile)}.\n\n`;
if (rows.length) {
  md += '| page | macOS | Linux | macOS only | Linux only | reason |\n|---|---|---|---:|---:|---|\n';
  const fmt = (x) => (x ? `${x.status} ${x.pass}/${x.n}` : 'absent');
  for (const r of rows.slice(0, 300)) md += `| ${r.url} | ${fmt(r.mac)} | ${fmt(r.linux)} | ${r.macOnly?.length ?? '?'} | ${r.linuxOnly?.length ?? '?'} | ${r.reason ?? (r.lessOnLinux ? '**unexplained**' : '')} |\n`;
  if (rows.length > 300) md += `\n…and ${rows.length - 300} more (see the JSON).\n`;
}
console.log(md);
if (opt('--summary')) fs.appendFileSync(opt('--summary'), md);
if (opt('--out')) fs.writeFileSync(opt('--out'), JSON.stringify({ mac: files[0], linux: files[1], rows }, null, 1));
if (argv.includes('--strict') && unexplained.length) process.exitCode = 1;
