// `pyro run`: Playwright's `playwright test` for tests written against
// `@povio/pyro/wright`. It reads the config, finds each project's test files
// (`testDir`, `testMatch`, `testIgnore`) and adds the projects they depend
// on (`dependencies`, and the `teardown` projects of those), starts the web
// servers and runs global setup once, then runs the projects in Playwright's
// phases: a project starts once the projects it depends on finished, and
// does not run when one of them failed; a teardown project runs after the
// projects that depend on its setup project. A phase's files run in worker
// processes (`bun test`, one browser each): a file's tests stay in one worker
// unless they are parallel (`fullyParallel`, `mode: 'parallel'`), which
// spread over the workers, each with its own module state. `--shard i/n`
// runs part i of n of the tests, split as Playwright splits them. It reports
// from the results the workers record (skips as skips, retries as flaky).
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { availableParallelism, tmpdir } from 'node:os';
import { join, relative, resolve } from 'node:path';
import type { FilePlan, ResolvedProject, RunPlan } from './config.js';

type Args = {
  config?: string;
  projects: string[];
  workers?: string;
  grep?: string;
  grepInvert?: string;
  retries?: string;
  timeout?: string;
  update: boolean;
  /** `-u [mode]`: all, changed (the default), missing or none. */
  updateMode?: string;
  list: boolean;
  fullyParallel: boolean;
  reporter: 'list' | 'dot' | 'line';
  maxFailures?: number;
  quiet: boolean;
  shard?: string;
  noDeps: boolean;
  filters: string[];
};

const HELP = `usage: pyro run [options] [test-filter...]

Runs Playwright-style tests (written against @povio/pyro/wright)
with bun test, in worker processes, as \`playwright test\` does.

  -c, --config <file>     config file (default: pyro.config.* or playwright.config.*)
      --project <name>    only this project and the projects it depends on
                          (repeatable; any case, * matches any text)
      --no-deps           do not run the projects' dependencies
  -j, --workers <n|n%>    worker processes (default: the config's workers, else half the CPUs)
  -g, --grep <regex>      only tests whose title path matches
      --grep-invert <re>  skip tests whose title path matches
      --retries <n>       retries of a failing test
      --timeout <ms>      test timeout
  -u, --update-snapshots [mode]
                          rewrite snapshots and screenshot baselines: changed
                          (the default), all, missing or none
      --fully-parallel    run every file's tests in parallel
      --shard <i/n>       run part i of n of the tests (one part per CI job;
                          whole files, single tests when fully parallel)
  -x                      stop after the first failure (as --max-failures=1)
      --max-failures <n>  stop after n failures
      --reporter <name>   list (default), line or dot
      --list              list the tests without running them
  -q, --quiet             do not print the tests' own output

test-filter: a regular expression the test file's path must match.

A project's dependencies run first, to completion, and in full (no filter,
grep or shard applies to them); when one fails, the projects that depend on
it do not run. A teardown project runs after the projects that depend on
its setup project.
`;

function parse(argv: string[]): Args {
  const a: Args = { projects: [], update: false, list: false, fullyParallel: false, reporter: 'list', quiet: false, noDeps: false, filters: [] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const [flag, inline] = arg.startsWith('--') && arg.includes('=') ? [arg.slice(0, arg.indexOf('=')), arg.slice(arg.indexOf('=') + 1)] : [arg, undefined];
    const value = () => inline ?? argv[++i];
    switch (flag) {
      case '-h':
      case '--help':
        console.log(HELP);
        process.exit(0);
      case '-c':
      case '--config':
        a.config = value();
        break;
      case '--project':
        a.projects.push(value());
        break;
      case '--no-deps':
        a.noDeps = true;
        break;
      case '-j':
      case '--workers':
        a.workers = value();
        break;
      case '-g':
      case '--grep':
        a.grep = value();
        break;
      case '--grep-invert':
        a.grepInvert = value();
        break;
      case '--retries':
        a.retries = value();
        break;
      case '--timeout':
        a.timeout = value();
        break;
      case '-u':
      case '--update-snapshots': {
        a.update = true;
        const next = inline ?? argv[i + 1];
        if (['all', 'changed', 'missing', 'none'].includes(next ?? '')) {
          a.updateMode = next;
          if (inline === undefined) i++;
        }
        break;
      }
      case '--fully-parallel':
        a.fullyParallel = true;
        break;
      case '--shard':
        a.shard = value();
        break;
      case '-x':
        a.maxFailures = 1;
        break;
      case '--max-failures':
        a.maxFailures = Number(value());
        break;
      case '--reporter':
        a.reporter = value() as Args['reporter'];
        break;
      case '--list':
        a.list = true;
        break;
      case '-q':
      case '--quiet':
        a.quiet = true;
        break;
      case '--headed':
      case '--debug':
      case '--ui':
        console.error(`pyro run: ${flag} has no effect (pyro is headless)`);
        break;
      default:
        if (flag.startsWith('-')) {
          console.error(`pyro run: unknown option ${flag}\n\n${HELP}`);
          process.exit(2);
        }
        a.filters.push(arg);
    }
  }
  return a;
}

