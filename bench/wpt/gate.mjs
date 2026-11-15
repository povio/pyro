// WPT coverage gate: a subtest that passes in baseline.json must still pass, and a page that completed must still
// complete. See bench/wpt/README.md. With --kind reftest (every command), the same gate over the paint reftests:
// run.mjs --kind reftest reports each reftest as one subtest ("reftest"), the baseline is reftest-baseline.json, the
// flaky list reftest-flaky.json and the CI artifact wpt-reftest-baseline.
//
//   bun bench/wpt/gate.mjs run --wpt DIR --bin PATH [--out FILE] [--jobs N] [--shard I/N] [--full]
//                              [--dirs a,b] [--match RE] [--retest PREVIOUS.json] [--no-rerun] [--no-check]
//                              [--summary FILE] [--reuse-context] [--timeout MS] [--long-timeout MS]
//       Runs the gate's plan (every page with --full, or when there is no baseline for this WPT revision yet), then
//       reruns every page whose result differs from the baseline at least twice more, at the same --jobs (with no
//       baseline: every page once more), then checks the result.
//       --retest PREVIOUS.json runs only the pages whose result in that file differs from the baseline.
//   bun bench/wpt/gate.mjs check FILE... [--summary FILE] [--base-baseline FILE [--base-flaky FILE]] [--accept-loss]
//       Merges shard results and compares them with the baseline. Exit 1 on a regression, a missing shard, a
//       planned page without a result, a baseline page that no shard planned, improvements the baseline does not
//       record yet ("stale"), or no baseline for this WPT revision while the base branch has one ("rebaseline").
//       With no baseline anywhere it only reports (bootstrap). --base-baseline/--base-flaky: the base branch's
//       files; coverage this branch's files drop is an error unless --accept-loss (the `wpt-accept-loss` label).
//   bun bench/wpt/gate.mjs update FILE... [--out FILE] [--replace --match RE|--tests FILE] [--prune] [--durations]
//       Writes the results into the baseline, only adding: a subtest passes if it passed in the baseline or in every
//       run of the page; a page whose runs are a regression keeps its entry. --replace takes the pages that --match
//       or --tests name as they are (accepting their losses); --prune drops pages no shard discovered. Durations
//       change on full runs when they moved by 2x and 1 s, never on gate runs; --durations refreshes all of them.
//       Without a baseline (or for a new WPT revision) it starts one from full runs that ran every page twice.
//   bun bench/wpt/gate.mjs diff A.json[,A2.json...] B.json[,B2.json...] [C.json...] [--suggest]
//       Pages whose status or passing subtests differ between sets of results (two binaries, two CI runs, the
//       runner with and without --reuse-context). With --suggest, prints flaky.json entries for them.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  MAX_IGNORED_INCOMPLETE, consensus, differs, entryOf, formatBaseline, isImprovement, isRegression, judge, lostCount,
  mergeEntry, newMs, passCount, passing, readFlaky, readJson, roundMs, skippable, weakening, wholePage,
} from './lib.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const [cmd, ...argv] = process.argv.slice(2);
const FLAGS = ['--full', '--no-rerun', '--no-check', '--replace', '--prune', '--durations', '--reuse-context', '--accept-loss', '--suggest'];
const opt = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const flag = (k) => argv.includes(k);
const positional = () => argv.filter((a, i) => !a.startsWith('--') && !(argv[i - 1]?.startsWith('--') && !FLAGS.includes(argv[i - 1])));
const KIND = opt('--kind', 'testharness');
if (KIND !== 'testharness' && KIND !== 'reftest') { console.error(`gate.mjs: --kind ${KIND}: expected testharness or reftest`); process.exit(2); }
const SUITE = KIND === 'reftest'
  ? { title: 'WPT reftest gate', baseline: 'reftest-baseline.json', flaky: 'reftest-flaky.json', artifact: 'wpt-reftest-baseline' }
  : { title: 'WPT coverage gate', baseline: 'baseline.json', flaky: 'flaky.json', artifact: 'wpt-baseline' };
