// bun aggregate.mjs out/lp.json out/upstream.json out/chrome.json > wpt.md
import fs from 'node:fs';
const runs = process.argv.slice(2).map((f) => JSON.parse(fs.readFileSync(f)));
const labels = runs.map((r) => r.label);
// Denominator per test page: the largest subtest count any browser reported (a harness crash reports 0-1).
const maxSubs = new Map();
for (const r of runs) for (const t of r.results) maxSubs.set(t.url, Math.max(maxSubs.get(t.url) ?? 1, t.subtests.length));
const dirs = [...new Set(runs[0].results.map((t) => t.dir))];
const stat = {};
for (const r of runs) {
  for (const t of r.results) {
    const s = ((stat[r.label] ??= {})[t.dir] ??= { pass: 0, reported: 0, total: 0, files: 0, filesAllPass: 0, timeouts: 0, errors: 0 });
    const pass = t.subtests.filter((x) => x.status === 'Pass').length;
    s.pass += pass; s.reported += t.subtests.length; s.total += maxSubs.get(t.url); s.files++;
    if (pass > 0 && pass === maxSubs.get(t.url)) s.filesAllPass++;
    if (t.status === 'timeout') s.timeouts++;
    if (t.status === 'error') s.errors++;
  }
}
const pct = (a, b) => (b ? ((100 * a) / b).toFixed(1) + '%' : '-');
const sum = (label, filter) => {
  const o = { pass: 0, total: 0, files: 0, filesAllPass: 0, timeouts: 0, errors: 0 };
  for (const d of dirs) if (filter(d)) for (const k of Object.keys(o)) o[k] += stat[label][d][k];
  return o;
};
let md = `| directory | pages | subtests (max reported) | ${labels.map((l) => `${l} pass`).join(' | ')} |\n|---|---:|---:|${labels.map(() => '---:').join('|')}|\n`;
for (const d of dirs) {
  const s0 = stat[labels[0]][d];
  md += `| ${d} | ${s0.files} | ${s0.total} | ${labels.map((l) => `${stat[l][d].pass} (${pct(stat[l][d].pass, stat[l][d].total)})`).join(' | ')} |\n`;
}
for (const [name, f] of [['**all**', () => true], ['**all except encoding/**', (d) => d !== 'encoding']]) {
  const s0 = sum(labels[0], f);
  md += `| ${name} | ${s0.files} | ${s0.total} | ${labels.map((l) => { const s = sum(l, f); return `**${s.pass} (${pct(s.pass, s.total)})**`; }).join(' | ')} |\n`;
}
md += `\nPages fully passing / page timeouts / page errors (crash, navigation failure):\n\n| browser | all-pass pages | timeouts | errors | wall time | browser restarts |\n|---|---:|---:|---:|---:|---:|\n`;
for (const r of runs) { const s = sum(r.label, () => true); md += `| ${r.label} | ${s.filesAllPass}/${s.files} | ${s.timeouts} | ${s.errors} | ${(r.wallMs / 1000).toFixed(0)} s | ${r.restarts} |\n`; }
md += `\nExcluded from discovery for every browser: ${JSON.stringify(runs[0].excluded)}\n`;
console.log(md);
