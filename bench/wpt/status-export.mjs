// WPT results for the status pages of the docs site (website/, /status/wpt.html).
//
// The data lives on the orphan branch `wpt-results`:
//   runs/<sha>.json.gz       one main build: every page pyro ran, per kind (status, passed and total subtests, the
//                            failing subtest names of a testharness page when the baseline names them)
//   index.json               the runs, oldest first, with totals per kind and per top-level directory
//   manifest/<rev>.json.gz   every test URL of WPT's manifest at that revision, per kind: the denominator, so a page
//                            pyro does not run counts as "not run" and percentages never cover only the run subset
//   chrome/<rev>.json.gz     Chrome stable's results on wpt.fyi nearest to that revision, per URL
//
//   bun status-export.mjs export --data DIR --sha SHA --date ISO [--baselines bench/wpt]
//       a run from the committed baselines (on main they are what the gates passed with) + index.json
//   bun status-export.mjs backfill --data DIR [--repo .] [--ref origin/main]
//       one run per first-parent commit of REF that changed a baseline
//   bun status-export.mjs chrome --data DIR --rev REV [--run WPTFYI_RUN_ID]   (default: Chrome stable nearest to REV)
//   export --chrome also writes chrome/<rev>.json.gz when the run's revision has none
//   bun status-export.mjs site --data DIR --docs website/docs
//       the page (docs/status/wpt.md) and every directory's data (docs/public/status-data/wpt/); after the build,
//       copyPages writes one HTML file per directory (website/tools/wpt-status.mjs)
//
// A run is made from the baselines, not from the shard results: a push to main passes its gates only when the
// baselines record what the run found (an unrecorded gain fails as stale), so they are that build's results, and
// history can be rebuilt from git for every commit before this existed.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const KINDS = ['testharness', 'reftest', 'crashtest'];
const BASELINE_FILES = { testharness: 'baseline.json', reftest: 'reftest-baseline.json', crashtest: 'crashtest-baseline.json' };
const WPTFYI = 'https://wpt.fyi/api';

// ---------- files ----------
export const readMaybeGz = (file) => {
  const buf = fs.readFileSync(file);
  return JSON.parse((buf[0] === 0x1f && buf[1] === 0x8b ? zlib.gunzipSync(buf) : buf).toString('utf8'));
};
const writeGz = (file, obj) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, zlib.gzipSync(JSON.stringify(obj), { level: 9 }));
};
const writeJson = (file, obj) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(obj));
};