const BASELINE_FILE = path.resolve(opt('--baseline', path.join(HERE, SUITE.baseline)));
const FLAKY_FILE = path.resolve(opt('--flaky', path.join(HERE, SUITE.flaky)));
const B = path.basename(BASELINE_FILE), F = path.basename(FLAKY_FILE);
// A reftest's coverage is its pass: whether a failing one completes is not checked (lib.mjs judge()).
const JUDGE = KIND === 'reftest' ? { completion: false } : undefined;
const WPT_REPO = 'https://github.com/web-platform-tests/wpt';
const LABEL = 'wpt-accept-loss';
// Reruns: each page whose first result differs runs at least RERUNS more times, more when few pages differ, so the
// rerun keeps the shard's --jobs busy (a timing-sensitive page then runs under about the load it first ran under).
// Past PAD_MAX differing pages (a broken build, not a flake) each runs once more.
const RERUNS = 2, PAD_MAX = 100;
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
  const bin = opt('--bin', process.env.PYRO_BIN);
  if (!wpt || !bin) throw new Error('run needs --wpt DIR (or WPT_DIR) and --bin PATH (or PYRO_BIN)');
  let baseline = readBaseline();
  const rev = gitRevision(wpt);
  if (baseline && rev && rev !== baseline.wpt.revision) {
    console.error(`warning: ${wpt} is at ${rev}; the baseline is for ${baseline.wpt.revision}. Running every page twice (bootstrap).`);
    baseline = null;
  }
  const full = flag('--full') || !baseline;
  const out = path.resolve(opt('--out', path.join(os.tmpdir(), `wpt-gate-${process.pid}.json`)));
  const jobs = Number(opt('--jobs', os.availableParallelism?.() ?? os.cpus().length));
  const common = ['--kind', KIND, '--wpt', wpt, '--bin', `gate=${path.resolve(bin)}`];
  if (baseline) common.push('--baseline', BASELINE_FILE);
  for (const k of ['--dirs', '--match', '--timeout', '--long-timeout', '--sample-encoding']) if (opt(k)) common.push(k, opt(k));
  if (flag('--reuse-context')) common.push('--reuse-context');
  const first = ['--out', out, '--jobs', String(jobs), ...common, ...(full ? [] : ['--gate'])];
  if (opt('--shard')) first.push('--shard', opt('--shard'));
  const flaky = readFlaky(FLAKY_FILE);
  if (opt('--retest')) {
    if (!baseline) throw new Error('--retest needs a baseline');
    const prev = readJson(opt('--retest'));
    const urls = prev.results.filter((t) => differs(judge(baseline.tests[t.url], [t], flaky, JUDGE))).map((t) => t.url);
    console.error(`retest: ${urls.length} page(s) in ${opt('--retest')} differ from the baseline`);
    const list = out + '.tests.json';
    writeJson(list, urls);
    first.push('--tests', list);
  }
  runRunner(first);
  const res = readJson(out);
  // Reruns, with the normal timeouts: every page whose result differs from the baseline (regressions and
  // improvements alike), so a page that lost a race gets more tries and a pass that happened once does not enter
  // the baseline. Without a baseline every page runs twice, so a new baseline holds only what passed both times.
  let again = [], copies = 1;
  if (!flag('--no-rerun')) {
    if (!baseline) again = res.results.map((t) => t.url);
    else {
      again = res.results.filter((t) => differs(judge(baseline.tests[t.url], [t], flaky, JUDGE))).map((t) => t.url);
      copies = again.length > PAD_MAX ? 1 : Math.max(RERUNS, Math.ceil(jobs / again.length));
    }
  }
  if (again.length) {
    console.error(`rerunning ${again.length} page(s) ${baseline ? 'that differ from the baseline' : '(no baseline: every page runs twice)'}, ${copies} time(s) each at --jobs ${jobs}`);
    const list = out + '.rerun.json';
    writeJson(list, again);
    const rerunOut = out + '.rerun-results.json';
    runRunner(['--out', rerunOut, '--tests', list, '--repeat', String(copies), '--jobs', String(jobs), ...common]);
    res.reruns = readJson(rerunOut).results;
    res.rerunCopies = copies;
    writeJson(out, res);
  }
  console.error(`results: ${out}`);
  if (!flag('--no-check')) check([out]);
}

