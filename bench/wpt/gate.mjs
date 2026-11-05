// WPT coverage gate: a subtest that passes in baseline.json must still pass, and a page that completed must still
// complete. See bench/wpt/README.md.
//
//   bun bench/wpt/gate.mjs run --wpt DIR --bin PATH [--out FILE] [--jobs N] [--shard I/N] [--full]
//                              [--dirs a,b] [--match RE] [--retest PREVIOUS.json] [--no-rerun] [--no-check]
//                              [--summary FILE] [--reuse-context] [--timeout MS] [--long-timeout MS]
//       Runs the gate's plan (every page with --full, or when there is no baseline for this WPT revision yet),
//       reruns once every page whose result differs from the baseline, then checks the result.
//       --retest PREVIOUS.json runs only the pages whose result in that file differs from the baseline.
//   bun bench/wpt/gate.mjs check FILE... [--summary FILE]
//       Merges shard results and compares them with the baseline. Exit 1 on a regression, a missing shard, a
//       planned page without a result, or a baseline page that no shard planned. Without a baseline for the run's
//       WPT revision it only reports (bootstrap).
//   bun bench/wpt/gate.mjs update FILE... [--out FILE] [--replace] [--prune] [--durations]
//       Writes the results into the baseline. By default it only adds: a subtest that passed in the baseline or in
//       both runs of a rerun page passes. --replace takes the results as they are (accepting losses), --prune drops
//       pages no full run discovered. Durations change on full runs when they moved by 2x and 1 s, never on gate
//       runs; --durations refreshes all of them. Without a baseline (or for a new WPT revision) it starts a new one
//       from full runs.
//   bun bench/wpt/gate.mjs diff A.json[,A2.json...] B.json[,B2.json...]
//       Pages whose status or passing subtests differ between two sets of results (e.g. two binaries, or the
//       runner with and without --reuse-context).
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { combine, compareTest, differs, entryFromResult, gainedCount, isRegression, lostCount, mergeEntry, newMs, passCount, passing, readFlaky, readJson } from './lib.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const [cmd, ...argv] = process.argv.slice(2);
const FLAGS = ['--full', '--no-rerun', '--no-check', '--replace', '--prune', '--durations', '--reuse-context'];
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const flag = (k) => argv.includes(k);
const positional = () => argv.filter((a, i) => !a.startsWith('--') && !(argv[i - 1]?.startsWith('--') && !FLAGS.includes(argv[i - 1])));
const BASELINE_FILE = path.resolve(opt('--baseline', path.join(HERE, 'baseline.json')));
const FLAKY_FILE = path.resolve(opt('--flaky', path.join(HERE, 'flaky.json')));
const WPT_REPO = 'https://github.com/web-platform-tests/wpt';
const GH = !!process.env.GITHUB_ACTIONS;
const RUN_URL = process.env.GITHUB_RUN_ID && `${process.env.GITHUB_SERVER_URL}/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}`;
const readBaseline = () => (fs.existsSync(BASELINE_FILE) ? readJson(BASELINE_FILE) : null);
const rel = (f) => { const r = path.relative(process.cwd(), f); return r.startsWith('..') ? f : r; };
const writeJson = (f, j) => fs.writeFileSync(f, JSON.stringify(j));

function runRunner(args) {
  const r = spawnSync(process.execPath, [path.join(HERE, 'run.mjs'), ...args], { stdio: 'inherit' });
  if (r.status !== 0) throw new Error(`run.mjs exited with ${r.status}`);
}
function gitRevision(dir) {
  const r = spawnSync('git', ['-C', dir, 'rev-parse', 'HEAD'], { encoding: 'utf8' });
  return r.status === 0 ? r.stdout.trim() : null;
}

