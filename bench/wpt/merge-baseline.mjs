// Git merge driver for baseline.json and reftest-baseline.json (`merge=wpt-baseline` in .gitattributes; each clone
// sets the driver up once with scripts/vendor/setup-merge-drivers.sh).
//
//   bun bench/wpt/merge-baseline.mjs BASE OURS THEIRS [MARKER_SIZE] [PATH]      (git passes %O %A %B %L %P)
//
// Merges the two sides page by page and writes the result to OURS as gate.mjs writes a baseline (lib.mjs
// formatBaseline: one page per line, sorted):
// - a page only one side changed takes that side's entry, a new page included. A page one side removed or lost passes
//   on while the other side left it alone was changed on purpose (`gate.mjs update --replace` with the
//   `wpt-accept-loss` label, or `--prune`), so that change is kept;
// - a page both sides changed keeps every pass of either side, and completes if either side's entry does: the entry
//   that passes more when its passes include the other's, else one entry with the passes of both. A page one side
//   removed and the other changed keeps the changed entry.
// A page whose two entries cannot be combined without losing a pass (`fail` entries of different subtest names: their
// passes are not named) is left as a conflict of its two lines (exit 1); the other pages are merged. Falls back to
// git's own textual merge when a file does not parse, or when both sides changed the header (WPT revision,
// directories) differently or one side changed it while the other changed pages. The gate checks the result in CI like
// any baseline change.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { formatBaseline, namesHash, passCount } from './lib.mjs';

const [baseFile, oursFile, theirsFile, markerSize = '7', where = oursFile] = process.argv.slice(2);
if (!theirsFile) {
  console.error('usage: merge-baseline.mjs BASE OURS THEIRS [MARKER_SIZE] [PATH]');
  process.exit(2);
}

function textMerge(why) {
  console.error(`merge-baseline: ${where}: ${why}; leaving a textual merge`);
  const r = spawnSync('git', ['merge-file', `--marker-size=${markerSize}`, '-L', 'ours', '-L', 'base', '-L', 'theirs', oursFile, baseFile, theirsFile], { stdio: 'inherit' });
  process.exit(r.status === 0 ? 0 : 1);
}

function read(file, side) {
  const text = fs.readFileSync(file, 'utf8');
  if (side === 'base' && !text.trim()) return { head: null, tests: {} }; // no common ancestor
  const j = JSON.parse(text);
  if (!j || typeof j.tests !== 'object' || Array.isArray(j.tests)) throw new Error(`${side}: no "tests" object`);
  for (const [url, e] of Object.entries(j.tests)) {
    const ok = e && typeof e.status === 'string' && Number.isInteger(e.n) &&
      (Array.isArray(e.pass) || (Array.isArray(e.fail) && typeof e.h === 'string'));
    if (!ok) throw new Error(`${side}: unreadable entry for ${url}`);
  }
  const { tests, ...head } = j;
  return { head, tests };
}