// ---------- merging shard results ----------
// Every result file of one gate run (its shards), validated: the shards are 0..N-1 of one N, the WPT revision is
// the same everywhere, and every planned page has a result. pages: url -> { first, again: [reruns] }.
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
  const kinds = [...new Set(runs.map((r) => r.kind ?? 'testharness'))];
  if (kinds.length > 1 || kinds[0] !== KIND) problems.push(`results of kind ${kinds.join(', ')} given to the ${KIND} gate (--kind)`);
  const revisions = [...new Set(runs.map((r) => r.wptRevision ?? null))];
  if (revisions.length > 1) problems.push(`shards ran different WPT revisions: ${revisions.join(', ')}`);
  const pages = new Map();
  for (const r of runs) for (const t of r.results) pages.set(t.url, { first: t, again: [] });
  for (const r of runs) for (const t of r.reruns ?? []) pages.get(t.url)?.again.push(t);
  const planned = new Set(runs.flatMap((r) => r.plan ?? r.results.map((t) => t.url)));
  const missing = [...planned].filter((u) => !pages.has(u));
  if (missing.length) problems.push(`${missing.length} planned page(s) have no result: ${missing.slice(0, 5).join(', ')}`);
  const skipped = new Set(runs.flatMap((r) => r.skipped ?? []));
  return {
    runs, problems, pages, planned, skipped, revision: revisions[0],
    full: runs.every((r) => r.mode === 'full'), restricted: runs.some((r) => r.restricted),
  };
}

// The baseline these runs are checked against, or null when there is none for their WPT revision.
function baselineFor(revision) {
  const b = readBaseline();
  if (!b) return { baseline: null, why: `there is no ${rel(BASELINE_FILE)}` };
  if (revision && b.wpt.revision !== revision) return { baseline: null, why: `${rel(BASELINE_FILE)} is for WPT ${b.wpt.revision.slice(0, 12)} and this run used ${revision.slice(0, 12)}` };
  return { baseline: b };
}