// ---------- run ----------
function run() {
  const wpt = opt('--wpt', process.env.WPT_DIR);
  const bin = opt('--bin', process.env.LIGHTNINGPANDA_BIN);
  if (!wpt || !bin) throw new Error('run needs --wpt DIR (or WPT_DIR) and --bin PATH (or LIGHTNINGPANDA_BIN)');
  let baseline = readBaseline();
  const rev = gitRevision(wpt);
  if (baseline && rev && rev !== baseline.wpt.revision) {
    console.error(`warning: ${wpt} is at ${rev}; the baseline is for ${baseline.wpt.revision}. Running every page (bootstrap).`);
    baseline = null;
  }
  const full = flag('--full') || !baseline;
  const out = path.resolve(opt('--out', path.join(os.tmpdir(), `wpt-gate-${process.pid}.json`)));
  const jobs = Number(opt('--jobs', os.availableParallelism?.() ?? os.cpus().length));
  const common = ['--wpt', wpt, '--bin', `gate=${path.resolve(bin)}`];
  if (baseline) common.push('--baseline', BASELINE_FILE);
  for (const k of ['--dirs', '--match', '--timeout', '--long-timeout', '--sample-encoding']) if (opt(k)) common.push(k, opt(k));
  if (flag('--reuse-context')) common.push('--reuse-context');
  const first = ['--out', out, '--jobs', String(jobs), ...common, ...(full ? [] : ['--gate'])];
  if (opt('--shard')) first.push('--shard', opt('--shard'));
  const flaky = readFlaky(FLAKY_FILE);
  if (opt('--retest')) {
    if (!baseline) throw new Error('--retest needs a baseline');
    const prev = readJson(opt('--retest'));
    const urls = prev.results.filter((t) => differs(compareTest(baseline.tests[t.url], t, flaky))).map((t) => t.url);
    console.error(`retest: ${urls.length} page(s) in ${opt('--retest')} differ from the baseline`);
    const list = out + '.tests.json';
    writeJson(list, urls);
    first.push('--tests', list);
  }
  runRunner(first);
  const res = readJson(out);
  // One rerun of every page whose result differs from the baseline (regressions and improvements alike), with the
  // normal timeouts and at most one page per job, so a page that lost a race on a loaded machine gets a second try
  // and a pass that happened once does not enter the baseline.
  const again = baseline && !flag('--no-rerun') ? res.results.filter((t) => differs(compareTest(baseline.tests[t.url], t, flaky))) : [];
  if (again.length) {
    console.error(`rerunning ${again.length} page(s) that differ from the baseline`);
    const list = out + '.rerun.json';
    writeJson(list, again.map((t) => t.url));
    const rerunOut = out + '.rerun-results.json';
    runRunner(['--out', rerunOut, '--tests', list, '--jobs', String(Math.min(jobs, again.length)), ...common]);
    res.reruns = readJson(rerunOut).results;
    writeJson(out, res);
  }
  console.error(`results: ${out}`);
  if (!flag('--no-check')) check([out]);
}

// ---------- merging shard results ----------
// Every result file of one gate run (its shards), validated: the shards are 0..N-1 of one N, the WPT revision is
// the same everywhere, and every planned page has a result.
function load(files) {
  if (!files.length) throw new Error('no result files given');
  const runs = files.map(readJson);
  const problems = [];
  const parsed = runs.map((r) => (r.shard ?? '0/1').split('/').map(Number));
  const n = parsed[0][1];
  const indexes = new Set(parsed.map(([i, m]) => (m === n ? i : NaN)));
  if (runs.length !== n || indexes.size !== n || ![...indexes].every((i) => i >= 0 && i < n)) {
    problems.push(`shards: expected ${Array.from({ length: n }, (_, i) => `${i}/${n}`).join(', ')}; got ${runs.map((r) => r.shard).join(', ')}`);
  }
  const revisions = [...new Set(runs.map((r) => r.wptRevision ?? null))];
  if (revisions.length > 1) problems.push(`shards ran different WPT revisions: ${revisions.join(', ')}`);
  const pages = new Map();
  for (const r of runs) for (const t of r.results) pages.set(t.url, { first: t });
  for (const r of runs) for (const t of r.reruns ?? []) if (pages.has(t.url)) pages.get(t.url).again = t;
  const planned = new Set(runs.flatMap((r) => r.plan ?? r.results.map((t) => t.url)));
  const missing = [...planned].filter((u) => !pages.has(u));
  if (missing.length) problems.push(`${missing.length} planned page(s) have no result: ${missing.slice(0, 5).join(', ')}`);
  const skipped = new Set(runs.flatMap((r) => r.skipped ?? []));
  return {
    runs, problems, pages, planned, skipped, revision: revisions[0],
    full: runs.every((r) => r.mode === 'full'), restricted: runs.some((r) => r.restricted),
  };
}

