// Shared by run.mjs and gate.mjs: test discovery, the baseline format, shard planning and the coverage comparison.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

// The testharness directories, and the reftest directories (their testharness pages: parsing, computed values,
// interpolation, CSS2 geometry). REFTEST_DIRS lists the same ones, reftests first, and LAYOUT_DIRS, which only the reftest run has.
export const PAINT_DIRS = [
  'css/css-backgrounds', 'css/css-text-decor', 'css/filter-effects', 'css/compositing', 'css/css-masking',
  'css/css-color', 'css/css-transforms', 'css/CSS2',
];
// Directories added to the gate with the round 4 feature lanes (a feature passes a good share of them): their
// testharness pages run in wptrunner's 800x600 viewport. custom-elements: 190 pages (customized built-ins, lane F3a);
// css/css-font-loading: 23 pages without wptserve (FontFace, document.fonts, lane F1); css/css-typed-om: 362 pages
// (CSS Typed OM, lane F1).
export const FEATURE_DIRS = ['custom-elements', 'css/css-font-loading', 'css/css-typed-om', 'html/semantics/embedded-content/the-object-element'];
export const DEFAULT_DIRS = [
  'dom', 'html/dom', 'encoding', 'url', 'fetch/api/basic', 'xhr', 'css/selectors', 'uievents', 'shadow-dom', 'webstorage',
  ...PAINT_DIRS,
  ...FEATURE_DIRS,
];
// `--dirs all` (run.mjs): `keep` (a kind's directories, so their pages keep their `dir` and its rules) and every other
// directory of the checkout, the fewest that cover it: a directory that holds none of `keep` whole, else its
// subdirectories, recursively. Files directly in a directory that is split this way are not covered (none are tests
// at the pinned revision but WPT's root-level pages).
// Top-level directories WPT's manifest holds no tests in (its tools, the harness's own self-tests, conformance-checker
// samples): pages there are not tests.
export const NON_TEST_TOP = ['tools', 'resources', 'conformance-checkers'];
export function allDirs(wpt, keep) {
  const out = [...keep];
  const visit = (rel) => {
    for (const e of fs.readdirSync(path.join(wpt, rel), { withFileTypes: true })) {
      if (!e.isDirectory() || e.name.startsWith('.') || (!rel && NON_TEST_TOP.includes(e.name))) continue;
      const d = rel ? `${rel}/${e.name}` : e.name;
      if (keep.includes(d)) continue;
      if (keep.some((k) => k.startsWith(d + '/'))) visit(d); else out.push(d);
    }
  };
  visit('');
  return out;
}
// The directories whose testharness pages run in Playwright's 1280x720, the viewport their baseline was recorded with.
const LEGACY_VIEWPORT_DIRS = DEFAULT_DIRS.filter((d) => !PAINT_DIRS.includes(d) && !FEATURE_DIRS.includes(d));
// Testharness pages in every other directory run as wptrunner runs every page: in an 800x600 viewport (run.mjs; the
// browser has Ahem for every page). The older directories keep the viewport their baseline was recorded with
// (Playwright's 1280x720).
const WPTRUNNER_VIEWPORT = { width: 800, height: 600 };

// ---------- discovery ----------
function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) { if (!['resources', 'support', 'reference', 'tools', 'crashtests'].includes(e.name)) walk(p, out); }
    else out.push(p);
  }
  return out;
}

const stripComments = (src) => src.replace(/<!--[\s\S]*?-->/g, '');
// Pages whose own source names no Python handler outside a comment, but which need wptserve all the same: kept out
// by path (and, for the generated security pages, by the helper they load). None is in a directory we run today;
// the patterns keep them out when their directories are added.
//   referrer-policy/, content-security-policy/: generated pages (the generator comment names a .py file) that load
//     /common/security-features/, whose resources are wptserve handlers
//   html/syntax/speculative-parsing/, html/semantics/**/moving-between-documents/: generated pages whose resources
//     are wptserve handlers (stash, delays)
const SERVER_PAGES = [
  /^(referrer-policy|content-security-policy)\//,
  /^html\/syntax\/speculative-parsing\//,
  /^html\/semantics\/.*\/moving-between-documents\//,
];
const SERVER_HELPERS = /\/common\/security-features\//;