// ---------- check ----------
function check(files) {
  const L = load(files);
  const { baseline, why } = baselineFor(L.revision);
  const flaky = readFlaky(FLAKY_FILE);
  const base = opt('--base-baseline') ? readJson(opt('--base-baseline')) : null;
  const problems = [...L.problems];
  if (baseline && !L.restricted) {
    // The plan's skip rule, applied here again: a shard may skip only what the rule skips.
    const notRun = Object.keys(baseline.tests).filter((u) => !L.planned.has(u) && (L.full || !skippable(baseline.tests[u])));
    if (notRun.length) problems.push(`${notRun.length} baseline page(s) were not run by any shard (a WPT file missing from the checkout, or a discovery change; \`update --prune\` drops them on purpose): ${notRun.slice(0, 5).join(', ')}`);
    const badSkip = [...L.skipped].filter((u) => !baseline.tests[u] || !skippable(baseline.tests[u]));
    if (badSkip.length) problems.push(`${badSkip.length} page(s) were skipped that have coverage to protect: ${badSkip.slice(0, 5).join(', ')}`);
  }
  const cmp = [...L.pages.values()].map(({ first, again }) => {
    const runs = [first, ...again];
    return { ...judge(baseline?.tests[first.url], runs, flaky, JUDGE), runList: runs, t: runs[runs.length - 1], b: baseline?.tests[first.url] };
  });
  const regressions = baseline ? cmp.filter(isRegression) : [];
  const improvements = baseline ? cmp.filter(isImprovement) : [];
  const unstable = cmp.filter((c) => c.unstable);
  const ignoredIncomplete = cmp.filter((c) => c.ignoredIncomplete);
  if (ignoredIncomplete.length > MAX_IGNORED_INCOMPLETE) {
    problems.push(`${ignoredIncomplete.length} pages ${F} allows not to complete did not complete in any run (more than ${MAX_IGNORED_INCOMPLETE} is not a flake): ${ignoredIncomplete.slice(0, 5).map((c) => c.url).join(', ')}`);
  }
  // Coverage the base branch protects that this branch's baseline or flaky list no longer does.
  const weakened = base && baseline ? weakening(base, baseline, readFlaky(opt('--base-flaky')), flaky) : [];
  const accepted = flag('--accept-loss');
  if (weakened.length && !accepted) problems.push(`this branch's ${B} or ${F} drops coverage the base branch's protects (${weakened.length} page(s), listed below): label the pull request \`${LABEL}\` to accept that`);
  const reruns = cmp.filter((c) => c.runs > 1);
  const lost = regressions.reduce((a, c) => a + lostCount(c), 0);
  const gained = improvements.reduce((a, c) => a + c.gained.length, 0);
  const before = baseline ? Object.values(baseline.tests).reduce((a, e) => a + passCount(e), 0) : null;
  const sum = (k) => L.runs.reduce((a, r) => a + (r[k] ?? 0), 0);

  const verdict = problems.length || regressions.length ? 'fail' : !baseline ? (base ? 'rebaseline' : 'bootstrap') : improvements.length ? 'stale' : 'pass';
  const download = RUN_URL ? `\`gh run download ${process.env.GITHUB_RUN_ID} -n ${SUITE.artifact} -D bench/wpt\`` : '`bun bench/wpt/gate.mjs update <results>`';
  let md = `## ${SUITE.title}: ${{ fail: 'FAIL', rebaseline: 'FAIL (no baseline for this WPT revision)', bootstrap: 'bootstrap', stale: 'FAIL (baseline is stale)', pass: 'pass' }[verdict]}\n\n`;
  if (verdict === 'bootstrap') md += `No gate: ${why}. This run only reports; its \`${SUITE.artifact}\` artifact is the baseline to commit (${download}).\n\n`;
  if (verdict === 'rebaseline') md += `No gate: ${why}, but the base branch has a baseline (a WPT revision change, or a removed baseline). Every page ran twice; commit this run's baseline: ${download}. The next run then checks it against the base branch's (label \`${LABEL}\` to accept the passes it drops).\n\n`;
  md += `| | pages | passing subtests |\n|---|---:|---:|\n`;
  if (baseline) md += `| baseline (WPT ${baseline.wpt.revision.slice(0, 12)}) | ${Object.keys(baseline.tests).length} | ${before} |\n`;
  const skippedNote = L.skipped.size ? `, ${L.skipped.size} skipped (nothing to protect)` : '';
  const nowPass = cmp.reduce((a, c) => a + (c.runs > 1 ? consensus(c.runList).pass.length : passing(c.t).length), 0);
  md += `| this run | ${L.pages.size} run${skippedNote} | ${baseline ? `${before - lost + gained} (−${lost} +${gained})` : nowPass} |\n\n`;
  const walls = L.runs.map((r) => `${(r.wallMs / 1000).toFixed(0)} s`).join(' / ');
  md += `${L.full ? 'Every page' : 'Gate plan'}, ${L.runs.length} shard(s): wall ${walls}; CPU: browser ${sum('browserCpuS').toFixed(0)} s, runner ${sum('runnerCpuS').toFixed(0)} s; browser restarts ${sum('restarts')}.\n\n`;
  for (const p of problems) md += `- **${esc(p)}**\n`;
  if (weakened.length) {
    md += `\n### Coverage this branch stops protecting (${weakened.length})\n\nCompared with the base branch's bench/wpt/${B} and ${F}. ${accepted ? `Accepted: the pull request has the \`${LABEL}\` label.` : `An error unless the pull request has the \`${LABEL}\` label.`}\n\n`;
    for (const w of weakened.slice(0, 40)) md += `- ${esc(w.url)}: ${esc(w.what)}\n`;
    if (weakened.length > 40) md += `- …and ${weakened.length - 40} more\n`;
  }
  if (regressions.length) {
    md += `\n### Regressions (${regressions.length} pages, −${lost} subtests)\n\nLost in every run of the page (each regression ran ${RERUNS} or more extra times). A page that is flaky for a known reason goes in bench/wpt/${F}, with the reason.\n\n| page | status | lost subtests |\n|---|---|---|\n`;
    regressions.sort((a, b) => b.broke - a.broke || b.setChanged - a.setChanged || lostCount(b) - lostCount(a));
    for (const c of regressions.slice(0, 50)) {
      const why = c.lost.slice(0, 5).map((n) => `${esc(n)} → ${statusIn(c.runList, n)}`);
      if (c.lostUnnamed) why.push(`${c.lostUnnamed} more no longer reported`);
      if (c.setChanged) why.unshift(`**subtest names changed** (the baseline has ${c.b.n}; this run reported ${new Set(c.runList.flatMap((t) => (t.subtests ?? []).map((s) => s.name))).size}), so which passes are gone is unknown${c.lost.length ? '; not passing, lost or new:' : ''}`);
      const err = c.runList.find((t) => t.error)?.error;
      md += `| ${esc(c.url)} | ${c.was} → ${c.runList.map((t) => t.status).join(', ')}${err ? ` (${esc(err)})` : ''} | ${lostCount(c)}${why.length ? `: ${why.join('<br>')}` : ''}${c.lost.length > 5 ? '<br>…' : ''} |\n`;
    }
    if (regressions.length > 50) md += `\n…and ${regressions.length - 50} more pages.\n`;
    if (regressions.some((c) => c.setChanged)) md += `\nA page whose subtest names changed on purpose: \`gh run download ${process.env.GITHUB_RUN_ID ?? '<run>'} -p 'wpt-shard-*' -D /tmp/wpt-run\`, then \`bun bench/wpt/gate.mjs update /tmp/wpt-run/*/*.json --replace --match '<page regex>'\`, commit, and label the pull request \`${LABEL}\`.\n`;
  }
  if (reruns.length && baseline) {
    const recovered = reruns.filter((c) => !isRegression(c) && isRegression(judge(c.b, [c.runList[0]], flaky, JUDGE)));
    md += `\nReran ${reruns.length} page(s) whose first result differed from the baseline (${Math.max(...L.runs.map((r) => r.rerunCopies ?? 0))}x at most); ${recovered.length} first-run regression(s) did not repeat${recovered.length ? ` (${recovered.slice(0, 10).map((c) => esc(c.url)).join(', ')})` : ''}.\n`;
  }
  if (unstable.length) {
    md += `\n<details><summary>${unstable.length} page(s) whose runs disagreed (${F} candidates)</summary>\n\n`;
    for (const c of unstable.slice(0, 40)) md += `- ${esc(c.url)}: ${c.runList.map((t) => `${t.status} ${passing(t).length}`).join(' / ')}\n`;
    md += `\n</details>\n`;
  }
  if (improvements.length) {
    md += `\n### Improvements (${improvements.length} pages, +${gained} subtests${improvements.some((c) => c.fixed) ? ', or now completing' : ''})\n\n`;
    md += verdict === 'stale' ? `The baseline is stale: ${download}, then commit bench/wpt/${B} in this pull request.\n\n` : `Fix what fails first: no refreshed baseline is uploaded while the gate fails.\n\n`;
    for (const c of improvements.sort((a, b) => b.gained.length - a.gained.length).slice(0, 30)) md += `- ${esc(c.url)}: +${c.gained.length}${c.fixed ? ` (${c.was} → complete)` : ''}${c.isNew ? ' (new page)' : ''}\n`;
  }
  const slow = cmp.map((c) => c.runList[0]).sort((a, b) => (b.ms ?? 0) - (a.ms ?? 0)).slice(0, 10);
  md += `\n<details><summary>Slowest pages</summary>\n\n| page | status | ms |\n|---|---|---:|\n${slow.map((t) => `| ${esc(t.url)} | ${t.status} | ${t.ms} |`).join('\n')}\n\n</details>\n`;
  console.log(md);
  if (opt('--summary')) fs.appendFileSync(opt('--summary'), md + '\n');
  if (process.env.GITHUB_OUTPUT) fs.appendFileSync(process.env.GITHUB_OUTPUT, `verdict=${verdict}\n`);
  if (GH) {
    for (const p of problems) console.log(`::error title=${SUITE.title}::${ghEsc(p)}`);
    for (const c of regressions.slice(0, 20)) console.log(`::error title=${KIND === 'reftest' ? 'WPT reftest regression' : 'WPT regression'}::${ghEsc(`${c.url}: ${c.setChanged ? 'subtest names changed; ' : ''}${lostCount(c)} subtest(s) no longer pass${c.broke ? `; page ${c.was} → ${c.status}` : ''}`)}`);
    if (verdict === 'stale') console.log(`::error title=${SUITE.title}: baseline is stale::${ghEsc(`${improvements.length} page(s) pass ${gained} more subtests (or now complete); commit the refreshed bench/wpt/${B}: ${download.replace(/`/g, '')}`)}`);
    if (verdict === 'rebaseline') console.log(`::error title=${SUITE.title}: baseline missing::${ghEsc(`${why}; commit this run's baseline: ${download.replace(/`/g, '')}`)}`);
    if (verdict === 'bootstrap') console.log(`::warning title=${SUITE.title}: bootstrap::${ghEsc(`No gate: ${why}.`)}`);
    if (weakened.length && accepted) console.log(`::warning title=WPT coverage accepted::${ghEsc(`${weakened.length} page(s) lose protected coverage (label ${LABEL})`)}`);
  }
  if (verdict === 'fail' || verdict === 'stale' || verdict === 'rebaseline') process.exitCode = 1;
}
// How a subtest ended in each run that reported it.
function statusIn(runs, n) {
  const seen = runs.map((t) => (t.subtests ?? []).find((s) => s.name === n)).filter(Boolean);
  if (!seen.length) return 'not reported';
  const s = seen.find((x) => x.status !== 'Pass') ?? seen[0];
  return `${s.status}${s.msg ? `: ${esc(s.msg.slice(0, 100))}` : ''}${seen.length < runs.length ? ` (${runs.length - seen.length} run(s) did not report it)` : ''}`;
}
const esc = (s) => String(s).replace(/\|/g, '\\|').replace(/</g, '&lt;').replace(/\n/g, ' ');
const ghEsc = (s) => String(s).replace(/%/g, '%25').replace(/\r/g, '%0D').replace(/\n/g, '%0A');