// The baseline these runs are checked against, or null when there is none for their WPT revision (bootstrap).
function baselineFor(revision) {
  const b = readBaseline();
  if (!b) return { baseline: null, why: `there is no ${rel(BASELINE_FILE)} yet` };
  if (revision && b.wpt.revision !== revision) return { baseline: null, why: `the baseline is for WPT ${b.wpt.revision.slice(0, 12)} and this run used ${revision.slice(0, 12)}` };
  return { baseline: b };
}

// ---------- check ----------
function check(files) {
  const L = load(files);
  const { baseline, why } = baselineFor(L.revision);
  const flaky = readFlaky(FLAKY_FILE);
  const problems = [...L.problems];
  if (baseline && !L.restricted) {
    const notRun = Object.keys(baseline.tests).filter((u) => !L.planned.has(u) && !L.skipped.has(u));
    if (notRun.length) problems.push(`${notRun.length} baseline page(s) were not discovered by any shard (a WPT file missing from the checkout, or a discovery change; \`update --prune\` drops them on purpose): ${notRun.slice(0, 5).join(', ')}`);
  }
  const cmp = [...L.pages.values()].map(({ first, again }) => ({ ...combine(baseline?.tests[first.url], first, again, flaky), t: again ?? first }));
  const regressions = baseline ? cmp.filter(isRegression) : [];
  const improvements = baseline ? cmp.filter((c) => gainedCount(c) > 0) : [];
  const reruns = cmp.filter((c) => c.rerun);
  const lost = regressions.reduce((a, c) => a + lostCount(c), 0);
  const gained = improvements.reduce((a, c) => a + gainedCount(c), 0);
  const nowPass = cmp.reduce((a, c) => a + passing(c.t).length, 0);
  const before = baseline ? Object.values(baseline.tests).reduce((a, e) => a + passCount(e), 0) : null;
  const sum = (k) => L.runs.reduce((a, r) => a + (r[k] ?? 0), 0);

  const verdict = !baseline ? 'bootstrap' : regressions.length || problems.length ? 'FAIL' : 'pass';
  let md = `## WPT coverage gate: ${verdict}\n\n`;
  if (!baseline) md += `No gate: ${why}. This run only reports; the \`wpt-baseline\` artifact of a full run is the baseline to commit (bench/wpt/README.md).\n\n`;
  md += `| | pages | passing subtests |\n|---|---:|---:|\n`;
  if (baseline) md += `| baseline (WPT ${baseline.wpt.revision.slice(0, 12)}) | ${Object.keys(baseline.tests).length} | ${before} |\n`;
  const skippedNote = L.skipped.size ? `, ${L.skipped.size} skipped (nothing to protect)` : '';
  md += `| this run | ${L.pages.size} run${skippedNote} | ${baseline ? `${before - lost + gained} (−${lost} +${gained})` : nowPass} |\n\n`;
  const walls = L.runs.map((r) => `${(r.wallMs / 1000).toFixed(0)} s`).join(' / ');
  md += `${L.full ? 'Every page' : 'Gate plan'}, ${L.runs.length} shard(s): wall ${walls}; CPU: browser ${sum('browserCpuS').toFixed(0)} s, runner ${sum('runnerCpuS').toFixed(0)} s; browser restarts ${sum('restarts')}.\n\n`;
  for (const p of problems) md += `- **${esc(p)}**\n`;
  if (regressions.length) {
    md += `\n### Regressions (${regressions.length} pages, −${lost} subtests)\n\nFailed in the run and in its rerun. A page that is flaky for a known reason goes in bench/wpt/flaky.json, with the reason.\n\n| page | status | lost subtests |\n|---|---|---|\n`;
    regressions.sort((a, b) => b.broke - a.broke || lostCount(b) - lostCount(a));
    for (const c of regressions.slice(0, 50)) {
      const why = c.lost.slice(0, 5).map((n) => { const s = c.t.subtests.find((x) => x.name === n); return `${esc(n)} → ${s ? `${s.status}${s.msg ? `: ${esc(s.msg.slice(0, 100))}` : ''}` : 'not reported'}`; });
      if (c.lostUnnamed) why.push(`${c.lostUnnamed} more no longer reported`);
      md += `| ${esc(c.url)} | ${c.was} → ${c.status}${c.t.error ? ` (${esc(c.t.error)})` : ''} | ${lostCount(c)}${why.length ? `: ${why.join('<br>')}` : ''}${c.lost.length > 5 ? '<br>…' : ''} |\n`;
    }
    if (regressions.length > 50) md += `\n…and ${regressions.length - 50} more pages.\n`;
  }
  if (reruns.length) {
    const recovered = reruns.filter((c) => c.rerun.firstRegression && !isRegression(c));
    md += `\nRerun once: ${reruns.length} page(s) whose first result differed from the baseline; ${recovered.length} regression(s) did not repeat${recovered.length ? ` (${recovered.slice(0, 10).map((c) => esc(c.url)).join(', ')})` : ''}.\n`;
  }
  if (improvements.length) {
    const how = RUN_URL ? `\`gh run download ${process.env.GITHUB_RUN_ID} -n wpt-baseline -D bench/wpt\`` : '`bun bench/wpt/gate.mjs update <results>`';
    md += `\n### Improvements (${improvements.length} pages, +${gained} subtests)\n\nThe baseline is stale: ${how}, then commit bench/wpt/baseline.json in this pull request.\n\n`;
    for (const c of improvements.sort((a, b) => gainedCount(b) - gainedCount(a)).slice(0, 30)) md += `- ${esc(c.url)}: +${gainedCount(c)}${c.isNew ? ' (new page)' : ''}\n`;
  }
  const slow = cmp.map((c) => c.t).sort((a, b) => b.ms - a.ms).slice(0, 10);
  md += `\n<details><summary>Slowest pages</summary>\n\n| page | status | ms |\n|---|---|---:|\n${slow.map((t) => `| ${esc(t.url)} | ${t.status} | ${t.ms} |`).join('\n')}\n\n</details>\n`;
  console.log(md);
  if (opt('--summary')) fs.appendFileSync(opt('--summary'), md + '\n');
  if (GH) {
    for (const p of problems) console.log(`::error title=WPT gate::${ghEsc(p)}`);
    for (const c of regressions.slice(0, 20)) console.log(`::error title=WPT regression::${ghEsc(`${c.url}: ${lostCount(c)} subtest(s) no longer pass${c.broke ? `; page ${c.was} → ${c.status}` : ''}`)}`);
    if (improvements.length) console.log(`::warning title=WPT baseline is stale::${ghEsc(`${improvements.length} page(s) pass ${gained} more subtests; commit the refreshed bench/wpt/baseline.json`)}`);
    if (!baseline) console.log(`::warning title=WPT gate bootstrap::${ghEsc(`No gate: ${why}.`)}`);
  }
  if (problems.length || regressions.length) process.exitCode = 1;
}
const esc = (s) => String(s).replace(/\|/g, '\\|').replace(/</g, '&lt;').replace(/\n/g, ' ');
const ghEsc = (s) => String(s).replace(/%/g, '%25').replace(/\r/g, '%0D').replace(/\n/g, '%0A');

