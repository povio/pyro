// Markdown tables from run.mjs + compare.mjs output.
//   bun summary.mjs --out <dir>
import { readFileSync } from 'node:fs';
import path from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const out = path.resolve(args.out);
const rows = JSON.parse(readFileSync(path.join(out, 'runs.json'), 'utf8'));
const read = (f) => {
  try {
    return JSON.parse(readFileSync(path.join(out, f), 'utf8'));
  } catch {
    return null;
  }
};
const tp = read('throughput.json') ?? {};
const meta = read('meta.json') ?? {};
const cmp = Object.fromEntries((read('compare-chrome-fixed-vs-lp-fixed.json') ?? []).map((r) => [r.case, r]));
const ctl = Object.fromEntries((read('compare-chrome-base-vs-chrome-fixed.json') ?? []).map((r) => [r.case, r]));

const med = (xs) => {
  const s = xs.filter((x) => Number.isFinite(x)).sort((a, b) => a - b);
  return s.length ? s[Math.floor((s.length - 1) / 2)] : NaN;
};
const engines = ['chrome-base', 'chrome-fixed', 'lp-fixed'];
const cases = [...new Set(rows.map((r) => r.case))];
const get = (c, e) => rows.filter((r) => r.case === c && r.engine === e && !r.error);
const f0 = (x) => (Number.isFinite(x) ? x.toFixed(0) : '–');
const mb = (x) => (Number.isFinite(x) ? (x / 1048576).toFixed(0) : '–');
const pct = (x) => (x == null ? '–' : (100 * x).toFixed(1));

const lines = [];
lines.push(
  '| case | pages C/L | paginate ms Cb/Cf/L | pdf ms Cb/Cf/L | total ms Cf/L | KB Cf/L | peak RSS MB Cf/L | pixel diff % mean/max | text overlap | LP overflow px |',
);
lines.push('|---|---|---|---|---|---|---|---|---|---|');
const agg = Object.fromEntries(engines.map((e) => [e, { paginate: [], pdf: [], total: [], bytes: [], rss: [] }]));
for (const c of cases) {
  const m = Object.fromEntries(
    engines.map((e) => {
      const rs = get(c, e);
      const v = {
        paginate: med(rs.map((r) => r.paginateMs)),
        pdf: med(rs.map((r) => r.pdfMs)),
        total: med(rs.map((r) => r.totalMs)),
        bytes: rs[0]?.bytes,
        pages: rs.find((r) => r.pages != null)?.pages,
        rss: Math.max(...rs.map((r) => r.peakRss)),
        lp: rs.find((r) => r.lp)?.lp,
        err: rows.find((r) => r.case === c && r.engine === e && r.error)?.error,
      };
      for (const k of ['paginate', 'pdf', 'total', 'bytes', 'rss']) if (Number.isFinite(v[k])) agg[e][k].push(v[k]);
      return [e, v];
    }),
  );
  const lp = m['lp-fixed'].lp;
  const ov = lp?.pages?.length
    ? Math.max(...lp.pages.map((p) => Math.max(p.overflowRight, p.overflowBottom)))
    : NaN;
  const k = cmp[c];
  lines.push(
    `| ${c}${m['lp-fixed'].err ? ' (LP error)' : ''} | ${m['chrome-fixed'].pages ?? '–'}/${m['lp-fixed'].pages ?? '–'} | ${f0(m['chrome-base'].paginate)}/${f0(m['chrome-fixed'].paginate)}/${f0(m['lp-fixed'].paginate)} | ${f0(m['chrome-base'].pdf)}/${f0(m['chrome-fixed'].pdf)}/${f0(m['lp-fixed'].pdf)} | ${f0(m['chrome-fixed'].total)}/${f0(m['lp-fixed'].total)} | ${f0(m['chrome-fixed'].bytes / 1024)}/${f0(m['lp-fixed'].bytes / 1024)} | ${mb(m['chrome-fixed'].rss)}/${mb(m['lp-fixed'].rss)} | ${pct(k?.meanRatio)}/${pct(k?.maxRatio)} | ${k?.textOverlap?.toFixed(3) ?? '–'} | ${f0(ov)} |`,
  );
}
const sum = (xs) => xs.reduce((a, b) => a + b, 0);
lines.push('');
lines.push('| engine | sum of median paginate ms | sum of median pdf ms | median pdf ms | sum total ms | total MB | max peak RSS MB | 6-concurrent docs/s | p50/p95 ms | peak RSS MB (6-conc) | failed |');
lines.push('|---|---|---|---|---|---|---|---|---|---|---|');
for (const e of engines) {
  const a = agg[e];
  const t = tp[e] ?? {};
  lines.push(
    `| ${e} | ${f0(sum(a.paginate))} | ${f0(sum(a.pdf))} | ${f0(med(a.pdf))} | ${f0(sum(a.total))} | ${(sum(a.bytes) / 1048576).toFixed(1)} | ${mb(Math.max(...a.rss))} | ${t.docsPerSec?.toFixed(2) ?? '–'} | ${f0(t.p50Ms)}/${f0(t.p95Ms)} | ${mb(t.peakRss)} | ${t.failed ?? '–'} |`,
  );
}
lines.push('');
if (Object.keys(ctl).length) {
  const r = Object.values(ctl);
  lines.push(
    `Control (chrome-base vs chrome-fixed): mean pixel diff ${pct(sum(r.map((x) => x.meanRatio ?? 0)) / r.length)}%, max ${pct(Math.max(...r.map((x) => x.maxRatio ?? 0)))}%`,
  );
}
lines.push(`Load: ${JSON.stringify(meta)}`);
console.log(lines.join('\n'));