// ---------- update ----------
// --match RE / --tests FILE (a JSON list of URLs, or a results or baseline file) as a URL predicate, or null.
function pageFilter() {
  if (opt('--match')) { const re = new RegExp(opt('--match')); return (u) => re.test(u); }
  if (opt('--tests')) {
    const j = readJson(opt('--tests'));
    const s = new Set(Array.isArray(j) ? j : j.results ? j.results.map((t) => t.url) : Object.keys(j.tests ?? {}));
    return (u) => s.has(u);
  }
  return null;
}
function update(files) {
  const L = load(files);
  if (L.problems.length) throw new Error(`not updating the baseline from incomplete results:\n${L.problems.join('\n')}`);
  let { baseline } = baselineFor(L.revision);
  const flaky = readFlaky(FLAKY_FILE);
  if (!baseline) {
    if (!L.full || L.restricted) throw new Error('a new baseline needs full runs over every page (gate.mjs run --full, no --dirs/--match)');
    if (!L.revision) throw new Error('a new baseline needs the WPT checkout\'s git revision');
    const once = [...L.pages.values()].filter((p) => !p.again.length).length;
    if (once) throw new Error(`a new baseline needs every page run twice (gate.mjs run does that when there is no baseline, unless --no-rerun); ${once} page(s) ran once`);
    baseline = { wpt: { repo: WPT_REPO, revision: L.revision }, dirs: L.runs[0].dirs, tests: {} };
  }
  const replace = flag('--replace');
  const pick = pageFilter();
  if (replace && !pick) throw new Error('--replace accepts every loss in the pages it takes: name them with --match RE or --tests FILE');
  const tests = { ...baseline.tests };
  // Gate runs keep durations (they skip pages and bound timeouts); full runs take the ones that moved.
  const ms = flag('--durations') ? 'all' : L.full ? 'moved' : 'keep';
  let gained = 0, lost = 0, changed = 0, replaced = 0;
  const kept = [];
  for (const { first, again } of L.pages.values()) {
    const url = first.url, old = tests[url], runs = [first, ...again];
    const f = flaky.get(url);
    const con = consensus(runs);
    let e;
    if (replace && pick(url)) {
      e = entryOf(con.status, con.ms, con.all, con.pass, con.n);
      if (old) e.ms = newMs(old.ms, roundMs(con.ms ?? 0), ms);
      replaced++;
    } else if (old && wholePage(f)) continue;
    else if (old && isRegression(judge(old, runs, flaky, JUDGE))) { kept.push(url); continue; }
    else e = mergeEntry(old, con, { ms, ignore: f?.subtests && new Set(f.subtests) });
    const d = delta(old, e);
    gained += d.gained; lost += d.lost;
    if (JSON.stringify(old) !== JSON.stringify(e)) { changed++; tests[url] = e; }
  }
  // Pages no shard discovered (neither planned nor skipped by the plan).
  if (flag('--prune')) {
    if (L.restricted) throw new Error('--prune needs runs over every page (no --dirs/--match/--tests)');
    for (const u of Object.keys(tests)) if (!L.planned.has(u) && !L.skipped.has(u)) { lost += passCount(tests[u]); delete tests[u]; changed++; }
  }
  // A run that changes nothing writes the file as it was, so CI can tell that nothing changed.
  const head = { wpt: { repo: WPT_REPO, revision: baseline.wpt.revision }, dirs: baseline.dirs ?? L.runs[0].dirs };
  const outFile = path.resolve(opt('--out', BASELINE_FILE));
  writeBaseline(outFile, head, tests);
  const total = Object.values(tests).reduce((a, e) => a + passCount(e), 0);
  console.log(`baseline ${rel(outFile)}: ${changed} page entr${changed === 1 ? 'y' : 'ies'} changed${replaced ? ` (${replaced} replaced)` : ''}, +${gained} −${lost} passing subtests; ${Object.keys(tests).length} pages, ${total} passing subtests`);
  if (kept.length) console.log(`kept ${kept.length} page entr${kept.length === 1 ? 'y' : 'ies'} whose runs are a regression (accept with --replace --match): ${kept.slice(0, 10).join(', ')}`);
}

