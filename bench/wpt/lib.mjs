// Shared by run.mjs and gate.mjs: test discovery, the baseline format, shard planning and the coverage comparison.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export const DEFAULT_DIRS = ['dom', 'html/dom', 'encoding', 'url', 'fetch/api/basic', 'xhr', 'css/selectors', 'uievents', 'shadow-dom', 'webstorage'];

// ---------- discovery ----------
function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) { if (!['resources', 'support', 'reference', 'tools', 'crashtests'].includes(e.name)) walk(p, out); }
    else out.push(p);
  }
  return out;
}

// Every testharness page under `dirs` that our static server can run, one entry per variant.
// `excluded` counts what was left out and why (identical for every binary).
export function discover(wpt, dirs, { sampleEncoding = 1 } = {}) {
  const tests = [];
  const excluded = {};
  const exclude = (why) => { excluded[why] = (excluded[why] ?? 0) + 1; };
  for (const d of dirs) {
    for (const f of walk(path.join(wpt, d))) {
      const rel = path.relative(wpt, f);
      const base = path.basename(f);
      let src, url, variants = [''], long = false;
      if (/\.(any|window)\.js$/.test(base)) {
        src = fs.readFileSync(f, 'utf8');
        const g = src.match(/^\/\/\s*META:\s*global=(.*)$/m);
        if (base.endsWith('.any.js') && g && !g[1].split(',').map((s) => s.trim()).some((x) => x === 'window' || x === 'default')) { exclude('no window global'); continue; }
        url = rel.replace(/\.js$/, '.html');
        const v = [...src.matchAll(/^\/\/\s*META:\s*variant=(.*)$/gm)].map((m) => m[1].trim());
        if (v.length) variants = v;
        long = /META:\s*timeout=long/.test(src);
      } else if (/\.(html|htm|xhtml)$/.test(base)) {
        if (/-ref\.|-manual\.|^ref-/.test(base) || /\.(any|window|worker)\.html$/.test(base)) continue;
        src = fs.readFileSync(f, 'utf8');
        if (!src.includes('/resources/testharness.js')) continue;
        url = rel;
        const v = [...src.matchAll(/<meta\s+name=["']?variant["']?\s+content=["']([^"']*)["']/gi)].map((m) => m[1]);
        if (v.length) variants = v;
        long = /<meta\s+name=["']?timeout["']?\s+content=["']?long/i.test(src);
      } else continue;
      if (/\.sub\.|\.https\.|\.h2\.|\.www\./.test(base)) { exclude('needs wptserve substitution/https/h2'); continue; }
      if (/testdriver\.js|testdriver-vendor/.test(src)) { exclude('needs testdriver'); continue; }
      if (/\.py\b/.test(src)) { exclude('needs wptserve python handler'); continue; }
      for (const v of variants) tests.push({ dir: d, url: url + v, long });
    }
  }
  // sampleEncoding N: keep every Nth encoding/ page (identical selection for every browser)
  let k = 0;
  return { tests: tests.filter((t) => t.dir !== 'encoding' || (k++ % sampleEncoding === 0)), excluded };
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
    const p = { ...t, limit: def, est: b ? Math.max(b.ms, 20) : t.long ? 3000 : 500 };
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
export function judge(b, runs, flaky) {
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
  c.broke = b.status === 'complete' && !done.length;
  c.fixed = b.status !== 'complete' && done.length === use.length;
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
