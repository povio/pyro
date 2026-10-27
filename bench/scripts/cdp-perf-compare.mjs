// Compares two cdp-perf.mjs results and flags regressions.
//
// Usage:
//   node cdp-perf-compare.mjs BASE.json HEAD.json         two runs (each file's only binary, or pick with labels)
//   node cdp-perf-compare.mjs RUN.json --base L --head L   two binaries from one interleaved run (preferred)
// Options:
//   --base-label L / --head-label L   binary label inside BASE.json / HEAD.json (when a file has several)
//   --threshold 0.10                  smallest relative change of the median that counts (default 10%)
//   --alpha 0.05                      significance level of the Mann-Whitney U test on round medians
//   --md OUT.md                       also write the Markdown table to a file
//   --no-fail                         exit 0 even when something regressed
//
// Each round is one fresh browser process, and samples inside a process are correlated (they share a heap, a
// CPU frequency and whatever else the host was doing), so rounds are the unit of evidence. A scenario is a
// regression when the median of head's round medians is slower by at least --threshold and the round medians
// differ under an exact Mann-Whitney U test (p < alpha). Improvements mirror this. With 5 rounds per side,
// p < 0.05 needs near-complete separation; with fewer than 4 rounds nothing can reach it, and large changes
// are reported as "unclear".
// Exit status: 1 when any scenario regressed or failed in head, 0 otherwise, 2 for usage errors.
import fs from 'node:fs';
import { median, mannWhitney } from './cdp-perf-stats.mjs';

const argv = process.argv.slice(2);
const opt = (k, d) => {
  const i = argv.indexOf(k);
  return i >= 0 ? argv[i + 1] : d;
};
const valued = new Set(['--base', '--head', '--base-label', '--head-label', '--threshold', '--alpha', '--md']);
const files = argv.filter((a, i) => !a.startsWith('--') && !valued.has(argv[i - 1]));
const THRESHOLD = Number(opt('--threshold', 0.1));
const ALPHA = Number(opt('--alpha', 0.05));
const usage = (msg) => {
  if (msg) console.error(`cdp-perf-compare: ${msg}`);
  console.error('usage: node cdp-perf-compare.mjs BASE.json HEAD.json | RUN.json --base L --head L  [--threshold 0.1] [--alpha 0.05] [--md out.md] [--no-fail]');
  process.exit(2);
};

const load = (f) => {
  const r = JSON.parse(fs.readFileSync(f, 'utf8'));
  if (r.schema !== 'lightningpanda-cdp-perf/1') usage(`${f}: not a cdp-perf result (schema ${r.schema})`);
  return r;
};
const pickLabel = (report, wanted, file) => {
  const labels = report.bins.map((b) => b.label);
  if (wanted) {
    if (!labels.includes(wanted)) usage(`${file} has no binary labelled ${wanted} (has ${labels.join(', ')})`);
    return wanted;
  }
  if (labels.length !== 1) usage(`${file} has binaries ${labels.join(', ')}; pick one with a label option`);
  return labels[0];
};

let base;
let head;
if (files.length === 1) {
  const r = load(files[0]);
  if (!opt('--base') || !opt('--head')) usage('a single file needs --base and --head');
  base = { report: r, label: pickLabel(r, opt('--base'), files[0]) };
  head = { report: r, label: pickLabel(r, opt('--head'), files[0]) };
} else if (files.length === 2) {
  const a = load(files[0]);
  const b = load(files[1]);
  base = { report: a, label: pickLabel(a, opt('--base-label') ?? opt('--base'), files[0]) };
  head = { report: b, label: pickLabel(b, opt('--head-label') ?? opt('--head'), files[1]) };
} else {
  usage();
}

const pct = (x) => `${x >= 0 ? '+' : ''}${(x * 100).toFixed(1)}%`;
const fmt = (x) => (x == null || !Number.isFinite(x) ? '—' : x >= 100 ? x.toFixed(1) : x.toFixed(3));