// Passing subtests added and removed between two entries of one page (by name where both name them).
function delta(old, e) {
  if (!old) return { gained: passCount(e), lost: 0 };
  const byName = (a, b) => { const s = new Set(b); return a.filter((n) => !s.has(n)).length; };
  if (old.pass && e.pass) return { gained: byName(e.pass, old.pass), lost: byName(old.pass, e.pass) };
  if (old.fail && e.fail && old.h === e.h) return { gained: byName(old.fail, e.fail), lost: byName(e.fail, old.fail) };
  const d = passCount(e) - passCount(old);
  return { gained: Math.max(0, d), lost: Math.max(0, -d) };
}

function writeBaseline(file, head, tests) {
  fs.writeFileSync(file, formatBaseline(head, tests));
}

// ---------- diff ----------
function diff(sets) {
  if (sets.length < 2) throw new Error('usage: gate.mjs diff A.json[,A2.json...] B.json[,B2.json...] [C.json...] [--suggest]');
  const sides = sets.map((list) => {
    const m = new Map();
    for (const f of list.split(',')) for (const t of readJson(f).results) m.set(t.url, t);
    return m;
  });
  const suggest = {};
  let pages = 0, onlyA = 0, onlyB = 0;
  for (const u of [...new Set(sides.flatMap((m) => [...m.keys()]))].sort()) {
    const ts = sides.map((m) => m.get(u));
    const ps = ts.map((t) => new Set(t ? passing(t) : []));
    const statuses = ts.map((t) => t?.status ?? 'absent');
    const varying = [...new Set(ps.flatMap((s) => [...s]))].filter((n) => !ps.every((s) => s.has(n))).sort();
    if (!varying.length && new Set(statuses).size === 1) continue;
    pages++;
    const counts = ts.map((t, i) => `${statuses[i]} ${ps[i].size}`).join(' → ');
    if (sides.length === 2) {
      const lost = [...ps[0]].filter((n) => !ps[1].has(n)), gained = [...ps[1]].filter((n) => !ps[0].has(n));
      onlyA += lost.length; onlyB += gained.length;
      console.log(`${u}: ${counts}${lost.length ? ` −${lost.length} (${lost[0].slice(0, 80)}…)` : ''}${gained.length ? ` +${gained.length}` : ''}`);
    } else console.log(`${u}: ${counts}${varying.length ? ` (${varying.length} subtest(s) vary: ${varying[0].slice(0, 80)}…)` : ''}`);
    if (flag('--suggest')) {
      const done = ts.filter((t) => t?.status === 'complete');
      const donePass = done.map((t) => passing(t).sort().join('\n'));
      if (new Set(statuses).size === 1) suggest[u] = { reason: `TODO: why; ${varying.length} subtest(s) pass in only some of ${sides.length} runs (gate.mjs diff)`, subtests: varying };
      else if (done.length && new Set(donePass).size === 1) suggest[u] = { reason: `TODO: why; does not always complete (${[...new Set(statuses)].join('/')} in ${sides.length} runs)`, allowIncomplete: true };
      else suggest[u] = { reason: `TODO: why; status and passes vary across ${sides.length} runs (${[...new Set(statuses)].join('/')})` };
    }
  }
  const common = [...sides[0].keys()].filter((u) => sides.every((m) => m.has(u))).length;
  console.log(`${pages} page(s) differ; ${common} in every set${sides.length === 2 ? `; subtests passing only in A: ${onlyA}, only in B: ${onlyB}` : ''}`);
  if (flag('--suggest')) console.log(`\n${F} entries (replace each TODO with the reason):\n${JSON.stringify(suggest, null, 1)}`);
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
