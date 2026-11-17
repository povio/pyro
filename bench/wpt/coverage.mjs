// What of WPT runs, and why the rest does not: every test WPT's own manifest lists at the pinned revision, by kind
// and area, as run in CI today ("gate"), runnable by the current runner but not yet planned ("runnable"), or not run
// for a reason (discovery's reason, or the kind having no runner). Writes coverage.json (README "Coverage"), which
// the status site reads for its "not run" denominators.
// Usage: bun bench/wpt/coverage.mjs --wpt DIR [--manifest MANIFEST.json] [--out bench/wpt/coverage.json]
//   DIR: a whole checkout at the pinned revision (fetch-wpt.sh --all). --manifest: WPT's MANIFEST.json for that
//   revision; default: DIR/.git/pyro-MANIFEST.json, downloaded from wpt.fyi (the manifest WPT publishes per revision)
//   when missing. `./wpt manifest` in the checkout writes the same file.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { DEFAULT_DIRS, NON_TEST_TOP, REFTEST_DIRS, discover, discoverReftests } from './lib.mjs';
import { discoverCrashtests } from './crashtest.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const WPT = path.resolve(opt('--wpt'));
const OUT = opt('--out', path.join(HERE, 'coverage.json'));
const REV = fs.readFileSync(path.join(HERE, 'wpt-revision.txt'), 'utf8').trim();

async function manifest() {
  const file = opt('--manifest', path.join(WPT, '.git', 'pyro-MANIFEST.json'));
  if (!fs.existsSync(file)) {
    const res = await fetch(`https://wpt.fyi/api/manifest?sha=${REV}`);
    if (!res.ok) throw new Error(`wpt.fyi manifest: HTTP ${res.status}`);
    const sha = res.headers.get('x-wpt-sha');
    if (sha !== REV) throw new Error(`wpt.fyi manifest is for ${sha}, not the pinned ${REV}`);
    let buf = Buffer.from(await res.arrayBuffer());
    if (buf[0] === 0x1f && buf[1] === 0x8b) buf = zlib.gunzipSync(buf);
    fs.writeFileSync(file, buf);
  }
  const m = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (m.version !== 9) throw new Error(`manifest version ${m.version}: coverage.mjs reads version 9`);
  return m;
}

// The area a test is counted under (as crashtest.mjs and the coverage report): css/<spec>, html/<section>,
// svg/<section>, else the top-level directory.
const areaOf = (rel) => {
  const p = rel.split('/');
  return ['css', 'html', 'svg'].includes(p[0]) && p.length > 2 ? `${p[0]}/${p[1]}` : p[0];
};
const under = (rel, dirs) => dirs.some((d) => d === '.' || rel === d || rel.startsWith(d + '/'));

// Kinds no runner of ours takes, with the reason.
const NO_RUNNER = {
  'print-reftest': 'print reftest (no print runner)',
  manual: 'manual test',
  visual: 'visual test (no reference)',
  wdspec: 'wdspec (WebDriver, Python)',
  aamtest: 'accessibility API test',
  test262: 'test262 (JavaScript engine suite)',
};
const WORKER = /\.(worker|sharedworker|serviceworker|sharedworker-module|serviceworker-module|worker-module)\.html/;
const SHADOWREALM = /\.any\.shadowrealm[\w-]*\.html|\.shadowrealm[\w-]*\.html/;

const m = await manifest();
const top = fs.readdirSync(WPT, { withFileTypes: true }).filter((e) => e.isDirectory() && !e.name.startsWith('.') && !NON_TEST_TOP.includes(e.name)).map((e) => e.name).sort();

const th = { reasons: new Map() }, rt = { reasons: new Map() }, ct = { reasons: new Map() };
th.urls = new Set(discover(WPT, top, { reasons: th.reasons }).tests.map((t) => t.url));
rt.urls = new Set(discoverReftests(WPT, top, { reasons: rt.reasons }).tests.map((t) => t.url));
ct.urls = new Set(discoverCrashtests(WPT, ['.'], { reasons: ct.reasons }).tests.map((t) => t.url));
const GATE_DIRS = { testharness: DEFAULT_DIRS, reftest: REFTEST_DIRS, crashtest: ['.'] };
const DISC = { testharness: th, reftest: rt, crashtest: ct };