/** What a worker records per test (runner.ts); `kind: 'error'` is an error outside any test. */
type Outcome = {
  kind?: 'error';
  file: string;
  project: number;
  titlePath: string[];
  status: 'passed' | 'failed' | 'timedOut' | 'skipped';
  flaky?: boolean;
  expectedFailure?: boolean;
  didNotRun?: boolean;
  duration: number;
  retries: number;
  errors?: { message: string; stack?: string }[];
  skipReason?: string;
  parallel?: number;
  worker: number;
};
const isFailure = (o: Outcome) => o.status === 'failed' || o.status === 'timedOut';

/** A project's file in this run; `top`: a selected project (filters, grep and shard apply), not one that runs as a dependency. */
type Entry = { project: ResolvedProject; file: string; top: boolean; tests?: { titlePath: string[]; parallel?: number }[]; shard?: { general: boolean; parallel: number[] } };
type Plan = RunPlan & { path: string; order: string[] };
type Proc = { id: number; results: string; seen: number; listing: boolean; outcomes: Outcome[]; code?: number; stderr: string };

const color = (code: number) => (s: string) => (process.stdout.isTTY && !process.env.NO_COLOR ? `\x1b[${code}m${s}\x1b[0m` : s);
const green = color(32);
const red = color(31);
const yellow = color(33);
const dim = color(2);

/** Variables `pyro run` sets for its workers: never inherited from a run this one runs in. */
const INTERNAL_ENV = ['PYRO_WORKER', 'PYRO_PLAN', 'PYRO_WORKER_INDEX', 'PYRO_WORKER_ID', 'PYRO_WORKERS', 'PYRO_RESULTS', 'PYRO_RUN_WORKERS', 'PYRO_SHARD', 'PYRO_LIST', 'PYRO_FULLY_PARALLEL', 'PYRO_FILE_PROJECTS', 'PYRO_FILE_OWNERS'];

/** Ends each worker after its last file: afterAll hooks still due, worker fixtures, browsers (runner.ts, `finish`). */
const PRELOAD = `import { afterAll } from 'bun:test';
globalThis[Symbol.for('pyro.wright.preload')] = true;
afterAll(async () => {
  for (const finish of globalThis[Symbol.for('pyro.wright.finish')] ?? []) await finish();
}, 2147483647);
`;

/** Playwright's \`--project\` match: any case, \`*\` matches any text; every name must match a project. */
function filterProjects(projects: ResolvedProject[], names: string[]): ResolvedProject[] {
  if (!names.length) return projects;
  const exact = new Map(names.filter((n) => !n.includes('*')).map((n) => [n.toLowerCase(), n]));
  const patterns = names.filter((n) => n.includes('*')).map((n) => new RegExp(`^${n.toLowerCase().replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`));
  const unmatched = new Map(exact);
  const out = projects.filter((p) => {
    const name = p.name.toLowerCase();
    if (exact.has(name)) {
      unmatched.delete(name);
      return true;
    }
    return patterns.some((re) => re.test(name));
  });
  if (unmatched.size || !out.length) throw new Error(`no project named ${[...(unmatched.size ? unmatched.values() : names)].map((n) => JSON.stringify(n)).join(', ')} (projects: ${projects.map((p) => JSON.stringify(p.name)).join(', ')})`);
  return out;
}

