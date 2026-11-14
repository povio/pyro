// pyro's reftest results against Chrome's (and Firefox's and Safari's) on wpt.fyi, for the same reftests.
//
//   bun bench/wpt/chrome-compare.mjs [SHARD.json ...] [--baseline FILE] [--run ID] [--canary] [--md FILE] [--json FILE]
//
// pyro's side: the given reftest results (gate.mjs/run.mjs --kind reftest output, every shard of one run; a page
// passes if every run of it passed), else bench/wpt/reftest-baseline.json. Its WPT revision picks the wpt.fyi runs:
// Chrome stable at that revision when wpt.fyi has one, otherwise the Chrome stable run nearest to it in WPT's commit
// history (GitHub's compare API; GH_TOKEN or GITHUB_TOKEN raises its rate limit); Firefox and Safari stable at the
// revision Chrome's run used. --canary takes Chrome's experimental (canary/dev) run instead, which wpt.fyi usually
// has at the exact revision; --run ID takes that wpt.fyi run as Chrome's. Prints, per directory, the reftests each
// browser passes, then the reftests Chrome passes and pyro fails, grouped by feature (the paint work queue).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { consensus, passCount, readJson } from './lib.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const VALUED = ['--baseline', '--run', '--md', '--json'];
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const files = argv.filter((a, i) => !a.startsWith('--') && !VALUED.includes(argv[i - 1]));
const WPTFYI = 'https://wpt.fyi/api';
const GH = 'https://api.github.com/repos/web-platform-tests/wpt';
const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;

async function get(url, json = true) {
  const headers = url.startsWith(GH) && token ? { authorization: `Bearer ${token}` } : {};
  for (let i = 0; ; i++) {
    const r = await fetch(url, { headers });
    if (r.ok) return json ? r.json() : r.text();
    if (i >= 2 || r.status < 500) throw new Error(`${url}: HTTP ${r.status}`);
    await new Promise((res) => setTimeout(res, 1000 * (i + 1)));
  }
}

// ---------- pyro ----------
// url -> { dir, pass }
function pyroResults() {
  const out = new Map();
  if (files.length) {
    const runs = files.map(readJson);
    if (runs.some((r) => r.kind !== 'reftest')) throw new Error('give reftest results (run.mjs --kind reftest)');
    const pages = new Map();
    for (const r of runs) for (const t of r.results) pages.set(t.url, [t]);
    for (const r of runs) for (const t of r.reruns ?? []) pages.get(t.url)?.push(t);
    for (const [url, list] of pages) out.set(url, { dir: list[0].dir, pass: consensus(list).pass.length > 0, status: list[list.length - 1].status });
    return { pages: out, revision: runs[0].wptRevision, dirs: runs[0].dirs, source: `${files.length} result file(s)` };
  }
  const file = opt('--baseline', path.join(HERE, 'reftest-baseline.json'));
  const b = readJson(file);
  for (const [url, e] of Object.entries(b.tests)) out.set(url, { dir: dirOf(url, b.dirs), pass: passCount(e) > 0, status: e.status });
  return { pages: out, revision: b.wpt.revision, dirs: b.dirs, source: path.relative(process.cwd(), file) };
}
const dirOf = (url, dirs) => dirs.filter((d) => url.startsWith(d + '/')).sort((a, b) => b.length - a.length)[0] ?? url.split('/').slice(0, 2).join('/');

// ---------- wpt.fyi ----------
// wpt.fyi answers 404 when no run matches.
const runsAt = (product, sha, label) => get(`${WPTFYI}/runs?product=${product}&label=master&label=${label}&sha=${sha}&max-count=1`).catch((e) => (/HTTP 404/.test(e.message) ? [] : Promise.reject(e)));
async function chromeRun(revision) {
  if (opt('--run')) return { run: await get(`${WPTFYI}/runs/${opt('--run')}`), how: `wpt.fyi run ${opt('--run')} (--run)` };
  if (argv.includes('--canary')) {
    const [run] = await runsAt('chrome', revision.slice(0, 10), 'experimental');
    if (!run) throw new Error(`wpt.fyi has no experimental Chrome run at ${revision.slice(0, 10)}`);
    return { run, how: 'Chrome experimental (canary) at the pinned revision' };
  }
  const [exact] = await runsAt('chrome', revision.slice(0, 10), 'stable');
  if (exact) return { run: exact, how: 'Chrome stable at the pinned revision' };
  // The nearest stable run in commit history: candidates within a week of the pinned commit, by distance.
  const commit = await get(`${GH}/commits/${revision}`);
  const t = new Date(commit.commit.committer.date).getTime();
  const iso = (ms) => new Date(ms).toISOString();
  const candidates = await get(`${WPTFYI}/runs?product=chrome&label=master&label=stable&from=${iso(t - 7 * 864e5)}&to=${iso(t + 7 * 864e5)}&max-count=30`);
  let best = null;
  for (const run of candidates) {
    const c = await get(`${GH}/compare/${revision}...${run.full_revision_hash}`);
    const distance = c.ahead_by + c.behind_by;
    if (!best || distance < best.distance) best = { run, distance, c };
  }
  if (!best) throw new Error('no Chrome stable run on wpt.fyi within a week of the pinned revision');
  const { ahead_by: ahead, behind_by: behind } = best.c;
  return { run: best.run, how: `the Chrome stable run nearest to the pinned revision: ${ahead ? `${ahead} commit(s) after it` : `${behind} commit(s) before it`}` };
}
async function summary(run) {
  return run ? get(run.results_url) : null;
}