// Every testharness page under `dirs` that our static server can run, one entry per variant.
// `excluded` counts what was left out and why (identical for every binary).
// `reasons` (a Map, optional) receives each left-out file's reason, keyed by its path (coverage.mjs).
// `wptserve`: the server is wptserve (run.mjs --server wptserve), so pages that need it are kept.
export function discover(wpt, dirs, { sampleEncoding = 1, reasons, wptserve = false, testdriver = false } = {}) {
  const tests = [];
  const excluded = {};
  let rel;
  const exclude = (why) => { excluded[why] = (excluded[why] ?? 0) + 1; reasons?.set(rel, why); };
  for (const d of dirs) {
    for (const f of walk(path.join(wpt, d))) {
      rel = path.relative(wpt, f).split(path.sep).join('/');
      const base = path.basename(f);
      // `code`: the source the wptserve check reads (markup without its comments).
      let src, code, url, variants = [''], long = false;
      if (/\.(any|window)\.js$/.test(base)) {
        src = code = fs.readFileSync(f, 'utf8');
        const g = src.match(/^\/\/\s*META:\s*global=(.*)$/m);
        if (base.endsWith('.any.js') && g && !g[1].split(',').map((s) => s.trim()).some((x) => x === 'window' || x === 'default')) { exclude('no window global'); continue; }
        url = rel.replace(/\.js$/, '.html');
        const v = [...src.matchAll(/^\/\/\s*META:\s*variant=(.*)$/gm)].map((m) => m[1].trim());
        if (v.length) variants = v;
        long = /META:\s*timeout=long/.test(src);
      } else if (MARKUP.test(base)) {
        if (/-ref\.|-manual\.|^ref-/.test(base) || /\.(any|window|worker)\.html$/.test(base)) continue;
        src = fs.readFileSync(f, 'utf8');
        // A testharness page loads the harness with a script element (XML and SVG pages: src or xlink:href, with
        // or without a prefix); a page that only names it in a comment or a string is not one.
        code = stripComments(src);
        if (!tagsOf(code, 'script').some((a) => /(^|\/)resources\/testharness\.js$/.test(a.src ?? a['xlink:href'] ?? a.href ?? ''))) continue;
        url = rel;
        const v = [...src.matchAll(/<meta\s+name=["']?variant["']?\s+content=["']([^"']*)["']/gi)].map((m) => m[1]);
        if (v.length) variants = v;
        long = /<meta\s+name=["']?timeout["']?\s+content=["']?long/i.test(src);
      } else continue;
      if (!wptserve && /\.sub\.|\.https\.|\.h2\.|\.www\./.test(base)) { exclude('needs wptserve substitution/https/h2'); continue; }
      if (!testdriver && /testdriver\.js|testdriver-vendor/.test(src)) { exclude('needs testdriver'); continue; }
      if (!wptserve && (/\.py\b/.test(code) || SERVER_PAGES.some((re) => re.test(rel)) || SERVER_HELPERS.test(code))) { exclude('needs wptserve python handler'); continue; }
      // wasm/core/: the spec interpreter's tests, compiled and run in the page (CPU-bound, 0.5-10 s alone): beside 15
      // other pages (wpt-full.yml --jobs 16) the larger SIMD ones outran the normal timeout, so they get the long one.
      if (rel.startsWith('wasm/core/')) long = true;
      const viewport = LEGACY_VIEWPORT_DIRS.includes(d) ? {} : { viewport: WPTRUNNER_VIEWPORT };
      for (const v of variants) tests.push({ dir: d, url: url + v, long, ...viewport });
    }
  }
  // sampleEncoding N: keep every Nth encoding/ page (identical selection for every browser)
  let k = 0;
  return { tests: tests.filter((t) => t.dir !== 'encoding' || (k++ % sampleEncoding === 0)), excluded };
}

// ---------- reftest discovery ----------
// The directories the reftest gate runs (reftest.mjs; README "Reftests"): the paint directories, with all of CSS2,
// whose ~6,200 reftests (layout as much as paint) cost about 50 s of the run on the CI runner, the reftests of
// the testharness directories that have any (selector invalidation, dir=auto and bidi, shadow trees), and the
// layout directories.
// LAYOUT_DIRS: the core CSS layout and style directories, run as reftests only (their testharness pages, about as
// many again as the testharness run has today, are not run yet: README "Reftests").
export const LAYOUT_DIRS = [
  'css/css-flexbox', 'css/css-sizing', 'css/css-position', 'css/css-tables', 'css/css-display', 'css/css-align',
  'css/css-box', 'css/css-overflow', 'css/css-images', 'css/css-values', 'css/css-variables', 'css/css-cascade',
  'css/css-conditional', 'css/css-nesting', 'css/css-lists', 'css/css-pseudo', 'css/css-content', 'css/css-logical',
  'css/cssom', 'css/cssom-view', 'css/mediaqueries', 'css/css-syntax', 'css/css-shadow', 'css/css-transitions',
  'css/css-animations', 'css/css-properties-values-api', 'css/css-grid', 'css/css-text', 'css/css-fonts',
  'css/css-inline', 'css/css-borders',
];
export const REFTEST_DIRS = [...PAINT_DIRS, 'css/selectors', 'html/dom', 'shadow-dom', 'dom', 'encoding', ...LAYOUT_DIRS];
// Reftests left out by path, with the reason discovery counts them under (`excluded`). A pattern is a prefix of the
// page's URL or a whole URL.
//   css/css-grid/grid-lanes/: masonry layout, which pyro (Taffy) does not implement and Chrome ships behind a flag:
//     911 reftests of which Chrome passes 120 (13%). Failing pages still run on every pull request (only a page
//     that times out with nothing passing is skipped), so these would cost about a fifth of the layout directories'
//     time and protect nothing.
export const REFTEST_SKIP = [
  { prefix: 'css/css-grid/grid-lanes/', why: 'css-grid grid-lanes (masonry: not implemented)' },
];
const skipReftest = (rel) => REFTEST_SKIP.find((x) => rel === x.prefix || (x.prefix.endsWith('/') && rel.startsWith(x.prefix)));

// Markup files as WPT's manifest reads them (tools/manifest/sourcefile.py): the tags it looks for, by regular
// expression rather than a parser. A tag may carry a namespace prefix (SVG tests use <h:link>); comments are dropped.
const MARKUP = /\.(html|htm|xht|xhtml|xml|svg)$/;
const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" };
const unescapeAttr = (s) => s.replace(/&(amp|lt|gt|quot|apos);/g, (_, e) => ENTITIES[e]);
function tagsOf(src, name) {
  const out = [];
  for (const m of src.matchAll(new RegExp(`<(?:[A-Za-z][\\w-]*:)?${name}\\b((?:"[^"]*"|'[^']*'|[^'">])*)>`, 'gi'))) {
    const a = {};
    for (const x of m[1].matchAll(/([^\s=/>"']+)\s*(?:=\s*("[^"]*"|'[^']*'|[^\s>"']+))?/g)) {
      a[x[1].toLowerCase()] = x[2] ? unescapeAttr(x[2].replace(/^(["'])([\s\S]*)\1$/, '$2')) : '';
    }
    out.push(a);
  }
  return out;
}
const metaOf = (tags, name) => tags.filter((a) => a.name === name).map((a) => a.content ?? '');
// A URL relative to the WPT root, from `href` in the file at `rel` (a query or fragment is kept).
function resolveIn(rel, href) {
  if (href.trim() === 'about:blank') return 'about:blank';
  const u = new URL(href.trim(), `http://wpt.invalid/${rel}`);
  return u.host === 'wpt.invalid' ? decodeURIComponent(u.pathname).slice(1) + u.search : null;
}
// <meta name=fuzzy content="[ref-url:]maxDifference=a-b;totalPixels=c-d"> (names optional, a single value n means
// n-n), as the manifest parses it: { '': [[a, b], [c, d]], '<ref url>': ... }, '' for the unprefixed value.
export function parseFuzzy(contents, rel, refUrls) {
  const out = {};
  for (const content of contents) {
    const i = content.lastIndexOf(':');
    let key = '', value = content;
    if (i >= 0) {
      key = resolveIn(rel, content.slice(0, i));
      if (!refUrls.includes(key)) throw new Error(`fuzzy key ${content.slice(0, i)} names no reference`);
      value = content.slice(i + 1);
    }
    const parts = value.split(';');
    if (parts.length !== 2) throw new Error(`malformed fuzzy value ${value}`);
    const named = {}, positional = [];
    for (let p of parts) {
      let name = null;
      if (p.includes('=')) {
        [name, p] = p.split('=', 2).map((s) => s.trim());
        if (name !== 'maxDifference' && name !== 'totalPixels') throw new Error(`${name} is not a fuzzy property`);
      }
      const [lo, hi = lo] = p.split('-').map((s) => s.trim());
      const range = [Number(lo), Number(hi)];
      if (!range.every((n) => Number.isInteger(n) && n >= 0)) throw new Error(`fuzzy value ${p} is not a range of integers`);
      if (name) named[name] = range; else positional.push(range);
    }
    if (key in out) throw new Error(`two fuzzy values for ${key || 'every reference'}`);
    out[key] = ['maxDifference', 'totalPixels'].map((n) => named[n] ?? positional.shift());
  }
  return out;
}

function walkMarkup(dir, out = []) {
  let entries;
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return out; }
  for (const e of entries) {
    const p = path.join(dir, e.name);
    // The manifest's non-test directories: nothing under them is a test.
    if (e.isDirectory()) { if (!['resources', 'support', 'tools'].includes(e.name)) walkMarkup(p, out); }
    else if (MARKUP.test(e.name)) out.push(p);
  }
  return out;
}

// What a page needs from wptserve that serve.mjs does not do: substitutions, pipes, Python handlers, other hosts.
const SERVER_NAME = /\.(sub|https|h2|www)\./;
const SERVER_CONTENT = /\.py\b|[?&]pipe=|\.sub\.(html|htm|xht|xhtml|svg|css|js)\b/;

// Every reftest under `dirs` that serve.mjs can serve, as WPT's manifest classifies files: a markup file with
// <link rel=match|mismatch href> that is not a manual, visual, crash or print test, not a testharness page and not
// in a non-test directory. A reference that has references of its own is a test of its own too; as in wptrunner, a
// test is compared with its own references only (the chain is not followed). `excluded` counts what was left out
// and why; `info` counts what discovery noticed but kept.
// `reasons` (a Map, optional) receives each left-out file's reason, keyed by its path (coverage.mjs).
export function discoverReftests(wpt, dirs, { reasons, wptserve = false, testdriver = false } = {}) {
  const tests = [];
  const excluded = {};
  const info = {};
  let rel;
  const exclude = (why, at = rel) => { excluded[why] = (excluded[why] ?? 0) + 1; reasons?.set(at, why); };
  const note = (why) => { info[why] = (info[why] ?? 0) + 1; };
  const parsed = new Map();
  const parse = (rel) => {
    if (parsed.has(rel)) return parsed.get(rel);
    if (rel === 'about:blank') return { src: '', links: [], metas: [], scripts: [] };
    let src = null;
    try { src = fs.readFileSync(path.join(wpt, rel.replace(/[?#].*$/, '')), 'utf8').replace(/<!--[\s\S]*?-->/g, ''); } catch {}
    const r = src == null ? null : { src, links: tagsOf(src, 'link'), metas: tagsOf(src, 'meta'), scripts: tagsOf(src, 'script') };
    parsed.set(rel, r);
    return r;
  };
  const refsOf = (rel, p) => p.links
    .filter((a) => (a.rel === 'match' || a.rel === 'mismatch') && a.href != null)
    .map((a) => ({ url: resolveIn(rel, a.href), rel: a.rel === 'match' ? '==' : '!=' }));
  const referenced = new Set();
  const noRefs = [];
  for (const d of dirs) {
    for (const f of walkMarkup(path.join(wpt, d))) {
      rel = path.relative(wpt, f).split(path.sep).join('/');
      const base = path.basename(f);
      const name = base.replace(/\.[^.]*$/, '');
      const flag = name.includes('-') ? name.slice(name.lastIndexOf('-') + 1).split('.')[0] : null;
      const parts = rel.split('/').slice(0, -1);
      const isRefName = rel.includes('/reference/') || /(^|[-_])(not)?ref[0-9]*([-_]|$)/.test(name);
      if (flag === 'manual') { exclude('manual'); continue; }
      if (flag === 'visual') { exclude('visual (no reference)'); continue; }
      if (flag === 'crash' || parts.includes('crashtests')) { exclude('crash test'); continue; }
      const p = parse(rel);
      if (!p) continue;
      const refs = refsOf(rel, p);
      for (const r of refs) if (r.url) referenced.add(r.url.replace(/[?#].*$/, ''));
      if (flag === 'print' || parts.includes('print')) { if (refs.length) exclude('print reftest'); continue; }
      const cssFlags = new Set(metaOf(p.metas, 'flags').flatMap((c) => c.split(/\s+/)));
      if (!isRefName && ['animated', 'font', 'history', 'interact', 'paged', 'speech', 'userstyle'].some((x) => cssFlags.has(x))) { if (refs.length) exclude('manual (CSS flags)'); continue; }
      if (p.scripts.some((a) => /(^|\/)resources\/testharness\.js$/.test(a.src ?? ''))) { note('testharness page (not a reftest)'); continue; }
      if (!refs.length) { noRefs.push({ rel, isRefName }); continue; }
      if (refs.some((r) => !r.url)) { exclude('reference on another host'); continue; }
      // The page and its references, as served: wptserve features serve.mjs lacks, and testdriver.
      const files = [rel, ...refs.map((r) => r.url)];
      const ps = files.map((u) => parse(u.replace(/[?#].*$/, '')));
      if (ps.some((x) => !x)) { exclude('reference not in the checkout'); continue; }
      if (!wptserve && (files.some((u) => SERVER_NAME.test(u) || /[?&]pipe=/.test(u)) || ps.some((x) => SERVER_CONTENT.test(x.src)))) { exclude('needs wptserve (substitution, pipe, Python handler)'); continue; }
      if (!testdriver && ps.some((x) => x.scripts.some((a) => /(^|\/)resources\/testdriver(-vendor)?\.js$/.test(a.src ?? '')))) { exclude('needs testdriver'); continue; }
      if (metaOf(p.metas, 'device-pixel-ratio').length) { exclude('needs another device pixel ratio'); continue; }
      let fuzzy;
      try { fuzzy = parseFuzzy(metaOf(p.metas, 'fuzzy'), rel, refs.map((r) => r.url)); } catch { exclude('malformed fuzzy metadata'); continue; }
      const vs = metaOf(p.metas, 'viewport-size')[0]?.match(/^\s*(\d+)\s*x\s*(\d+)\s*$/);
      if (ps.slice(1).some((x) => refsOf('', x).length)) note('a reference has references of its own (compared one level, as wptrunner does)');
      const skip = skipReftest(rel);
      if (skip) { exclude(skip.why); continue; }
      const long = metaOf(p.metas, 'timeout').some((c) => c.trim() === 'long');
      const variants = metaOf(p.metas, 'variant');
      for (const v of variants.length ? variants : ['']) {
        tests.push({
          dir: d, url: rel + v, long,
          refs: refs.map((r) => ({ url: r.url + v, rel: r.rel })),
          ...(Object.keys(fuzzy).length ? { fuzzy } : {}),
          ...(vs ? { viewport: { width: Number(vs[1]), height: Number(vs[2]) } } : {}),
        });
      }
    }
  }
  // Markup without references: reference files (named or linked as one) and the rest (visual tests, support pages).
  for (const x of noRefs) exclude(x.isRefName || referenced.has(x.rel) ? 'reference file' : 'no reference (visual test or support page)', x.rel);
  return { tests, excluded, info };
}

// ---------- results ----------
export const readJson = (f) => JSON.parse(fs.readFileSync(f, 'utf8'));
// The subtest names a page reported, once each, and those that passed. The report shim keys subtests by name, so a
// name appears once; if one ever appears twice, it passes only if every report of it passed.
export const names = (t) => [...new Set((t.subtests ?? []).map((s) => s.name))];
export function passing(t) {
  const ok = new Map();
  for (const s of t.subtests ?? []) ok.set(s.name, (ok.get(s.name) ?? true) && s.status === 'Pass');
  return [...ok].filter(([, v]) => v).map(([n]) => n);
}
// A short hash of a set of subtest names (order-independent).
export const namesHash = (list) => crypto.createHash('sha1').update([...new Set(list)].sort().join('\n')).digest('hex').slice(0, 8);

// ---------- baseline ----------
// baseline.json: { wpt: {repo, revision}, dirs, tests: { url: entry } }, one entry per page, lists sorted:
//   { status: 'complete', ms, n, fail: [names], h }  a completed page that reported these n subtests (h: namesHash
//       of their names) and passed all but `fail`. Used when that list is shorter: most pages pass most subtests,
//       and naming every pass would make the file ~38 MB instead of ~0.5 MB. The passes are "every other name", so
//       they can be checked by name only against a run that reported the same names (same h).
//   { status, ms, n, pass: [names] }                  any page: these passed (of n reported). Used for pages that did
//       not complete, when it is shorter, and when the runs that made the entry disagreed on the names.
// ms is the page's duration (rounded), only used to balance shards and to bound a page's timeout in the gate.
export const roundMs = (ms) => (ms < 1000 ? Math.round(ms / 10) * 10 : Math.round(ms / 100) * 100);
export function entryOf(status, ms, all, pass, n) {
  const e = { status, ms: roundMs(ms ?? 0), n: all ? all.length : n ?? pass.length };
  if (status === 'complete' && all && pass.length > all.length - pass.length) {
    const p = new Set(pass);
    return { ...e, fail: all.filter((x) => !p.has(x)).sort(), h: namesHash(all) };
  }
  return { ...e, pass: [...pass].sort() };
}
export const passCount = (e) => (e.pass ? e.pass.length : e.n - e.fail.length);
// The file: one page per line, sorted, so a baseline change reads as a small diff and two branches' changes merge
// (merge-baseline.mjs merges them page by page).
export function formatBaseline(head, tests) {
  const urls = Object.keys(tests).sort();
  let s = JSON.stringify(head, null, 1).replace(/\n}$/, ',\n "tests": {\n');
  s += urls.map((u) => `  ${JSON.stringify(u)}: ${JSON.stringify(tests[u])}`).join(',\n');
  s += '\n }\n}\n';
  return s;
}

// A baseline is one file, or a directory of them, one per top-level WPT directory (`<dir>/<top>.json`, each in the
// format above with the same head), for the whole suite's ~70,000 pages: each file stays small enough to diff and
// to merge page by page (merge-baseline.mjs), and a change touches only its directories' files.
export const topOf = (url) => url.split('/')[0];
export function readBaselineAt(p) {
  if (!fs.existsSync(p)) return null;
  if (!fs.statSync(p).isDirectory()) return readJson(p);
  const files = fs.readdirSync(p).filter((f) => f.endsWith('.json')).sort();
  if (!files.length) return null;
  let head = null;
  const tests = {};
  for (const f of files) {
    const j = readJson(path.join(p, f));
    if (!head) head = { ...j, tests: undefined };
    else if (j.wpt?.revision !== head.wpt?.revision) throw new Error(`${path.join(p, f)} is for WPT ${j.wpt?.revision}, ${files[0]} for ${head.wpt?.revision}`);
    for (const [u, e] of Object.entries(j.tests)) {
      if (topOf(u) + '.json' !== f) throw new Error(`${path.join(p, f)} holds ${u}, which belongs in ${topOf(u)}.json`);
      tests[u] = e;
    }
  }
  delete head.tests;
  return { ...head, tests };
}
// Writes `tests` to `p`: a directory (an existing one, or a path ending in '/') gets one file per top-level
// directory, and loses the files of directories no page is in any more; anything else is one file.
export function writeBaselineAt(p, head, tests) {
  const dir = p.endsWith('/') || (fs.existsSync(p) && fs.statSync(p).isDirectory());
  if (!dir) { fs.writeFileSync(p, formatBaseline(head, tests)); return; }
  fs.mkdirSync(p, { recursive: true });
  const groups = {};
  for (const [u, e] of Object.entries(tests)) (groups[topOf(u)] ??= {})[u] = e;
  for (const f of fs.readdirSync(p)) if (f.endsWith('.json') && !groups[f.slice(0, -5)]) fs.unlinkSync(path.join(p, f));
  for (const [top, t] of Object.entries(groups)) {
    const file = path.join(p, `${top}.json`);
    const s = formatBaseline(head, t);
    // A file whose pages did not change is left as it was.
    if (!fs.existsSync(file) || fs.readFileSync(file, 'utf8') !== s) fs.writeFileSync(file, s);
  }
}

// What every run of a page agrees on (a run and its reruns): it completed only if every run did, a subtest passed
// only if it passed in every run, and `all` is its set of names when every run completed with the same one.
// ms: the first completed run's duration (a timed-out run's says nothing about the page).
export function consensus(runs) {
  const done = runs.filter((t) => t.status === 'complete');
  const sets = done.map(names);
  const hashes = sets.map(namesHash);
  const all = done.length === runs.length && hashes.every((x) => x === hashes[0]) ? sets[0] : null;
  const passSets = runs.map((t) => new Set(passing(t)));
  const pass = [...passSets[0]].filter((n) => passSets.every((s) => s.has(n)));
  const status = done.length === runs.length ? 'complete' : runs.find((t) => t.status !== 'complete').status;
  return { status, ms: (done[0] ?? runs[0]).ms, all, pass, n: new Set(runs.flatMap(names)).size };
}

// The entry after a page's runs (their consensus), given its old entry, without losing anything: a subtest passes if
// it passed before or in every run, and the page completes if it did before or in every run. `ignore`: subtest names
// (flaky.json) whose passes are not added. Call it only for pages whose runs are not a regression (judge()).
// Durations only balance shards and bound timeouts, and a refresh should not touch every line of the file, so
// `ms` says when an old entry's duration changes: 'keep' (never: gate runs, which skip and bound pages), 'moved'
// (when it moved by 2x and 1 s or more: full runs), 'all' (always).
export const movedMs = (old, now) => Math.abs(now - old) >= 1000 && (now >= 2 * old || now <= old / 2);
export const newMs = (old, now, ms) => (ms === 'all' || (ms === 'moved' && movedMs(old, now)) ? now : old);
export function mergeEntry(old, con, { ms = 'moved', ignore } = {}) {
  const gain = new Set(ignore ? con.pass.filter((n) => !ignore.has(n)) : con.pass);
  if (!old) return entryOf(con.status, con.ms, con.all, [...gain], con.n);
  const msNow = newMs(old.ms, roundMs(con.ms ?? 0), ms);
  // A fail-list entry keeps its names (h): a subtest it lists that passed in every run leaves the list.
  if (old.fail) return { ...old, ms: msNow, fail: old.fail.filter((n) => !gain.has(n)) };
  const pass = [...new Set([...old.pass, ...gain])];
  const status = old.status === 'complete' || con.status === 'complete' ? 'complete' : old.status;
  if (pass.length === old.pass.length && status === old.status) return { ...old, ms: msNow };
  // The runs' names, when they agree and include every pass, allow the shorter fail-list form.
  const conAll = con.all && new Set(con.all);
  const all = conAll && pass.every((n) => conAll.has(n)) ? con.all : null;
  return entryOf(status, msNow, all, pass, Math.max(old.n, con.n));
}

// ---------- planning ----------
// A baseline page with no coverage to protect: it never completed and never passed a subtest, or it completed
// without passing anything after 5 s or more (testharness's own timeout ended it; checking that it still does costs
// ~10 s per page and protects nothing). The gate's plan skips it; `check` applies the same rule.
export const skippable = (b) => !passCount(b) && (b.status !== 'complete' || b.ms >= 5000);
// Estimated cost (ms) of each page, and in gate mode (a baseline and `gate`) which pages run and their timeout:
//   skipped: skippable() pages
//   limit: a page that completed gets 10x its baseline duration, at least 10 s and at most the normal timeout, so a
//     page that normally takes 0.5 s and now hangs (or flakes) costs 10 s instead of 15-45 s. Reruns use the normal
//     timeout.
export function plan(tests, baseline, { gate = false, timeout, longTimeout }) {
  const out = [], skipped = [];
  for (const t of tests) {
    const b = baseline?.tests?.[t.url];
    const def = t.long ? longTimeout : timeout;
    const p = { ...t, limit: def, est: b ? Math.max(b.ms, 20) : t.long ? 3000 : 500, known: b?.status };
    if (gate && b) {
      if (skippable(b)) { skipped.push(t.url); continue; }
      if (b.status === 'complete') p.limit = Math.min(def, Math.max(10 * b.ms, 10000));
    }
    out.push(p);
  }
  return { tests: out, skipped };
}

// Longest-processing-time-first split into `count` shards; deterministic for the same inputs. Each shard's list
// is longest first, so its slowest pages start at once instead of ending the run alone.
export function shard(tests, index, count) {
  const sorted = [...tests].sort((a, b) => b.est - a.est || (a.url < b.url ? -1 : 1));
  if (count <= 1) return sorted;
  const load = Array(count).fill(0);
  const mine = [];
  for (const t of sorted) {
    let k = 0;
    for (let i = 1; i < count; i++) if (load[i] < load[k]) k = i;
    load[k] += t.est;
    if (k === index) mine.push(t);
  }
  return mine;
}

// ---------- flaky list ----------
// flaky.json: { tests: { key: { reason, subtests?: [names], allowIncomplete?: true } } }. A key is a page URL, or a
// pattern where * matches anything (an exact URL wins). Every entry needs a reason.
//   { reason }                    the page is not gated
//   { reason, subtests }          these subtests are not gated (the page must still report them: a page whose
//                                 subtests come and go needs a whole-page entry)
//   { reason, allowIncomplete }   a run of the page that did not complete (timeout, hang, crash) is ignored; the
//                                 page is checked as usual when a run completes. check() fails if more than
//                                 MAX_IGNORED_INCOMPLETE pages did not complete in any run: that is not a flake.
export const MAX_IGNORED_INCOMPLETE = 3;
const FLAKY_KEYS = new Set(['reason', 'subtests', 'allowIncomplete']);
export function readFlaky(file) {
  const entries = file && fs.existsSync(file) ? (readJson(file).tests ?? {}) : {};
  const patterns = [];
  for (const [key, e] of Object.entries(entries)) {
    if (!e || typeof e.reason !== 'string' || !e.reason.trim()) throw new Error(`flaky.json: ${key} needs a reason`);
    const bad = Object.keys(e).filter((k) => !FLAKY_KEYS.has(k));
    if (bad.length) throw new Error(`flaky.json: ${key}: unknown field(s) ${bad.join(', ')} (allowed: ${[...FLAKY_KEYS].join(', ')})`);
    if (e.subtests && !(Array.isArray(e.subtests) && e.subtests.length && e.subtests.every((s) => typeof s === 'string'))) throw new Error(`flaky.json: ${key}: subtests must be a non-empty list of names`);
    if (e.subtests && e.allowIncomplete) throw new Error(`flaky.json: ${key}: use subtests or allowIncomplete, not both`);
    if (key.includes('*')) patterns.push([new RegExp(`^${key.split('*').map((s) => s.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*')}$`), e]);
  }
  return { entries, get: (url) => entries[url] ?? patterns.find(([re]) => re.test(url))?.[1] };
}
export const wholePage = (f) => !!f && !f.subtests && !f.allowIncomplete;

// ---------- comparison ----------
// One page's runs (its run, and its reruns if any) against its baseline entry. A subtest is lost if it passed in the
// baseline and passed in no run; a page broke if it completed in the baseline and in no run. A subtest is gained if
// it passed in every run and not in the baseline; a page is fixed if it completes in every run and did not in the
// baseline. So a flake that passes once neither fails the gate nor enters the baseline.
//   lost: names (for a fail-list entry, those its runs reported); lostUnnamed: how many passes of a fail-list entry
//     no run reported, when no run reported its names (a lower bound)
//   setChanged: a fail-list page completed, but no run reported the baseline's names (h), so which of its passes
//     are gone is unknown: a regression until the entry is replaced (gate.mjs update --replace --match)
//   unstable: the runs disagreed (status or passes): a flaky.json candidate
// `completion: false` (reftests) protects passes only: a page that passed nothing may stop completing without breaking,
// and completing without a pass is no gain (a reftest's completion carries no coverage of its own).
export function judge(b, runs, flaky, { completion = true } = {}) {
  const url = runs[0].url;
  const f = flaky?.get(url);
  const c = {
    url, status: runs[runs.length - 1].status, was: b?.status, runs: runs.length,
    lost: [], lostUnnamed: 0, gained: [], broke: false, fixed: false, setChanged: false, unstable: unstable(runs),
  };
  if (wholePage(f)) return { ...c, flaky: true };
  const ignore = new Set(f?.subtests ?? []);
  if (!b) return { ...c, isNew: true, gained: consensus(runs).pass.filter((n) => !ignore.has(n)) };
  let use = runs;
  if (f?.allowIncomplete && b.status === 'complete') {
    use = runs.filter((t) => t.status === 'complete');
    if (!use.length) return { ...c, ignoredIncomplete: true };
  }
  const passSets = use.map((t) => new Set(passing(t)));
  const passedAny = (n) => passSets.some((s) => s.has(n));
  const passedAll = (n) => passSets.every((s) => s.has(n));
  const done = use.filter((t) => t.status === 'complete');
  c.broke = completion && b.status === 'complete' && !done.length;
  c.fixed = completion && b.status !== 'complete' && done.length === use.length;
  if (b.pass) {
    const was = new Set(b.pass);
    c.lost = b.pass.filter((n) => !passedAny(n) && !ignore.has(n));
    c.gained = [...passSets[0]].filter((n) => !was.has(n) && passedAll(n) && !ignore.has(n));
    return c;
  }
  const fail = new Set(b.fail);
  c.gained = b.fail.filter((n) => passedAll(n) && !ignore.has(n));
  const exact = done.find((t) => namesHash(names(t)) === b.h);
  if (exact) {
    c.lost = names(exact).filter((n) => !fail.has(n) && !passedAny(n) && !ignore.has(n));
    return c;
  }
  // No run reported the baseline's names. Named: what no run passed and the baseline does not list as failing (a
  // lost pass, or a new subtest that fails); unnamed: baseline passes beyond every name the runs reported.
  const seen = new Set(use.flatMap(names));
  c.lost = [...seen].filter((n) => !fail.has(n) && !passedAny(n) && !ignore.has(n));
  c.lostUnnamed = Math.max(0, passCount(b) - [...seen].filter((n) => !fail.has(n)).length);
  c.setChanged = done.length > 0;
  return c;
}
function unstable(runs) {
  if (runs.length < 2) return false;
  const key = (t) => `${t.status === 'complete'}|${passing(t).sort().join('\n')}`;
  return runs.some((t) => key(t) !== key(runs[0]));
}
export const lostCount = (c) => c.lost.length + c.lostUnnamed;
export const isRegression = (c) => lostCount(c) > 0 || c.broke || c.setChanged;
export const isImprovement = (c) => c.gained.length > 0 || c.fixed;
// Pages the gate runs again: anything that may be a regression or an improvement, and a page allowed not to
// complete that did not.
export const differs = (c) => isRegression(c) || isImprovement(c) || !!c.ignoredIncomplete;

// Coverage the base branch's baseline and flaky list protect that this branch's no longer do: passes an entry
// dropped, a page that completed and is now recorded as not completing, a removed page, a fail-list entry whose
// names changed (unverifiable), and flaky.json entries added or widened.
export function weakening(baseB, b, baseFlaky, flaky) {
  const out = [];
  for (const [url, o] of Object.entries(baseB.tests)) {
    const e = b.tests[url];
    if (!e) { if (passCount(o) || o.status === 'complete') out.push({ url, what: `removed (${passCount(o)} passing subtests, ${o.status})` }); continue; }
    const why = [];
    if (o.status === 'complete' && e.status !== 'complete') why.push(`complete → ${e.status}`);
    let lost = [];
    if (o.pass && e.pass) { const s = new Set(e.pass); lost = o.pass.filter((n) => !s.has(n)); }
    else if (o.fail && e.fail && o.h === e.h) { const s = new Set(o.fail); lost = e.fail.filter((n) => !s.has(n)); }
    else if (o.fail && e.fail) why.push(`subtest names changed (${o.n} → ${e.n}; passes ${passCount(o)} → ${passCount(e)})`);
    else if (o.pass && e.fail) { const f = new Set(e.fail); lost = o.pass.filter((n) => f.has(n)); }
    if (lost.length) why.push(`${lost.length} pass(es) dropped: ${lost.slice(0, 3).join(' / ').slice(0, 200)}`);
    else if (!(o.fail && e.fail) && passCount(e) < passCount(o)) why.push(`passes ${passCount(o)} → ${passCount(e)}`);
    if (why.length) out.push({ url, what: why.join('; ') });
  }
  for (const [key, e] of Object.entries(flaky.entries)) {
    const o = baseFlaky.entries[key];
    if (!o) out.push({ url: key, what: `flaky.json entry added (${e.reason})` });
    else if (wholePage(e) && !wholePage(o)) out.push({ url: key, what: 'flaky.json entry widened to the whole page' });
    else if (e.allowIncomplete && !o.allowIncomplete) out.push({ url: key, what: 'flaky.json entry now allows the page not to complete' });
    else if (e.subtests && o.subtests) {
      const s = new Set(o.subtests);
      const more = e.subtests.filter((n) => !s.has(n));
      if (more.length) out.push({ url: key, what: `flaky.json entry ignores ${more.length} more subtest(s)` });
    }
  }
  return out;
}