// ---------- update ----------
function update(files) {
  const L = load(files);
  if (L.problems.length) throw new Error(`not updating the baseline from incomplete results:\n${L.problems.join('\n')}`);
  let { baseline } = baselineFor(L.revision);
  const flaky = readFlaky(FLAKY_FILE);
  if (!baseline) {
    if (!L.full || L.restricted) throw new Error('a new baseline needs full runs over every page (gate.mjs run --full, no --dirs/--match)');
    if (!L.revision) throw new Error('a new baseline needs the WPT checkout\'s git revision');
    baseline = { wpt: { repo: WPT_REPO, revision: L.revision }, tests: {} };
  }
  const tests = { ...baseline.tests };
  // Gate runs keep durations (they skip pages and bound timeouts); full runs take the ones that moved.
  const ms = flag('--durations') ? 'all' : L.full ? 'moved' : 'keep';
  let gained = 0, lost = 0, changed = 0;
  for (const { first, again } of L.pages.values()) {
    const old = tests[first.url];
    let e;
    if (flag('--replace')) {
      e = entryFromResult(again ?? first);
      if (old) e.ms = newMs(old.ms, e.ms, ms);
    } else e = mergeEntry(old, combine(old, first, again, flaky).result, { ms });
    const d = delta(old, e);
    gained += d.gained; lost += d.lost;
    if (JSON.stringify(old) !== JSON.stringify(e)) { changed++; tests[first.url] = e; }
  }
  if (flag('--prune')) {
    if (!L.full || L.restricted) throw new Error('--prune needs full runs over every page');
    for (const u of Object.keys(tests)) if (!L.planned.has(u)) { lost += passCount(tests[u]); delete tests[u]; changed++; }
  }
  // `source` names the CI run that last changed the file (a local change drops it rather than keep a stale one); a
  // run that changes nothing writes the file as it was, so CI can tell that nothing changed.
  const head = { wpt: { repo: WPT_REPO, revision: baseline.wpt.revision }, dirs: changed ? L.runs[0].dirs : baseline.dirs ?? L.runs[0].dirs };
  if (!changed && baseline.source) head.source = baseline.source;
  else if (changed && RUN_URL) head.source = { run: RUN_URL, commit: process.env.GITHUB_SHA };
  const outFile = path.resolve(opt('--out', BASELINE_FILE));
  writeBaseline(outFile, head, tests);
  const total = Object.values(tests).reduce((a, e) => a + passCount(e), 0);
  console.log(`baseline ${rel(outFile)}: ${changed} page entr${changed === 1 ? 'y' : 'ies'} changed, +${gained} −${lost} passing subtests; ${Object.keys(tests).length} pages, ${total} passing subtests`);
}