// ---------- features ----------
// What a reftest is about, from its path: the first rule that matches, else its subdirectory or name stem.
const FEATURES = [
  [/box-shadow|inset-box-shadow/, 'box-shadow'],
  [/text-shadow/, 'text-shadow'],
  [/drop-shadow/, 'filter: drop-shadow()'],
  [/backdrop-filter/, 'backdrop-filter'],
  [/filter-effects\/.*(blur)/, 'filter: blur()'],
  [/filter-effects\/.*(fe[A-Z]\w+|feimage|feflood|tainting|svg-|effect-reference|filter-region|filter-in-|url)/i, 'SVG filters (url(), fe*)'],
  [/filter-effects\/.*(grayscale|sepia|saturate|hue-rotate|invert|brightness|contrast|opacity|filter-function|filters-test)/, 'filter functions'],
  [/filter-effects\//, 'filter (other)'],
  [/mix-blend-mode/, 'mix-blend-mode'],
  [/background-blend|background-blending/, 'background-blend-mode'],
  [/isolation/, 'isolation'],
  [/compositing\//, 'compositing (other)'],
  [/clip-path/, 'clip-path'],
  [/mask/, 'mask'],
  [/css-masking\/clip\/|visufx\/clip/, 'clip'],
  [/css-masking\//, 'masking (other)'],
  [/border-image/, 'border-image'],
  [/border-radius|rounded/, 'border-radius'],
  [/outline/, 'outline'],
  [/border/, 'borders'],
  [/background-size/, 'background-size'],
  [/background-clip|background-origin|clip-border-area/, 'background-clip, background-origin'],
  [/background-attachment|local-attachment/, 'background-attachment'],
  [/background-position|background-repeat|subpixel-repeat/, 'background-position, background-repeat'],
  [/gradient/, 'gradients'],
  [/background/, 'backgrounds (other)'],
  [/opacity/, 'opacity'],
  [/z-?index|stack|zorder|stacking-context/, 'stacking order (z-index)'],
  [/overflow|visibility/, 'overflow, visibility'],
  [/text-decoration|line-through|text-underline|decorating-box/, 'text-decoration'],
  [/text-emphasis|ruby/, 'text-emphasis'],
  [/css-text-decor\//, 'text decoration (other)'],
  [/preserve-?3d|perspective|backface|transform3d|rotate[XYZ3]|translate[Z3]|scale[Z3]|matrix3d|-3d/i, 'transforms: 3D'],
  [/transform-origin|transform-box/, 'transform-origin, transform-box'],
  [/css-transforms\/(individual-transform|translate|rotate|scale)/, 'transforms: individual properties'],
  [/css-transforms\//, 'transforms: 2D'],
  [/color-mix/, 'color-mix()'],
  [/relative|currentcolor/, 'colors: relative, currentcolor'],
  [/lab|lch|oklab|oklch|xyz|display-p3|rec2020|prophoto|a98|srgb-linear|predefined|color\(/, 'colors: color spaces'],
  [/hsl|hwb|rgb|t4\d\d/, 'colors: rgb(), hsl(), hwb()'],
  [/css-color\/|CSS2\/colors/, 'colors (other)'],
];
const featureOf = (url) => FEATURES.find(([re]) => re.test(url))?.[1] ?? url.split('/').slice(-2, -1)[0];

// ---------- report ----------
const pct = (n, d) => (d ? `${((100 * n) / d).toFixed(1)}%` : '–');
async function main() {
  const pyro = pyroResults();
  if (!pyro.revision) throw new Error('the results name no WPT revision');
  const { run: chrome, how } = await chromeRun(pyro.revision);
  const sha = chrome.full_revision_hash;
  const [firefoxRun] = await runsAt('firefox', sha.slice(0, 10), 'stable');
  const [safariRun] = await runsAt('safari', sha.slice(0, 10), 'stable');
  const [c, f, s] = await Promise.all([summary(chrome), summary(firefoxRun), summary(safariRun)]);
  const passes = (sum, url) => (sum ? sum[`/${url}`]?.s === 'P' : null);
  const dirs = [...new Set([...pyro.pages.values()].map((p) => p.dir))].sort((a, b) => (pyro.dirs?.indexOf(a) ?? 0) - (pyro.dirs?.indexOf(b) ?? 0));
  const rows = [];
  const queue = [];
  const total = { n: 0, pyro: 0, chrome: 0, firefox: 0, safari: 0, gap: 0, ahead: 0, missing: 0 };
  for (const d of dirs) {
    const r = { dir: d, n: 0, pyro: 0, chrome: 0, firefox: 0, safari: 0, gap: 0, ahead: 0, missing: 0 };
    for (const [url, p] of pyro.pages) {
      if (p.dir !== d) continue;
      const cp = passes(c, url);
      r.n++;
      if (c?.[`/${url}`] == null) r.missing++;
      if (p.pass) r.pyro++;
      if (cp) r.chrome++;
      if (passes(f, url)) r.firefox++;
      if (passes(s, url)) r.safari++;
      if (cp && !p.pass) { r.gap++; queue.push({ url, feature: featureOf(url), status: p.status }); }
      if (p.pass && c?.[`/${url}`] && !cp) r.ahead++;
    }
    rows.push(r);
    for (const k of Object.keys(total)) total[k] += r[k];
  }
  const label = (run) => (run ? `${run.browser_name} ${run.browser_version}` : 'none');
  let md = `### pyro vs Chrome on the paint reftests\n\n`;
  md += `pyro: ${pyro.source} (WPT ${pyro.revision.slice(0, 10)}). Chrome: ${label(chrome)}, ${how} (WPT ${sha.slice(0, 10)}, [wpt.fyi run ${chrome.id}](https://wpt.fyi/results/?run_id=${chrome.id})); Firefox: ${label(firefoxRun)}; Safari: ${label(safariRun)} (both at Chrome's revision). A reftest passes in Chrome if wpt.fyi records PASS; one wpt.fyi does not list counts as failing.\n\n`;
  md += `| directory | reftests | pyro | Chrome | Firefox | Safari | Chrome passes, pyro fails | pyro passes, Chrome fails |\n|---|---:|---:|---:|---:|---:|---:|---:|\n`;
  const line = (r, name) => `| ${name} | ${r.n} | ${r.pyro} (${pct(r.pyro, r.n)}) | ${r.chrome} (${pct(r.chrome, r.n)}) | ${f ? `${r.firefox} (${pct(r.firefox, r.n)})` : '–'} | ${s ? `${r.safari} (${pct(r.safari, r.n)})` : '–'} | ${r.gap} | ${r.ahead} |\n`;
  for (const r of rows) md += line(r, r.dir);
  md += line(total, '**all**');
  // css/CSS2 is large and mostly layout: its subdirectories too.
  const sub = new Map();
  for (const [url, p] of pyro.pages) {
    if (p.dir !== 'css/CSS2') continue;
    const name = url.split('/').length > 3 ? url.split('/').slice(0, 3).join('/') : 'css/CSS2 (top level)';
    const r = sub.get(name) ?? { n: 0, pyro: 0, chrome: 0, firefox: 0, safari: 0, gap: 0, ahead: 0 };
    const cp = passes(c, url);
    r.n++; if (p.pass) r.pyro++; if (cp) r.chrome++; if (passes(f, url)) r.firefox++; if (passes(s, url)) r.safari++;
    if (cp && !p.pass) r.gap++;
    if (p.pass && c?.[`/${url}`] && !cp) r.ahead++;
    sub.set(name, r);
  }
  if (sub.size) {
    md += `\n<details><summary>css/CSS2 by subdirectory</summary>\n\n| directory | reftests | pyro | Chrome | Firefox | Safari | Chrome passes, pyro fails | pyro passes, Chrome fails |\n|---|---:|---:|---:|---:|---:|---:|---:|\n`;
    for (const [name, r] of [...sub].sort((a, b) => b[1].gap - a[1].gap)) md += line(r, name);
    md += `\n</details>\n`;
  }
  if (total.missing) md += `\n${total.missing} of these reftests have no result in Chrome's run (added after its revision, or renamed).\n`;
  const byFeature = new Map();
  for (const q of queue) byFeature.set(q.feature, [...(byFeature.get(q.feature) ?? []), q]);
  const features = [...byFeature].sort((a, b) => b[1].length - a[1].length);
  md += `\n### Reftests Chrome passes and pyro fails, by feature (${queue.length})\n\n| feature | reftests | examples |\n|---|---:|---|\n`;
  for (const [feat, list] of features) md += `| ${feat} | ${list.length} | ${list.slice(0, 3).map((q) => `\`${q.url.split('/').pop()}\``).join(', ')} |\n`;
  md += `\n<details><summary>Every one of them</summary>\n\n`;
  for (const [feat, list] of features) {
    md += `**${feat}** (${list.length})\n\n`;
    for (const q of list.sort((a, b) => (a.url < b.url ? -1 : 1))) md += `- ${q.url}${q.status !== 'complete' ? ` (${q.status})` : ''}\n`;
    md += '\n';
  }
  md += `</details>\n`;
  console.log(md);
  if (opt('--md')) fs.writeFileSync(opt('--md'), md);
  if (opt('--json')) fs.writeFileSync(opt('--json'), JSON.stringify({ pyro: { source: pyro.source, revision: pyro.revision }, chrome: { run: chrome.id, version: chrome.browser_version, revision: sha, how }, firefox: firefoxRun?.browser_version ?? null, safari: safariRun?.browser_version ?? null, rows, total, queue }, null, 1));
}

main().catch((e) => { console.error(`chrome-compare.mjs: ${e.message}`); process.exitCode = 2; });