/**
 * Playwright's shard split: groups (a file's tests that are not parallel, or
 * one parallel test) in order go to the shard where their first test falls,
 * shards balanced by test count.
 */
function shardGroups<G extends { size: number }>(shard: { current: number; total: number }, groups: G[]): G[] {
  const total = groups.reduce((n, g) => n + g.size, 0);
  const size = Math.floor(total / shard.total);
  const extra = total - size * shard.total;
  const k = shard.current - 1;
  const from = size * k + Math.min(extra, k);
  const to = from + size + (k < extra ? 1 : 0);
  const out: G[] = [];
  let at = 0;
  for (const g of groups) {
    if (at >= from && at < to) out.push(g);
    at += g.size;
  }
  return out;
}

export async function main(argv: string[]): Promise<number> {
  const args = parse(argv);
  if (args.config) process.env.PYRO_CONFIG = resolve(args.config);
  if (args.retries !== undefined) process.env.PYRO_RETRIES = args.retries;
  if (args.timeout !== undefined) process.env.PYRO_TIMEOUT = args.timeout;
  const { dependenciesOf, fullConfig, globalSetup, loadConfig, parseShard, projectClosure, projectFiles, projectPhases, startWebServers, stopWebServers } = await import('./config.js');
  const l = await loadConfig();

  // The shard: `--shard`, else the config's.
  let shard: { current: number; total: number } | null = null;
  const shardArg = args.shard ?? (l.config.shard && typeof l.config.shard === 'object' ? `${(l.config.shard as { current: number }).current}/${(l.config.shard as { total: number }).total}` : undefined);
  if (shardArg !== undefined) {
    const s = parseShard(shardArg);
    if (typeof s === 'string') {
      console.error(`pyro run: ${s}`);
      return 2;
    }
    shard = s.total > 1 ? s : null;
  }

  // Files, by project: the selected projects' files that match the filters.
  let selected: ResolvedProject[];
  try {
    selected = filterProjects(l.projects, args.projects);
  } catch (e) {
    console.error(`pyro run: ${(e as Error).message}`);
    return 1;
  }
  const filters = args.filters.map((f) => new RegExp(f));
  const matches = (f: string) => !filters.length || filters.some((re) => re.test(f) || re.test(relative(process.cwd(), f)));
  const allFiles = new Map<ResolvedProject, string[]>();
  const filesOf = (p: ResolvedProject) => {
    let f = allFiles.get(p);
    if (!f) allFiles.set(p, (f = projectFiles(p)));
    return f;
  };
  const closureOf = (list: Entry[]): Map<ResolvedProject, 'top-level' | 'dependency'> => {
    const projects = [...new Set(list.map((e) => e.project))];
    return args.noDeps ? new Map(projects.map((p) => [p, 'top-level'])) : projectClosure(l, projects);
  };
  let tops: Entry[] = selected.flatMap((p) => filesOf(p).filter(matches).map((file) => ({ project: p, file, top: true })));
  // A selected project another one depends on runs as a dependency (in full, in every shard), as in Playwright.
  const firstClosure = closureOf(tops);
  tops = tops.filter((e) => firstClosure.get(e.project) === 'top-level');
  /** The run: the projects the top entries' projects depend on (all their files), then those, in config order. */
  const withDependencies = (list: Entry[]): Entry[] => {
    const out: Entry[] = [];
    for (const [p, kind] of closureOf(list)) {
      if (kind === 'top-level') out.push(...list.filter((e) => e.project === p));
      else out.push(...filesOf(p).map((file) => ({ project: p, file, top: false })));
    }
    return out;
  };
  let entries = withDependencies(tops);
  if (!entries.length) {
    console.error('pyro run: no tests found');
    return 1;
  }

  // Workers.
  const cpus = availableParallelism();
  const wanted = args.workers ?? (l.config.workers === undefined ? undefined : String(l.config.workers));
  const workers = wanted === undefined ? Math.max(1, Math.floor(cpus / 2)) : wanted.endsWith('%') ? Math.max(1, Math.floor((cpus * Number(wanted.slice(0, -1))) / 100)) : Math.max(1, Number(wanted));

  const dir = mkdtempSync(join(tmpdir(), 'pyro-test-'));
  const preload = join(dir, 'preload.mjs');
  writeFileSync(preload, PRELOAD);
  const started = Date.now();
  const results: Outcome[] = [];
  const procs: Proc[] = [];
  const children = new Set<ChildProcess>();
  let failures = 0;
  let stopped = false;
  let printed = 0;
  const projectTitle = (o: { project: number; titlePath: string[] }) => `${l.projects[o.project]?.name ? `[${l.projects[o.project].name}] › ` : ''}${o.titlePath.join(' › ')}`;
  const print = (o: Outcome) => {
    if (o.kind === 'error') return;
    const title = projectTitle(o);
    const time = dim(`(${o.duration}ms)`);
    if (args.reporter === 'dot') {
      process.stdout.write(o.status === 'passed' ? (o.flaky ? yellow('±') : green('·')) : o.status === 'skipped' ? dim('°') : red('F'));
      return;
    }
    const n = String(++printed).padStart(3);
    const line =
      o.status === 'passed'
        ? `  ${o.flaky ? yellow('✓') : green('✓')} ${n} ${title} ${time}${o.flaky ? yellow(` (flaky, ${o.retries} ${o.retries === 1 ? 'retry' : 'retries'})`) : ''}${o.expectedFailure ? dim(' (expected to fail)') : ''}`
        : o.status === 'skipped'
          ? `  ${dim('-')} ${n} ${dim(title)}${o.skipReason ? dim(` (${o.skipReason})`) : ''}`
          : `  ${red('✘')} ${n} ${title} ${time}${o.status === 'timedOut' ? red(' (timed out)') : ''}`;
    console.log(line);
  };
  const drain = (p: Proc) => {
    let text = '';
    try {
      text = readFileSync(p.results, 'utf8');
    } catch {
      return;
    }
    const lines = text.split('\n');
    for (const line of lines.slice(p.seen, lines.length - 1)) {
      const o = JSON.parse(line) as Outcome;
      p.outcomes.push(o);
      if (p.listing) continue;
      results.push(o);
      if (isFailure(o) && o.kind !== 'error') failures++;
      print(o);
    }
    p.seen = lines.length - 1;
    if (args.maxFailures && failures >= args.maxFailures && !stopped) {
      stopped = true;
      for (const c of children) c.kill('SIGTERM');
    }
  };
  /** A worker that ended without results for a reason other than having no tests: a load error, a crash. */
  const crashed = (p: Proc) => {
    if (p.code === 0 || stopped) return false;
    if (p.listing) return /error:|SyntaxError|Cannot find/.test(p.stderr);
    if (p.outcomes.some((o) => isFailure(o) && o.kind !== 'error')) return false;
    // Bun exits non-zero for a worker that got no tests of its own: not a failure.
    return !(/No tests found|0 pass/.test(p.stderr) && !/error:|SyntaxError|Cannot find/.test(p.stderr));
  };

  const sizes = new Map<string, number>();
  const sizeOf = (f: string) => sizes.get(f) ?? (sizes.set(f, statSync(f).size), sizes.get(f)!);
  const parallelText = new Map<string, boolean>();
  /** Whether a file may hold parallel tests (a describe's `mode: 'parallel'`): then it may use every worker. */
  const mayBeParallel = (f: string) => parallelText.get(f) ?? (parallelText.set(f, /parallel/i.test(readFileSync(f, 'utf8'))), parallelText.get(f)!);
  const fullyParallelOf = (e: Entry) => args.fullyParallel || e.project.fullyParallel;
  const greps = (v: RegExp | RegExp[] | string | null | undefined): [string, string][] => (v === null || v === undefined ? [] : Array.isArray(v) ? v : [v]).map((g) => (g instanceof RegExp ? [g.source, g.flags] : [String(g), '']));
  /** Workers a file's tests can use: 1 for its tests that are not parallel, 1 per parallel test. */
  const unitsOf = (e: Entry): number | undefined => {
    if (e.shard) return (e.shard.general ? 1 : 0) + e.shard.parallel.length;
    if (!e.tests) return undefined;
    return (e.tests.some((t) => t.parallel === undefined) ? 1 : 0) + e.tests.filter((t) => t.parallel !== undefined).length;
  };
  let plans = 0;
  /** A worker process per slot for `round` (its files appear once): the files' projects, owners and shard parts. */
  const plan = (round: Entry[], listing: boolean): Plan => {
    let slots = 1;
    if (!listing) {
      const units = round.map(unitsOf);
      const known = units.every((u) => u !== undefined) ? units.reduce<number>((n, u) => n + u!, 0) : undefined;
      const spread = round.some((e) => fullyParallelOf(e) || mayBeParallel(e.file));
      slots = Math.max(1, Math.min(workers, known ?? (spread ? workers : round.length)));
    }
    const files: Record<string, FilePlan> = {};
    // Whole files to workers, round-robin by size so the big ones spread first.
    [...round]
      .sort((a, b) => sizeOf(b.file) - sizeOf(a.file) || a.file.localeCompare(b.file))
      .forEach((e, i) => {
        const g = greps(e.project.grep).filter(([s]) => s !== '.*');
        const gi = greps(e.project.grepInvert);
        files[e.file] = { project: e.project.index, owner: i % slots, fullyParallel: fullyParallelOf(e), grep: e.top, ...(g.length ? { projectGrep: g } : {}), ...(gi.length ? { projectGrepInvert: gi } : {}), ...(e.shard ? { shard: e.shard } : {}) };
      });
    const path = join(dir, `plan${plans++}.json`);
    writeFileSync(path, JSON.stringify({ slots, files }));
    return { slots, files, path, order: round.map((e) => e.file) };
  };
  /** Splits entries into rounds where each file appears once (a file in two projects runs in two processes). */
  const rounds = (list: Entry[]): Entry[][] => {
    const out: { files: Set<string>; entries: Entry[] }[] = [];
    for (const e of list) {
      let r = out.find((x) => !x.files.has(e.file));
      if (!r) out.push((r = { files: new Set(), entries: [] }));
      r.files.add(e.file);
      r.entries.push(e);
    }
    return out.map((r) => r.entries);
  };
  // Workers are numbered from 0 in start order (`workerIndex`); listing processes apart.
  const spawned = { workers: 0, listings: 0 };
  const spawnWorker = (p: Plan, slot: number, listing: boolean): Promise<Proc> => {
    const id = listing ? spawned.listings++ : spawned.workers++;
    const proc: Proc = { id, results: join(dir, `${listing ? 'list' : 'w'}${id}.jsonl`), seen: 0, listing, outcomes: [], stderr: '' };
    procs.push(proc);
    const env: Record<string, string | undefined> = { ...process.env };
    for (const k of INTERNAL_ENV) delete env[k];
    Object.assign(env, {
      PYRO_WORKER: '1',
      PYRO_PLAN: p.path,
      PYRO_WORKER_INDEX: String(slot),
      PYRO_WORKER_ID: String(proc.id),
      PYRO_RESULTS: proc.results,
      PYRO_RUN_WORKERS: String(workers),
      ...(shard ? { PYRO_SHARD: `${shard.current}/${shard.total}` } : {}),
      ...(args.grep ? { PYRO_GREP: args.grep } : {}),
      ...(args.grepInvert ? { PYRO_GREP_INVERT: args.grepInvert } : {}),
      ...(listing ? { PYRO_LIST: '1' } : {}),
      ...(args.update ? { PYRO_UPDATE_SNAPSHOTS: args.updateMode ?? 'changed' } : {}),
    });
    return new Promise((done) => {
      const child = spawn(process.execPath, ['test', '--preload', preload, ...p.order, '--timeout', '2147483647'], {
        cwd: process.cwd(),
        env,
        stdio: ['ignore', args.quiet || listing ? 'ignore' : 'pipe', 'pipe'],
      });
      children.add(child);
      // The tests' own output (console.log), line by line.
      let out = '';
      child.stdout?.on('data', (d) => {
        out += d;
        const parts = out.split('\n');
        out = parts.pop() ?? '';
        for (const line of parts) if (line.trim() && !/^bun test v|^pyro: CDP server listening/.test(line)) console.log(dim(`  [w${proc.id}] ${line}`));
      });
      // Bun's report: kept for a worker that ends without results (a crash or a load error).
      child.stderr?.on('data', (d) => (proc.stderr += d));
      child.on('exit', (code) => {
        children.delete(child);
        proc.code = code ?? 1;
        drain(proc);
        done(proc);
      });
    });
  };
  /** Lists the tests of `list` (one process per round, no browser): fills `tests`; false when a file failed to load. */
  const listTests = async (list: Entry[]): Promise<boolean> => {
    const done = await Promise.all(
      rounds(list).map(async (round) => {
        const p = await spawnWorker(plan(round, true), 0, true);
        for (const e of round) e.tests = p.outcomes.filter((o) => o.file === e.file && o.project === e.project.index).map((o) => ({ titlePath: o.titlePath, parallel: o.parallel }));
        return p;
      }),
    );
    const broken = done.filter(crashed);
    for (const p of broken) {
      console.log(red('\n  pyro run: listing the tests failed:'));
      console.log(p.stderr.split('\n').filter((line) => line.trim()).slice(-40).map((line) => `    ${line}`).join('\n'));
    }
    return !broken.length;
  };
  /** Runs entries: a round's processes on the workers' slots, each slot's rounds one after another. */
  const runEntries = async (list: Entry[]): Promise<void> => {
    const plans = rounds(list).map((r) => plan(r, false));
    await Promise.all(
      Array.from({ length: workers }, async (_, slot) => {
        for (const p of plans) if (!stopped && slot < p.slots) await spawnWorker(p, slot, false);
      }),
    );
  };

  let teardowns: (() => Promise<unknown>)[] = [];
  const end = async (code: number) => {
    for (const t of teardowns.splice(0)) await t();
    stopWebServers();
    rmSync(dir, { recursive: true, force: true });
    return code;
  };

  // Grep with dependencies, `--list` and `--shard` need the tests: one process lists them (no browser).
  const grepping = !!(args.grep || args.grepInvert);
  if (args.list || shard || (grepping && entries.some((e) => !e.top))) {
    if (!(await listTests(tops))) return end(1);
    tops = tops.filter((e) => e.tests!.length);
    if (shard) {
      // As Playwright: the selected projects' tests are split; dependencies run in each shard that needs them.
      type Group = { entry: Entry; general?: boolean; parallel?: number; size: number };
      const groups: Group[] = [];
      for (const e of tops) {
        const general = e.tests!.filter((t) => t.parallel === undefined).length;
        if (general) groups.push({ entry: e, general: true, size: general });
        for (const t of e.tests!) if (t.parallel !== undefined) groups.push({ entry: e, parallel: t.parallel, size: 1 });
      }
      for (const e of tops) e.shard = { general: false, parallel: [] };
      for (const g of shardGroups(shard, groups)) {
        if (g.general) g.entry.shard!.general = true;
        else g.entry.shard!.parallel.push(g.parallel!);
      }
      for (const e of tops) e.tests = e.tests!.filter((t) => (t.parallel === undefined ? e.shard!.general : e.shard!.parallel.includes(t.parallel)));
      tops = tops.filter((e) => e.tests!.length);
    }
    entries = withDependencies(tops);
    if (!entries.length) {
      if (shard) {
        console.log(`\npyro run: shard ${shard.current} of ${shard.total} has no tests`);
        return end(0);
      }
      console.error('pyro run: no tests found');
      return end(1);
    }
  }
  const runProjects = [...new Set(entries.map((e) => e.project))];
  const phases = projectPhases(l, runProjects);

  if (args.list) {
    if (!(await listTests(entries.filter((e) => !e.tests)))) return end(1);
    let n = 0;
    for (const phase of phases) {
      for (const e of entries.filter((x) => phase.includes(x.project))) {
        for (const t of e.tests!) {
          n++;
          console.log(`  ${projectTitle({ project: e.project.index, titlePath: t.titlePath })}`);
        }
      }
    }
    const files = new Set(entries.filter((e) => e.tests!.length).map((e) => e.file)).size;
    console.log(`\nTotal: ${n} test${n === 1 ? '' : 's'} in ${files} file${files === 1 ? '' : 's'}`);
    return end(0);
  }

  await startWebServers(l);
  const full = fullConfig(l, {
    workers,
    shard,
    grep: args.grep ? new RegExp(args.grep) : undefined,
    grepInvert: args.grepInvert ? new RegExp(args.grepInvert) : undefined,
    maxFailures: args.maxFailures,
    quiet: args.quiet || undefined,
    fullyParallel: args.fullyParallel || undefined,
    updateSnapshots: args.update ? ((args.updateMode ?? 'changed') as 'changed') : undefined,
  });
  teardowns = await globalSetup(l, full);

  console.log(`\nRunning ${entries.length} file${entries.length === 1 ? '' : 's'} using ${workers} worker${workers === 1 ? '' : 's'}${shard ? `, shard ${shard.current} of ${shard.total}` : ''}\n`);
  const ticker = setInterval(() => {
    for (const p of procs) if (p.code === undefined) drain(p);
  }, 50);
  // Phases: a project whose dependency failed (or a run stopped by --max-failures) does not run.
  const inRun = new Set(runProjects);
  const successful = new Set<ResolvedProject>();
  const blocked: Entry[] = [];
  let workerErrors = false;
  for (const phase of phases) {
    const runnable = phase.filter((p) => !stopped && dependenciesOf(l, p).every((d) => !inRun.has(d) || successful.has(d)));
    blocked.push(...entries.filter((e) => phase.includes(e.project) && !runnable.includes(e.project)));
    const before = procs.length;
    await runEntries(entries.filter((e) => runnable.includes(e.project)));
    const ran = procs.slice(before);
    // As Playwright: after a worker error no project counts as passed.
    if (ran.some((p) => crashed(p) || p.outcomes.some((o) => o.kind === 'error'))) workerErrors = true;
    if (workerErrors) continue;
    for (const p of runnable) if (!results.some((r) => r.project === p.index && isFailure(r))) successful.add(p);
  }
  clearInterval(ticker);
  for (const p of procs) drain(p);
  // The tests of the projects that did not run, counted.
  const blockedListed = !stopped && blocked.length && (await listTests(blocked.filter((e) => !e.tests)));
  const notRun = blockedListed ? blocked.reduce((n, e) => n + e.tests!.length, 0) : 0;
  if (args.reporter === 'dot') console.log();

  // Workers that failed outside any test (a syntax error, a crash): show what Bun said.
  for (const p of procs) {
    if (p.listing || !crashed(p)) continue;
    failures++;
    console.log(red(`\n  worker ${p.id} exited with ${p.code}:`));
    console.log(p.stderr.split('\n').filter((line) => line.trim()).slice(-40).map((line) => `    ${line}`).join('\n'));
  }

  const showErrors = (errors: Outcome['errors']) => {
    for (const e of errors ?? [{ message: '' }]) {
      const text = e.stack && e.stack.includes(e.message) ? e.stack : `${e.message}\n${e.stack ?? ''}`;
      console.log(text.split('\n').map((line) => `    ${line}`).join('\n'));
    }
  };
  const failed = results.filter((r) => isFailure(r) && r.kind !== 'error');
  failed.forEach((r, i) => {
    const title = projectTitle(r);
    console.log(`\n  ${i + 1}) ${title} ${'─'.repeat(Math.max(0, 60 - title.length))}\n`);
    showErrors(r.errors);
  });
  const errors = results.filter((r) => r.kind === 'error');
  for (const r of errors) {
    console.log(red(`\n  Error outside any test: ${projectTitle(r)}\n`));
    showErrors(r.errors);
  }
  const count = (f: (r: Outcome) => boolean) => results.filter((r) => r.kind !== 'error' && f(r)).length;
  const passed = count((r) => r.status === 'passed' && !r.flaky);
  const flaky = count((r) => r.status === 'passed' && !!r.flaky);
  const skipped = count((r) => r.status === 'skipped' && !r.didNotRun);
  const didNotRun = count((r) => r.status === 'skipped' && !!r.didNotRun) + notRun;
  const secs = ((Date.now() - started) / 1000).toFixed(1);
  console.log('');
  if (failed.length) console.log(red(`  ${failed.length} failed`));
  if (flaky) console.log(yellow(`  ${flaky} flaky`));
  if (skipped) console.log(dim(`  ${skipped} skipped`));
  if (didNotRun) console.log(dim(`  ${didNotRun} did not run`));
  if (errors.length) console.log(red(`  ${errors.length} error${errors.length === 1 ? ' was' : 's were'} not part of any test`));
  console.log(green(`  ${passed} passed`) + dim(` (${secs}s)`));
  return end(failures || errors.length ? 1 : 0);
}