let base, ours, theirs;
try {
  base = read(baseFile, 'base');
  ours = read(oursFile, 'ours');
  theirs = read(theirsFile, 'theirs');
} catch (e) {
  textMerge(`cannot read it (${e.message})`);
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const sorted = (set) => [...set].sort();

// A `fail` entry names its failing subtests only. Its names are known when they are exactly its fail list plus
// `others`: the count and the hash of the names (lib.mjs namesHash) tell.
const namesAre = (e, others) => {
  const all = new Set([...e.fail, ...others]);
  return all.size === e.n && namesHash([...all]) === e.h;
};
// Every pass of y is a pass of x (as far as the entries show), and x completes if y does.
function covers(x, y) {
  if (y.status === 'complete' && x.status !== 'complete') return false;
  if (!passCount(y)) return true;
  if (x.pass && y.pass) return y.pass.every((p) => x.pass.includes(p));
  if (x.fail && y.fail) return x.h === y.h && x.fail.every((f) => y.fail.includes(f));
  if (x.fail) return y.pass.every((p) => !x.fail.includes(p)) && namesAre(x, y.pass);
  return namesAre(y, x.pass); // y's names are its failures and x's passes: y passes nothing x does not
}
// One entry with the passes of both, or null when that cannot be written without losing one.
function combine(x, y) {
  const status = x.status === 'complete' || y.status === 'complete' ? 'complete' : x.status;
  const ms = Math.max(x.ms ?? 0, y.ms ?? 0);
  if (x.pass && y.pass) return { status, ms, n: Math.max(x.n, y.n), pass: sorted(new Set([...x.pass, ...y.pass])) };
  if (x.fail && y.fail) {
    if (x.h !== y.h) return null;
    return { status: 'complete', ms, n: x.n, fail: x.fail.filter((f) => y.fail.includes(f)), h: x.h };
  }
  // A fail list and a pass list: the passes must be names the fail entry reported.
  const [f, p] = x.fail ? [x, y] : [y, x];
  if (!p.pass.every((n) => f.fail.includes(n)) && !namesAre(f, p.pass)) return null;
  return { status: 'complete', ms, n: f.n, fail: f.fail.filter((n) => !p.pass.includes(n)), h: f.h };
}

// Header.
let head;
const pagesChanged = (side) => Object.keys({ ...base.tests, ...side.tests }).some((u) => !same(base.tests[u], side.tests[u]));
if (same(ours.head, theirs.head)) head = ours.head;
else if (base.head && same(ours.head, base.head)) {
  if (pagesChanged(ours)) textMerge('theirs changed the header (WPT revision or directories) and ours changed pages');
  head = theirs.head;
} else if (base.head && same(theirs.head, base.head)) {
  if (pagesChanged(theirs)) textMerge('ours changed the header (WPT revision or directories) and theirs changed pages');
  head = ours.head;
} else textMerge('both sides changed the header (WPT revision or directories)');

// Pages.
const tests = {};
const took = { ours: 0, theirs: 0, combined: 0 };
const stuck = [];
const STUCK = 'merge-baseline: conflict';
for (const url of new Set([...Object.keys(ours.tests), ...Object.keys(theirs.tests), ...Object.keys(base.tests)])) {
  const b = base.tests[url], o = ours.tests[url], t = theirs.tests[url];
  let e;
  if (same(o, t)) e = o;
  else if (same(o, b)) { e = t; took.theirs++; }
  else if (same(t, b)) { e = o; took.ours++; }
  else if (!o || !t) e = o ?? t;
  else if (covers(o, t)) { e = o; took.ours++; }
  else if (covers(t, o)) { e = t; took.theirs++; }
  else {
    e = combine(o, t);
    if (!e) { stuck.push(url); e = STUCK; } else took.combined++;
  }
  if (e) tests[url] = e;
}

// A page whose entries cannot be combined is left as a conflict of its two lines; the rest is merged.
let out = formatBaseline(head, tests);
if (stuck.length) {
  const L = Number(markerSize) || 7;
  out = out.split('\n').map((line) => {
    const m = /^ {2}(".*"): "merge-baseline: conflict"(,?)$/.exec(line);
    if (!m) return line;
    const url = JSON.parse(m[1]);
    const row = (e) => `  ${m[1]}: ${JSON.stringify(e)}${m[2]}`;
    return [`${'<'.repeat(L)} ours`, row(ours.tests[url]), '='.repeat(L), row(theirs.tests[url]), `${'>'.repeat(L)} theirs`].join('\n');
  }).join('\n');
}
fs.writeFileSync(oursFile, out);
console.error(`merge-baseline: ${where}: merged page by page (${took.theirs} page(s) from theirs, ${took.ours} kept from ours, ${took.combined} combined)` +
  (stuck.length ? `; ${stuck.length} page(s) left as conflicts, their entries cannot be combined without losing a pass: ${stuck.slice(0, 5).join(', ')}` : ''));
process.exit(stuck.length ? 1 : 0);