// Passing subtests added and removed between two entries of one page (by name where both name them).
function delta(old, e) {
  if (!old) return { gained: passCount(e), lost: 0 };
  if (old.pass && e.pass) {
    const a = new Set(old.pass), b = new Set(e.pass);
    return { gained: e.pass.filter((n) => !a.has(n)).length, lost: old.pass.filter((n) => !b.has(n)).length };
  }
  const d = passCount(e) - passCount(old);
  return { gained: Math.max(0, d), lost: Math.max(0, -d) };
}

// One page per line, sorted, so a baseline change reads as a small diff.
function writeBaseline(file, head, tests) {
  const urls = Object.keys(tests).sort();
  let s = JSON.stringify(head, null, 1).replace(/\n}$/, ',\n "tests": {\n');
  s += urls.map((u) => `  ${JSON.stringify(u)}: ${JSON.stringify(tests[u])}`).join(',\n');
  s += '\n }\n}\n';
  fs.writeFileSync(file, s);
}

// ---------- diff ----------
function diff([a, b]) {
  if (!a || !b) throw new Error('usage: gate.mjs diff A.json[,A2.json...] B.json[,B2.json...]');
  const side = (list) => {
    const m = new Map();
    for (const f of list.split(',')) for (const t of readJson(f).results) m.set(t.url, t);
    return m;
  };
  const A = side(a), B = side(b);
  let lostT = 0, gainedT = 0, pages = 0;
  for (const u of [...new Set([...A.keys(), ...B.keys()])].sort()) {
    const x = A.get(u), y = B.get(u);
    const px = new Set(x ? passing(x) : []), py = new Set(y ? passing(y) : []);
    const lost = [...px].filter((n) => !py.has(n)), gained = [...py].filter((n) => !px.has(n));
    if (!lost.length && !gained.length && x?.status === y?.status) continue;
    pages++; lostT += lost.length; gainedT += gained.length;
    console.log(`${u}: ${x?.status ?? 'absent'} ${px.size} → ${y?.status ?? 'absent'} ${py.size}${lost.length ? ` −${lost.length} (${lost[0].slice(0, 80)}…)` : ''}${gained.length ? ` +${gained.length}` : ''}`);
  }
  const common = [...A.keys()].filter((u) => B.has(u)).length;
  console.log(`${pages} page(s) differ; ${common} in both, ${A.size - common} only in A, ${B.size - common} only in B; subtests passing only in A: ${lostT}, only in B: ${gainedT}`);
}

try {
  if (cmd === 'run') run();
  else if (cmd === 'check') check(positional());
  else if (cmd === 'update') update(positional());
  else if (cmd === 'diff') diff(positional());
  else { console.error('usage: gate.mjs run|check|update|diff (see the header of bench/wpt/gate.mjs)'); process.exitCode = 2; }
} catch (e) {
  console.error(`gate.mjs ${cmd}: ${e.message}`);
  process.exitCode = 2;
}