// ---------- manifest ----------
// WPT's MANIFEST.json (version 9): items[kind] is a tree of path segments whose leaves are [hash, [url|null, extras], ...];
// a null url is the file's own path.
export function manifestUrls(manifest) {
  const out = {};
  for (const kind of KINDS) {
    const urls = [];
    const walk = (node, prefix) => {
      for (const [k, v] of Object.entries(node)) {
        const p = prefix ? `${prefix}/${k}` : k;
        if (Array.isArray(v)) for (const e of v.slice(1)) urls.push(e[0] ? e[0].replace(/^\//, '') : p);
        else walk(v, p);
      }
    };
    walk(manifest.items?.[kind] ?? {}, '');
    out[kind] = urls.sort();
  }
  return out;
}

async function fetchJson(url) {
  for (let i = 0; ; i++) {
    const r = await fetch(url);
    if (r.ok) return r.json();
    if (i >= 2 || r.status < 500) throw new Error(`${url}: HTTP ${r.status}`);
    await new Promise((res) => setTimeout(res, 2000 * (i + 1)));
  }
}

export async function ensureManifest(data, rev) {
  const file = path.join(data, 'manifest', `${rev}.json.gz`);
  if (fs.existsSync(file)) return readMaybeGz(file);
  const m = await fetchJson(`${WPTFYI}/manifest?sha=${rev}`);
  const u = { revision: rev, kinds: manifestUrls(m) };
  writeGz(file, u);
  return u;
}

// ---------- runs ----------
// [status, passed, total, failing names?]. A testharness page's failing names are known when its baseline entry
// names its failures (`fail`); an entry that names its passes leaves them unknown.
export function pageOf(kind, e) {
  const passed = e.pass ? e.pass.length : e.n - e.fail.length;
  const row = [e.status, passed, e.n];
  if (kind === 'testharness' && e.fail?.length) row.push(e.fail);
  return row;
}

export function runFromBaselines(baselines, meta) {
  const run = { v: 1, sha: meta.sha, date: meta.date, source: meta.source ?? 'ci', wpt: null, kinds: {} };
  for (const kind of KINDS) {
    const b = baselines[kind];
    if (!b) continue;
    run.wpt ??= b.wpt?.revision ?? null;
    const tests = {};
    for (const url of Object.keys(b.tests).sort()) tests[url] = pageOf(kind, b.tests[url]);
    run.kinds[kind] = tests;
  }
  return run;
}

// A page passes when it completed with every subtest passing (a reftest or crashtest has one).
export const passes = (row) => !!row && row[0] === 'complete' && row[1] === row[2];

// Chrome's summary_v2 status letters: O(k) and P(ass) are a page that ran to the end.
export const chromePasses = (c) => !!c && (c[2] === 'O' || c[2] === 'P') && c[0] === c[1];

export const splitUrl = (url) => {
  const base = url.split(/[?#]/)[0];
  const i = base.lastIndexOf('/');
  return i < 0 ? ['', url] : [base.slice(0, i), url.slice(i + 1)];
};

// counts: [tests in WPT, run, passed, subtests passed, subtests, Chrome passed, Chrome ran, Chrome passed of the
// pages pyro ran, Chrome subtests passed, Chrome subtests]
const emptyCounts = () => [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
function add(c, row, inUniverse, chrome) {
  if (inUniverse) c[0]++;
  if (row) {
    if (!inUniverse) c[0]++;
    c[1]++;
    if (passes(row)) c[2]++;
    c[3] += row[1];
    c[4] += row[2];
  }
  if (chrome) {
    c[6]++;
    if (chromePasses(chrome)) {
      c[5]++;
      if (row) c[7]++;
    }
    c[8] += chrome[0];
    c[9] += chrome[1];
  }
}

// Every directory's counts per kind: Map dir -> {kind: counts}. '' is the root.
export function rollup(run, universe, chrome) {
  const dirs = new Map();
  const at = (d, kind) => {
    let m = dirs.get(d);
    if (!m) dirs.set(d, (m = {}));
    return (m[kind] ??= emptyCounts());
  };
  for (const kind of KINDS) {
    const rows = run.kinds[kind] ?? {};
    const all = new Set([...(universe?.kinds?.[kind] ?? []), ...Object.keys(rows)]);
    const inU = new Set(universe?.kinds?.[kind] ?? []);
    for (const url of all) {
      const row = rows[url];
      const ch = chrome?.tests?.[url];
      const [dir] = splitUrl(url);
      const parts = dir ? dir.split('/') : [];
      for (let i = 0; i <= parts.length; i++) add(at(parts.slice(0, i).join('/'), kind), row, inU.has(url), ch);
    }
  }
  return dirs;
}

export function indexEntry(run, universe, meta = {}) {
  const r = rollup(run, universe, null);
  const totals = r.get('') ?? {};
  const dirs = {};
  // Top-level directories pyro runs a page of (the others are all "not run": the manifest gives their sizes).
  for (const [d, m] of r) {
    if (!d || d.includes('/')) continue;
    const ran = Object.entries(m).filter(([, c]) => c[1] > 0);
    if (ran.length) dirs[d] = Object.fromEntries(ran.map(([k, c]) => [k, c.slice(0, 5)]));
  }
  return { sha: run.sha, date: run.date, source: run.source, wpt: run.wpt, ...meta, totals: Object.fromEntries(Object.entries(totals).map(([k, c]) => [k, c.slice(0, 5)])), dirs };
}

export function updateIndex(index, entry) {
  const runs = (index?.runs ?? []).filter((r) => r.sha !== entry.sha);
  runs.push(entry);
  runs.sort((a, b) => a.date.localeCompare(b.date) || a.sha.localeCompare(b.sha));
  return { v: 1, runs };
}

export async function writeRun(data, run, meta) {
  const universe = run.wpt ? await ensureManifest(data, run.wpt) : null;
  writeGz(path.join(data, 'runs', `${run.sha}.json.gz`), run);
  const indexFile = path.join(data, 'index.json');
  const index = fs.existsSync(indexFile) ? JSON.parse(fs.readFileSync(indexFile, 'utf8')) : null;
  fs.writeFileSync(indexFile, JSON.stringify(updateIndex(index, indexEntry(run, universe, meta))) + '\n');
}

// ---------- chrome ----------
// Chrome stable on wpt.fyi at the revision, else the stable run nearest to it in WPT's history (as chrome-compare.mjs
// picks it: candidates within a week, by GitHub's compare distance; GH_TOKEN or GITHUB_TOKEN raises its rate limit).
export async function nearestChromeRun(rev) {
  const runsAt = (q) => fetchJson(`${WPTFYI}/runs?product=chrome&label=master&label=stable&max-count=30&${q}`).catch((e) => (/HTTP 404/.test(e.message) ? [] : Promise.reject(e)));
  const [exact] = await runsAt(`sha=${rev.slice(0, 10)}`);
  if (exact) return { run: exact, distance: 0 };
  const GH = 'https://api.github.com/repos/web-platform-tests/wpt';
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
  const gh = async (u) => {
    const r = await fetch(`${GH}/${u}`, { headers: token ? { authorization: `Bearer ${token}` } : {} });
    if (!r.ok) throw new Error(`${GH}/${u}: HTTP ${r.status}`);
    return r.json();
  };
  const t = new Date((await gh(`commits/${rev}`)).commit.committer.date).getTime();
  const iso = (ms) => new Date(ms).toISOString();
  let best = null;
  for (const run of await runsAt(`from=${iso(t - 7 * 864e5)}&to=${iso(t + 7 * 864e5)}`)) {
    const c = await gh(`compare/${rev}...${run.full_revision_hash}`);
    const distance = c.ahead_by + c.behind_by;
    if (!best || distance < best.distance) best = { run, distance };
  }
  if (!best) throw new Error(`no Chrome stable run on wpt.fyi within a week of ${rev}`);
  return best;
}

export async function chromeResults(data, rev, runId) {
  const run = runId ? await fetchJson(`${WPTFYI}/runs/${runId}`) : (await nearestChromeRun(rev)).run;
  const summary = await fetchJson(run.results_url);
  const universe = await ensureManifest(data, rev);
  const tests = {};
  for (const kind of KINDS) for (const url of universe.kinds[kind]) {
    const s = summary['/' + url];
    if (s) tests[url] = [s.c[0], s.c[1], s.s];
  }
  const out = { revision: rev, run: { id: run.id, version: run.browser_version, revision: run.full_revision_hash }, tests };
  writeGz(path.join(data, 'chrome', `${rev}.json.gz`), out);
  return out;
}

// ---------- backfill ----------
export function backfill(data, repo, ref) {
  const git = (...a) => execFileSync('git', ['-C', repo, ...a], { encoding: 'utf8', maxBuffer: 1 << 30, stdio: ['ignore', 'pipe', 'ignore'] });
  const files = Object.values(BASELINE_FILES).map((f) => `bench/wpt/${f}`);
  const commits = git('log', '--first-parent', '--format=%H %cI %s', ref, '--', ...files).trim().split('\n').filter(Boolean).reverse();
  return commits.map((line) => {
    const [sha, date, ...subject] = line.split(' ');
    const baselines = {};
    for (const [kind, f] of Object.entries(BASELINE_FILES)) {
      try { baselines[kind] = JSON.parse(git('show', `${sha}:bench/wpt/${f}`)); } catch { /* not there yet */ }
    }
    return { run: runFromBaselines(baselines, { sha, date, source: 'backfill' }), meta: { subject: subject.join(' ') } };
  });
}

// ---------- site ----------
// One page per directory that holds a test of any kind, and a JSON file it loads.
export function siteData(runs, universes, chrome) {
  const latest = runs[runs.length - 1];
  const universe = universes[latest.wpt];
  const ch = chrome[latest.wpt] ?? null;
  const roll = rollup(latest, universe, ch);
  // History: per run, per directory, [passed, tests in WPT, subtests passed, subtests] per kind.
  const hist = new Map();
  for (const run of runs) {
    const r = run === latest ? roll : rollup(run, universes[run.wpt], null);
    for (const [d, m] of r) {
      let h = hist.get(d);
      if (!h) hist.set(d, (h = []));
      h.push([run.date, run.sha.slice(0, 10), Object.fromEntries(Object.entries(m).map(([k, c]) => [k, [c[2], c[0], c[3], c[4]]]))]);
    }
  }
  // A directory's history keeps the runs that changed it (and its last run), so its file grows with changes only.
  for (const [d, h] of hist) {
    const same = (a, b) => JSON.stringify(a[2]) === JSON.stringify(b[2]);
    hist.set(d, h.filter((p, i) => i === 0 || i === h.length - 1 || !same(p, h[i - 1])));
  }
  // Tests directly in each directory.
  const tests = new Map();
  for (const kind of KINDS) {
    const rows = latest.kinds[kind] ?? {};
    const all = new Set([...(universe?.kinds?.[kind] ?? []), ...Object.keys(rows)]);
    for (const url of all) {
      const [dir, name] = splitUrl(url);
      let list = tests.get(dir);
      if (!list) tests.set(dir, (list = []));
      const row = rows[url];
      const c = ch?.tests?.[url];
      const t = { n: name, k: kind, s: row ? row[0] : null };
      if (row) { t.p = row[1]; t.t = row[2]; if (row[3]) t.f = row[3]; }
      if (c) t.c = chromePasses(c) ? 1 : 0;
      list.push(t);
    }
  }
  const kids = new Map();
  for (const [d, m] of roll) {
    if (!d) continue;
    const i = d.lastIndexOf('/');
    const parent = i < 0 ? '' : d.slice(0, i);
    if (!kids.has(parent)) kids.set(parent, []);
    kids.get(parent).push({ name: d.slice(i + 1), counts: m });
  }
  const pages = new Map();
  for (const [d, m] of roll) {
    const children = (kids.get(d) ?? []).sort((a, b) => a.name.localeCompare(b.name));
    pages.set(d, {
      path: d, sha: latest.sha, date: latest.date, wpt: latest.wpt,
      chrome: ch ? ch.run : null,
      counts: m, dirs: children,
      tests: (tests.get(d) ?? []).sort((a, b) => a.n.localeCompare(b.n) || a.k.localeCompare(b.k)),
      history: hist.get(d) ?? [],
    });
  }
  return pages;
}

const pageFile = (d) => (d ? `wpt/${d}` : 'wpt');

// docs/status/wpt.md is the one VitePress page; every directory's page (/status/wpt/<dir>.html) is a copy of its
// HTML made after the build (copyPages), which WptStatus.vue fills from the address. 2,500 VitePress pages would
// each carry the site's page map, about 180 KB, and 500 MB in all.
export function writeSite(dataDir, docs) {
  const statusDir = path.join(docs, 'status');
  const publicDir = path.join(docs, 'public', 'status-data');
  fs.rmSync(statusDir, { recursive: true, force: true });
  fs.rmSync(publicDir, { recursive: true, force: true });
  fs.mkdirSync(statusDir, { recursive: true });
  const front = '---\ntitle: WPT status\nsidebar: false\naside: false\noutline: false\nsearch: false\npageClass: wpt-status-page\n---\n\n';
  const indexFile = dataDir && path.join(dataDir, 'index.json');
  if (!indexFile || !fs.existsSync(indexFile)) {
    fs.writeFileSync(path.join(statusDir, 'wpt.md'), `${front}# WPT status\n\nNo WPT results have been published yet.\n`);
    return 0;
  }
  const index = JSON.parse(fs.readFileSync(indexFile, 'utf8'));
  const runs = index.runs.map((r) => readMaybeGz(path.join(dataDir, 'runs', `${r.sha}.json.gz`)));
  const universes = {};
  const chrome = {};
  for (const rev of new Set(runs.map((r) => r.wpt))) {
    const m = path.join(dataDir, 'manifest', `${rev}.json.gz`);
    if (fs.existsSync(m)) universes[rev] = readMaybeGz(m);
    const c = path.join(dataDir, 'chrome', `${rev}.json.gz`);
    if (fs.existsSync(c)) chrome[rev] = readMaybeGz(c);
  }
  const pages = siteData(runs, universes, chrome);
  for (const [d, p] of pages) writeJson(path.join(publicDir, `${pageFile(d)}.json`), p);
  writeJson(path.join(publicDir, 'dirs.json'), [...pages.keys()].filter(Boolean).sort());
  fs.writeFileSync(path.join(statusDir, 'wpt.md'), `${front}<WptStatus />\n`);
  return pages.size;
}

// After the build: dist/status/wpt/<dir>.html for every directory, each a copy of dist/status/wpt.html. So that the
// router hydrates the copy as the page it was rendered from (no hydration mismatch), each copy adds itself to
// VitePress's page map (__VP_HASH_MAP__, inlined in the HTML) under its own name, pointing at a module that
// re-exports status/wpt.md's with the copy's own relativePath. A directory reached by a link instead has no entry:
// the theme shows WptStatus where "not found" would be (theme/index.ts), rendered on the client, where nothing needs
// to match.
const INVALID_CHAR = /[\u0000-\u001F"#$&*+,:;<=>?[\]^`{|}\u007F]/g; // vitepress sanitizeFileName (rollup's)
export const pageKey = (d) => `status_wpt_${d.replace(/\//g, '_')}`.replace(INVALID_CHAR, '_') + '.md';

export function copyPages(dist) {
  const dirsFile = path.join(dist, 'status-data', 'dirs.json');
  if (!fs.existsSync(dirsFile)) return 0;
  const html = fs.readFileSync(path.join(dist, 'status', 'wpt.html'), 'utf8');
  const hash = html.match(/\\"status_wpt\.md\\":\\"([\w-]+)\\"/)?.[1];
  const mapEnd = html.indexOf('</script>', html.indexOf('__VP_HASH_MAP__'));
  const dirs = JSON.parse(fs.readFileSync(dirsFile, 'utf8'));
  const keys = new Map();
  for (const d of dirs) keys.set(pageKey(d).toLowerCase(), (keys.get(pageKey(d).toLowerCase()) ?? 0) + 1);
  for (const d of dirs) {
    const f = path.join(dist, 'status', 'wpt', `${d}.html`);
    fs.mkdirSync(path.dirname(f), { recursive: true });
    const key = pageKey(d);
    if (!hash || mapEnd < 0 || keys.get(key.toLowerCase()) > 1) {
      fs.writeFileSync(f, html);
      continue;
    }
    for (const ext of ['js', 'lean.js']) {
      const target = `./status_wpt.md.${hash}.${ext}`;
      // Its own relativePath: the router rewrites the address bar to the page data's path after loading
      // (vitepress client/app/router.js), which would turn every copy's URL into /status/wpt.html.
      const rel = JSON.stringify(`status/wpt/${d}.md`);
      fs.writeFileSync(
        path.join(dist, 'assets', `${key}.${hash}.${ext}`),
        `import * as m from ${JSON.stringify(target)};export default m.default;` +
          `export const __pageData = { ...m.__pageData, relativePath: ${rel}, filePath: ${rel} };\n`,
      );
    }
    const entry = `<script>window.__VP_HASH_MAP__[${JSON.stringify(key.toLowerCase())}]=${JSON.stringify(hash)}</script>`;
    fs.writeFileSync(f, html.slice(0, mapEnd + 9) + entry + html.slice(mapEnd + 9));
  }
  return dirs.length;
}

// ---------- CLI ----------
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [cmd, ...argv] = process.argv.slice(2);
  const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
  const data = opt('--data');
  const HERE = path.dirname(fileURLToPath(import.meta.url));
  if (cmd === 'export') {
    const dir = opt('--baselines', HERE);
    const baselines = {};
    for (const [kind, f] of Object.entries(BASELINE_FILES)) if (fs.existsSync(path.join(dir, f))) baselines[kind] = JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8'));
    const run = runFromBaselines(baselines, { sha: opt('--sha'), date: opt('--date'), source: 'ci' });
    await writeRun(data, run, opt('--subject') ? { subject: opt('--subject') } : {});
    console.log(`runs/${run.sha}.json.gz: ${KINDS.map((k) => `${k} ${Object.keys(run.kinds[k] ?? {}).length}`).join(', ')}`);
    // A new WPT revision: Chrome's results for it. A failure leaves the run without a Chrome column, not unpublished.
    if (argv.includes('--chrome') && run.wpt && !fs.existsSync(path.join(data, 'chrome', `${run.wpt}.json.gz`))) {
      try {
        const c = await chromeResults(data, run.wpt);
        console.log(`chrome/${c.revision}.json.gz: Chrome ${c.run.version} at WPT ${c.run.revision.slice(0, 10)}, ${Object.keys(c.tests).length} pages`);
      } catch (e) {
        console.log(`::warning title=WPT status::no Chrome results for WPT ${run.wpt}: ${e.message}`);
      }
    }
  } else if (cmd === 'backfill') {
    for (const { run, meta } of backfill(data, opt('--repo', '.'), opt('--ref', 'origin/main'))) {
      await writeRun(data, run, meta);
      console.log(run.sha.slice(0, 10), run.date, meta.subject);
    }
  } else if (cmd === 'chrome') {
    const c = await chromeResults(data, opt('--rev'), opt('--run'));  // no --run: the nearest stable run
    console.log(`chrome/${c.revision}.json.gz: Chrome ${c.run.version}, ${Object.keys(c.tests).length} pages`);
  } else if (cmd === 'site') {
    console.log(`${writeSite(data, opt('--docs'))} directory pages`);
  } else {
    console.error('usage: status-export.mjs export|backfill|chrome|site --data DIR ...');
    process.exit(2);
  }
}
