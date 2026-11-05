// Shared by run.mjs and gate.mjs: test discovery, the baseline format, shard planning and the coverage comparison.
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
export const passing = (t) => (t.subtests ?? []).filter((s) => s.status === 'Pass').map((s) => s.name);

// ---------- baseline ----------
// baseline.json: { wpt: {repo, revision}, dirs, source, tests: { url: entry } }, one entry per page:
//   { status, ms, n, pass: [names] }   n subtests reported, these passed
//   { status, ms, n, fail: [names] }   n subtests reported, all but these passed: used for a completed page where
//                                       that list is shorter. Most pages pass most subtests, and naming every pass
//                                       would make the file ~38 MB instead of ~0.5 MB.
// ms is the page's duration (rounded), only used to balance shards and to bound a page's timeout in the gate.
export const roundMs = (ms) => (ms < 1000 ? Math.round(ms / 10) * 10 : Math.round(ms / 100) * 100);
export function entryFromResult(t) {
  const names = (t.subtests ?? []).map((s) => s.name);
  const pass = passing(t);
  const e = { status: t.status, ms: roundMs(t.ms ?? 0), n: names.length };
  if (t.status === 'complete' && pass.length > names.length - pass.length) {
    e.fail = (t.subtests ?? []).filter((s) => s.status !== 'Pass').map((s) => s.name);
  } else e.pass = pass;
  return e;
}
export const passCount = (e) => (e.pass ? e.pass.length : e.n - e.fail.length);

// Merges a new result into an old entry without losing passes: a subtest that passed in either still passes, and a
// page that completed in either still completes.
// Durations only balance shards and bound timeouts, and a refresh should not touch every line of the file, so
// `ms` says when an old entry's duration changes: 'keep' (never: gate runs, which skip and bound pages), 'moved'
// (when it moved by 2x and 1 s or more: full runs), 'all' (always).
export const movedMs = (old, now) => Math.abs(now - old) >= 1000 && (now >= 2 * old || now <= old / 2);
export const newMs = (old, now, ms) => (ms === 'all' || (ms === 'moved' && movedMs(old, now)) ? now : old);
export function mergeEntry(old, t, { ms = 'moved' } = {}) {
  const now = entryFromResult(t);
  if (!old) return now;
  const keepMs = (e) => ({ ...e, ms: newMs(old.ms, e.ms, ms) });
  if (t.status !== 'complete') {
    if (old.status === 'complete') return old;
    const pass = [...new Set([...(old.pass ?? []), ...now.pass])];
    return keepMs({ ...now, n: Math.max(now.n, pass.length), pass });
  }
  // Completed now: the new result names every subtest it reported; add back the old passes.
  const nowNames = new Set(t.subtests.map((s) => s.name));
  // A fail-list entry names only its failures: its passes are "everything else", which can be matched against this
  // run only when the page reported the same number of subtests. Otherwise keep the entry that passes more.
  if (old.fail && nowNames.size !== old.n) return passCount(now) >= passCount(old) ? keepMs(now) : old;
  const passedBefore = old.pass ? new Set(old.pass) : null;
  const failedBefore = old.fail ? new Set(old.fail) : null;
  const passes = (s) => s.status === 'Pass' || (passedBefore ? passedBefore.has(s.name) : !failedBefore.has(s.name));
  const merged = { ...t, subtests: t.subtests.map((s) => (passes(s) ? { ...s, status: 'Pass' } : s)) };
  // Old passes the new run did not report at all (pass-list form only) stay as passes.
  if (passedBefore) for (const n of passedBefore) if (!nowNames.has(n)) merged.subtests.push({ name: n, status: 'Pass' });
  return keepMs(entryFromResult(merged));
}