function* leaves(tree, prefix = '') {
  for (const [k, v] of Object.entries(tree)) {
    if (Array.isArray(v)) yield [prefix + k, v];
    else yield* leaves(v, prefix + k + '/');
  }
}

const blank = () => ({ total: 0, gate: 0, runnable: 0, notRun: {} });
const kinds = {}, areas = {};
const seen = { testharness: new Set(), reftest: new Set(), crashtest: new Set() };
const count = (kind, area, status, reason) => {
  for (const o of [(kinds[kind] ??= blank()), ((areas[area] ??= {})[kind] ??= blank())]) {
    o.total++;
    if (reason) o.notRun[reason] = (o.notRun[reason] ?? 0) + 1; else o[status]++;
  }
};
for (const [kind, tree] of Object.entries(m.items)) {
  if (kind === 'support') continue;
  for (const [file, [, ...items]] of leaves(tree)) {
    for (const it of items) {
      const url = (it[0] ?? '/' + file).replace(/^\//, '');
      const area = areaOf(file);
      if (NO_RUNNER[kind]) { count(kind, area, null, NO_RUNNER[kind]); continue; }
      const d = DISC[kind];
      if (!d) { count(kind, area, null, `${kind} (no runner)`); continue; }
      if (d.urls.has(url)) {
        seen[kind].add(url);
        count(kind, area, under(file, GATE_DIRS[kind]) ? 'gate' : 'runnable');
        continue;
      }
      const meta = it[it.length - 1] ?? {};
      let why = kind === 'testharness' && WORKER.test(url) ? 'worker variant'
        : kind === 'testharness' && SHADOWREALM.test(url) ? 'shadowrealm variant'
        : kind === 'testharness' && meta.jsshell ? 'jsshell variant'
        : d.reasons.get(file) ?? (meta.testdriver ? 'needs testdriver' : 'not discovered (UTF-16 source, or a testharness page under crashtests/)');
      count(kind, area, null, why);
    }
  }
}
// What discovery runs that the manifest does not list as that kind: should be nothing.
const extra = Object.fromEntries(Object.entries(DISC).map(([k, d]) => [k, [...d.urls].filter((u) => !seen[k].has(u)).sort()]));

const sortObj = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : 1)));
const tidy = (o) => ({ ...o, notRun: sortObj(o.notRun) });
const out = {
  wpt: { repo: 'web-platform-tests/wpt', revision: REV },
  about: 'Every test in WPT\'s MANIFEST.json at this revision, by kind (manifest item type) and area. total = gate + runnable + sum(notRun). gate: run by CI today (wpt.yml); runnable: the current runner can run it, not planned yet; notRun: reason -> count. One item per URL (variants count separately). Written by bench/wpt/coverage.mjs.',
  kinds: sortObj(Object.fromEntries(Object.entries(kinds).map(([k, v]) => [k, tidy(v)]))),
  areas: sortObj(Object.fromEntries(Object.entries(areas).map(([a, ks]) => [a, sortObj(Object.fromEntries(Object.entries(ks).map(([k, v]) => [k, tidy(v)])))]))),
  discoveredNotInManifest: Object.fromEntries(Object.entries(extra).map(([k, v]) => [k, { count: v.length, examples: v.slice(0, 10) }])),
};
// Areas one per line, so a change reads as a small diff.
let s = JSON.stringify({ ...out, areas: '@@' }, null, 1);
const areaLines = Object.entries(out.areas).map(([a, v]) => `  ${JSON.stringify(a)}: ${JSON.stringify(v)}`).join(',\n');
s = s.replace('"@@"', `{\n${areaLines}\n }`) + '\n';
fs.writeFileSync(OUT, s);
const sum = (k) => Object.values(out.kinds).reduce((n, v) => n + v[k], 0);
console.log(`${OUT}: ${sum('total')} tests, gate ${sum('gate')}, runnable ${sum('runnable')}`);
for (const [k, v] of Object.entries(out.kinds)) console.log(`  ${k}: ${v.total} (gate ${v.gate}, runnable ${v.runnable}) ${JSON.stringify(v.notRun)}`);
for (const [k, v] of Object.entries(out.discoveredNotInManifest)) if (v.count) console.log(`  discovered as ${k}, not in the manifest as one: ${v.count} (${v.examples.slice(0, 3).join(', ')})`);
