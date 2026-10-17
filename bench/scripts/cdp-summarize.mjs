// Summarize a cdp-record-proxy JSONL file into sorted method/event count tables.
// Usage: node cdp-summarize.mjs <in.jsonl> <out.json> [out.md]
import fs from 'node:fs';

const [inPath, outJson, outMd] = process.argv.slice(2);
if (!inPath || !outJson) { console.error('usage: node cdp-summarize.mjs <in.jsonl> <out.json> [out.md]'); process.exit(2); }
const recs = fs.readFileSync(inPath, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));

const commands = new Map(); // method -> {count, withSession, withoutSession, ok, error, errors:Set}
const events = new Map();   // method -> {count, withSession, withoutSession}
const get = (m, k, init) => { if (!m.has(k)) m.set(k, init()); return m.get(k); };

for (const r of recs) {
  if (r.dir === 'c2s' && r.kind === 'command') {
    const c = get(commands, r.method, () => ({ count: 0, withSession: 0, withoutSession: 0, ok: 0, error: 0, errors: new Set() }));
    c.count++; r.session ? c.withSession++ : c.withoutSession++;
  } else if (r.dir === 's2c' && (r.kind === 'response' || r.kind === 'error')) {
    const c = get(commands, r.method ?? '(unknown)', () => ({ count: 0, withSession: 0, withoutSession: 0, ok: 0, error: 0, errors: new Set() }));
    if (r.kind === 'error') { c.error++; c.errors.add(`${r.error?.code}: ${r.error?.message}`); } else c.ok++;
  } else if (r.dir === 's2c' && r.kind === 'event') {
    const e = get(events, r.method, () => ({ count: 0, withSession: 0, withoutSession: 0 }));
    e.count++; r.session ? e.withSession++ : e.withoutSession++;
  }
}
const sortRows = (m) => [...m.entries()].sort((a, b) => b[1].count - a[1].count || a[0].localeCompare(b[0]));
const cmdRows = sortRows(commands).map(([method, c]) => ({ method, ...c, errors: [...c.errors] }));
const evRows = sortRows(events).map(([method, e]) => ({ method, ...e }));
const conns = recs.filter((r) => r.kind === 'ws-open').length;
const closes = recs.filter((r) => r.kind === 'ws-close');
const http = recs.filter((r) => r.kind === 'http').map((r) => `${r.method} ${r.path} -> ${r.status}`);
const summary = {
  source: inPath, records: recs.length, wsConnections: conns, http, closes,
  totals: { commands: cmdRows.reduce((a, r) => a + r.count, 0), errors: cmdRows.reduce((a, r) => a + r.error, 0), events: evRows.reduce((a, r) => a + r.count, 0) },
  commands: cmdRows, events: evRows,
};
fs.writeFileSync(outJson, JSON.stringify(summary, null, 2));

let md = `# CDP inventory (${inPath})\n\nrecords=${recs.length} ws_connections=${conns} commands=${summary.totals.commands} error_responses=${summary.totals.errors} events=${summary.totals.events}\n\n`;
md += `HTTP: ${http.join('; ') || '(none)'}\n\n## Client -> server commands\n\n| method | count | w/ sessionId | w/o sessionId | ok | error | error messages |\n|---|---:|---:|---:|---:|---:|---|\n`;
for (const r of cmdRows) md += `| ${r.method} | ${r.count} | ${r.withSession} | ${r.withoutSession} | ${r.ok} | ${r.error} | ${r.errors.join('<br>').replace(/\|/g, '\\|')} |\n`;
md += `\n## Server -> client events\n\n| event | count | w/ sessionId | w/o sessionId |\n|---|---:|---:|---:|\n`;
for (const r of evRows) md += `| ${r.method} | ${r.count} | ${r.withSession} | ${r.withoutSession} |\n`;
if (outMd) fs.writeFileSync(outMd, md);
console.log(md);