// ---------- planning ----------
// Estimated cost (ms) of each page, and in gate mode (a baseline and `gate`) which pages run and their timeout:
//   skipped: pages with no coverage to protect: never completed and never passed a subtest, or completed without
//     passing anything after 5 s or more (testharness's own timeout ended them; checking that it still does costs
//     ~10 s per page and protects nothing)
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
      const pc = passCount(b);
      if (!pc && (b.status !== 'complete' || b.ms >= 5000)) { skipped.push(t.url); continue; }
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
// flaky.json: { tests: { url: { reason, subtests?: [names] } } }: the whole page, or only the listed subtests, is
// not gated. Every entry needs a reason.
export function readFlaky(file) {
  if (!fs.existsSync(file)) return {};
  const f = readJson(file).tests ?? {};
  for (const [url, e] of Object.entries(f)) if (!e.reason?.trim()) throw new Error(`flaky.json: ${url} needs a reason`);
  return f;
}

// ---------- comparison ----------
// One page against its baseline entry:
//   lost: subtests that passed in the baseline and do not pass now (named when the page reported them; lostUnnamed
//     counts passes of a fail-list entry that the page no longer reports at all)
//   gained: subtests that pass now and did not in the baseline (gainedUnnamed: a fail-list page reporting more passes)
//   broke: the page completed in the baseline and does not now (timeout, hang, crash, navigation error)
export function compareTest(b, t, flaky) {
  const f = flaky?.[t.url];
  const base = { url: t.url, status: t.status, was: b?.status, lost: [], lostUnnamed: 0, gained: [], gainedUnnamed: 0, broke: false };
  if (f && !f.subtests) return { ...base, flaky: true };
  const now = new Set(passing(t));
  if (!b) return { ...base, isNew: true, gained: [...now] };
  const ignore = new Set(f?.subtests ?? []);
  const c = { ...base, before: passCount(b), after: now.size };
  if (b.pass) {
    const was = new Set(b.pass);
    c.lost = b.pass.filter((n) => !now.has(n) && !ignore.has(n));
    c.gained = [...now].filter((n) => !was.has(n));
  } else {
    const fail = new Set(b.fail);
    const still = [...now].filter((n) => !fail.has(n)).length;
    const nonPass = (t.subtests ?? []).filter((s) => s.status !== 'Pass' && !fail.has(s.name));
    c.lost = nonPass.filter((s) => !ignore.has(s.name)).map((s) => s.name);
    c.lostUnnamed = Math.max(0, passCount(b) - still - nonPass.length);
    c.gained = [...now].filter((n) => fail.has(n));
    c.gainedUnnamed = Math.max(0, still - passCount(b));
  }
  c.broke = b.status === 'complete' && t.status !== 'complete';
  return c;
}
export const lostCount = (c) => c.lost.length + c.lostUnnamed;
export const gainedCount = (c) => c.gained.length + c.gainedUnnamed;
export const isRegression = (c) => lostCount(c) > 0 || c.broke;
export const differs = (c) => isRegression(c) || gainedCount(c) > 0;

// A page that ran twice (the gate reruns every page whose first result differs from the baseline): a subtest is
// lost only if it was lost both times, gained only if it was gained both times, and the page broke only if it broke
// both times. `result` is what the baseline update takes: the second run, with passes only where both runs passed.
export function combine(b, first, again, flaky) {
  const c1 = compareTest(b, first, flaky);
  if (!again) return { ...c1, result: first };
  const c2 = compareTest(b, again, flaky);
  const both = (x, y) => { const s = new Set(y); return x.filter((n) => s.has(n)); };
  const pass1 = new Set(passing(first));
  const result = { ...again, subtests: (again.subtests ?? []).map((s) => (s.status === 'Pass' && !pass1.has(s.name) ? { ...s, status: 'Fail', msg: 'passed only in the rerun' } : s)) };
  return {
    ...c2,
    status: again.status,
    lost: both(c1.lost, c2.lost),
    lostUnnamed: Math.min(c1.lostUnnamed, c2.lostUnnamed),
    gained: both(c1.gained, c2.gained),
    gainedUnnamed: Math.min(c1.gainedUnnamed, c2.gainedUnnamed),
    broke: c1.broke && c2.broke,
    rerun: { first: first.status, firstRegression: isRegression(c1), firstGained: gainedCount(c1) },
    result,
  };
}