const rows = [];
const byName = new Map(base.report.scenarios.map((s) => [s.name, s]));
for (const hs of head.report.scenarios) {
  const bs = byName.get(hs.name);
  const h = hs.results[head.label];
  const b = bs?.results[base.label];
  const row = { name: hs.name, group: hs.group };
  rows.push(row);
  if (!h?.stats?.n) {
    row.verdict = 'FAILED';
    row.note = h?.errors?.[0]?.error?.split('\n')[0] ?? 'no samples';
    continue;
  }
  if (!b?.stats?.n) {
    row.verdict = 'new';
    row.head = h.stats.median;
    continue;
  }
  // Samples inside one process are correlated, so the unit of evidence is a round (one fresh process).
  const bm = median(b.roundMedians);
  const hm = median(h.roundMedians);
  const ratio = hm / bm;
  const { p } = mannWhitney(b.roundMedians, h.roundMedians);
  const rounds = Math.min(b.roundMedians.length, h.roundMedians.length);
  Object.assign(row, { base: bm, head: hm, ratio, p, rounds, cpuBase: b.cpuMsPerOp, cpuHead: h.cpuMsPerOp });
  const big = Math.abs(Math.log(ratio)) >= Math.log(1 + THRESHOLD);
  if (big && p < ALPHA) row.verdict = ratio > 1 ? 'REGRESSION' : 'improvement';
  else if (big) row.verdict = 'unclear';
  else row.verdict = 'same';
  if (b.errors?.length || h.errors?.length) row.note = `${b.errors.length + h.errors.length} failed round(s)`;
}

const order = { FAILED: 0, REGRESSION: 1, unclear: 2, improvement: 3, new: 4, same: 5 };
rows.sort((x, y) => order[x.verdict] - order[y.verdict] || (y.ratio ?? 0) - (x.ratio ?? 0));

const desc = (s) => (base.report === head.report ? s.label : `${s.label} (${s.report.meta.startedAt})`);
const lines = [
  `# CDP performance: ${base.label} → ${head.label}`,
  '',
  `Base: ${desc(base)}, head: ${desc(head)}. Threshold ${pct(THRESHOLD)}, alpha ${ALPHA}.`,
];
if (base.report !== head.report) {
  lines.push('', 'These results come from two separate runs, so host drift between them is not controlled. Prefer one interleaved run (`--bin base=... --bin head=...`).');
  const hb = base.report.meta.host;
  const hh = head.report.meta.host;
  if (hb.hostname !== hh.hostname || hb.cpuModel !== hh.cpuModel) lines.push('', `**Different hosts** (${hb.hostname} / ${hh.hostname}): the comparison is not meaningful.`);
}
lines.push(
  '',
  '| scenario | verdict | base ms | head ms | change | p | rounds | cpu/op base → head |',
  '|---|---|---:|---:|---:|---:|---:|---:|',
);
for (const r of rows) {
  const change = r.ratio ? pct(r.ratio - 1) : '';
  const cpu = r.cpuBase != null && r.cpuHead != null ? `${fmt(r.cpuBase)} → ${fmt(r.cpuHead)} (${pct(r.cpuHead / r.cpuBase - 1)})` : '';
  const verdict = r.verdict === 'REGRESSION' || r.verdict === 'FAILED' ? `**${r.verdict}**` : r.verdict;
  lines.push(
    `| ${r.name} | ${verdict}${r.note ? ` (${r.note})` : ''} | ${fmt(r.base)} | ${fmt(r.head)} | ${change} | ${r.p == null ? '' : r.p < 1e-4 ? '<1e-4' : r.p.toFixed(4)} | ${r.rounds ?? ''} | ${cpu} |`,
  );
}
const count = (v) => rows.filter((r) => r.verdict === v).length;
lines.push('', `${count('REGRESSION')} regression(s), ${count('improvement')} improvement(s), ${count('unclear')} unclear, ${count('same')} unchanged, ${count('FAILED')} failed.`);
const md = lines.join('\n') + '\n';
process.stdout.write(md);
if (opt('--md')) fs.writeFileSync(opt('--md'), md);
process.exit(!argv.includes('--no-fail') && (count('REGRESSION') || count('FAILED')) ? 1 : 0);
