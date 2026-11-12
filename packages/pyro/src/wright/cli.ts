// `pyro run`: Playwright's `playwright test` for tests written
// against `@povio/pyro/wright`. It reads the config, finds
// each project's test files (`testDir`, `testMatch`, `testIgnore`), starts
// the web servers and runs global setup once, then runs the files in worker
// processes (`bun test`, one browser each): a file's tests stay in one
// worker unless they are parallel (`fullyParallel`, `mode: 'parallel'`),
// which spread over the workers, each with its own module state. It reports
// from the results the workers record (skips as skips, retries as flaky).
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { availableParallelism, tmpdir } from 'node:os';
import { join, relative, resolve } from 'node:path';

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
  filters: string[];
};

const HELP = `usage: pyro run [options] [test-filter...]

Runs Playwright-style tests (written against @povio/pyro/wright)
with bun test, in worker processes, as \`playwright test\` does.

  -c, --config <file>     config file (default: pyro.config.* or playwright.config.*)
      --project <name>    only this project (repeatable)
  -j, --workers <n|n%>    worker processes (default: the config's workers, else half the CPUs)
  -g, --grep <regex>      only tests whose title path matches
      --grep-invert <re>  skip tests whose title path matches
      --retries <n>       retries of a failing test
      --timeout <ms>      test timeout
  -u, --update-snapshots [mode]
                          rewrite snapshots and screenshot baselines: changed
                          (the default), all, missing or none
      --fully-parallel    run every file's tests in parallel
  -x                      stop after the first failure (as --max-failures=1)
      --max-failures <n>  stop after n failures
      --reporter <name>   list (default), line or dot
      --list              list the tests without running them
  -q, --quiet             do not print the tests' own output

test-filter: a regular expression the test file's path must match.
`;

function parse(argv: string[]): Args {
  const a: Args = { projects: [], update: false, list: false, fullyParallel: false, reporter: 'list', quiet: false, filters: [] };
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

type Outcome = { file: string; titlePath: string[]; status: 'passed' | 'failed' | 'timedOut' | 'skipped'; flaky?: boolean; expectedFailure?: boolean; duration: number; retries: number; error?: { message: string; stack?: string }; skipReason?: string; worker: number };

const color = (code: number) => (s: string) => (process.stdout.isTTY && !process.env.NO_COLOR ? `\x1b[${code}m${s}\x1b[0m` : s);
const green = color(32);
const red = color(31);
const yellow = color(33);
const dim = color(2);

export async function main(argv: string[]): Promise<number> {
  const args = parse(argv);
  if (args.config) process.env.PYRO_CONFIG = resolve(args.config);
  if (args.retries !== undefined) process.env.PYRO_RETRIES = args.retries;
  if (args.timeout !== undefined) process.env.PYRO_TIMEOUT = args.timeout;
  const { globalSetup, loadConfig, projectFiles, startWebServers, stopWebServers } = await import('./config.js');
  const l = await loadConfig();

  // Files, by project.
  const projects = args.projects.length ? l.projects.filter((p) => args.projects.includes(p.name)) : l.projects;
  if (!projects.length) {
    console.error(`pyro run: no project named ${args.projects.join(', ')}`);
    return 1;
  }
  const filters = args.filters.map((f) => new RegExp(f));
  const fileProject: Record<string, number> = {};
  for (const p of projects) {
    for (const f of projectFiles(p)) {
      const rel = relative(process.cwd(), f);
      if (filters.length && !filters.some((re) => re.test(f) || re.test(rel))) continue;
      fileProject[f] ??= p.index;
    }
  }
  const files = Object.keys(fileProject);
  if (!files.length) {
    console.error('pyro run: no tests found');
    return 1;
  }

  // Workers.
  const cpus = availableParallelism();
  const wanted = args.workers ?? (l.config.workers === undefined ? undefined : String(l.config.workers));
  const fullyParallel = args.fullyParallel || l.config.fullyParallel === true;
  let workers = wanted === undefined ? Math.max(1, Math.floor(cpus / 2)) : wanted.endsWith('%') ? Math.max(1, Math.floor((cpus * Number(wanted.slice(0, -1))) / 100)) : Math.max(1, Number(wanted));
  // Not capped by the file count: a file's parallel describes spread over every worker
  // (a worker without tests of its own exits before it starts a browser).
  if (args.list) workers = 1;
  // Whole files to workers, round-robin by size so the big ones spread first.
  const bySize = [...files].sort((a, b) => (statSync(b).size - statSync(a).size) || a.localeCompare(b));
  const owners: Record<string, number> = {};
  bySize.forEach((f, i) => (owners[f] = i % workers));

  let teardowns: (() => Promise<unknown>)[] = [];
  if (!args.list) {
    await startWebServers(l);
    teardowns = await globalSetup(l);
  }

  const dir = mkdtempSync(join(tmpdir(), 'pyro-test-'));
  const started = Date.now();
  const results: Outcome[] = [];
  const seen = new Map<number, number>();
  let failures = 0;
  let stopped = false;
  const children: ReturnType<typeof spawn>[] = [];
  const print = (o: Outcome) => {
    if (args.list) return;
    const title = `${o.titlePath.join(' › ')}`;
    const time = dim(`(${o.duration}ms)`);
    if (args.reporter === 'dot') {
      process.stdout.write(o.status === 'passed' ? (o.flaky ? yellow('±') : green('·')) : o.status === 'skipped' ? dim('°') : red('F'));
      return;
    }
    const n = String(results.length).padStart(3);
    const line =
      o.status === 'passed'
        ? `  ${o.flaky ? yellow('✓') : green('✓')} ${n} ${title} ${time}${o.flaky ? yellow(` (flaky, ${o.retries} ${o.retries === 1 ? 'retry' : 'retries'})`) : ''}${o.expectedFailure ? dim(' (expected to fail)') : ''}`
        : o.status === 'skipped'
          ? `  ${dim('-')} ${n} ${dim(title)}${o.skipReason ? dim(` (${o.skipReason})`) : ''}`
          : `  ${red('✘')} ${n} ${title} ${time}${o.status === 'timedOut' ? red(' (timed out)') : ''}`;
    console.log(line);
  };
  const drain = (k: number) => {
    let text = '';
    try {
      text = readFileSync(join(dir, `w${k}.jsonl`), 'utf8');
    } catch {
      return;
    }
    const lines = text.split('\n');
    const from = seen.get(k) ?? 0;
    for (const line of lines.slice(from, lines.length - 1)) {
      const o = JSON.parse(line) as Outcome;
      results.push(o);
      if (o.status === 'failed' || o.status === 'timedOut') failures++;
      print(o);
    }
    seen.set(k, lines.length - 1);
    if (args.maxFailures && failures >= args.maxFailures && !stopped) {
      stopped = true;
      for (const c of children) c.kill('SIGTERM');
    }
  };

  if (!args.list) console.log(`\nRunning ${files.length} file${files.length === 1 ? '' : 's'} using ${workers} worker${workers === 1 ? '' : 's'}\n`);
  const ticker = setInterval(() => {
    for (let k = 0; k < workers; k++) drain(k);
  }, 50);
  const stderr: string[] = [];
  const exits = await Promise.all(
    Array.from({ length: workers }, (_, k) =>
      new Promise<number>((done) => {
        const child = spawn(process.execPath, ['test', ...files, '--timeout', '2147483647'], {
          cwd: process.cwd(),
          env: {
            ...process.env,
            PYRO_WORKER: '1',
            PYRO_WORKERS: String(workers),
            PYRO_WORKER_INDEX: String(k),
            PYRO_RESULTS: join(dir, `w${k}.jsonl`),
            PYRO_FILE_PROJECTS: JSON.stringify(fileProject),
            PYRO_FILE_OWNERS: JSON.stringify(owners),
            ...(fullyParallel ? { PYRO_FULLY_PARALLEL: '1' } : {}),
            ...(args.grep ? { PYRO_GREP: args.grep } : {}),
            ...(args.grepInvert ? { PYRO_GREP_INVERT: args.grepInvert } : {}),
            ...(args.list ? { PYRO_LIST: '1' } : {}),
            ...(args.update ? { PYRO_UPDATE_SNAPSHOTS: args.updateMode ?? 'changed' } : {}),
          },
          stdio: ['ignore', args.quiet ? 'ignore' : 'pipe', 'pipe'],
        });
        children.push(child);
        // The tests' own output (console.log), line by line.
        let out = '';
        child.stdout?.on('data', (d) => {
          out += d;
          const parts = out.split('\n');
          out = parts.pop() ?? '';
          for (const p of parts) if (p.trim() && !/^bun test v|^pyro: CDP server listening/.test(p)) console.log(dim(`  [w${k}] ${p}`));
        });
        // Bun's report: kept for a worker that ends without results (a crash or a load error).
        let err = '';
        child.stderr?.on('data', (d) => (err += d));
        child.on('exit', (code) => {
          stderr[k] = err;
          done(code ?? 1);
        });
      }),
    ),
  );
  clearInterval(ticker);
  for (let k = 0; k < workers; k++) drain(k);

  for (const t of teardowns.splice(0)) await t();
  stopWebServers();
  rmSync(dir, { recursive: true, force: true });

  if (args.list) {
    for (const o of results) console.log(`  ${o.titlePath.join(' › ')}`);
    console.log(`\nTotal: ${results.length} test${results.length === 1 ? '' : 's'} in ${new Set(results.map((r) => r.file)).size} files`);
    return 0;
  }
  if (args.reporter === 'dot') console.log();

  // Workers that failed outside any test (a syntax error, a crash): show what Bun said.
  exits.forEach((code, k) => {
    const mine = results.filter((r) => r.worker === k);
    const testFailed = mine.some((r) => r.status === 'failed' || r.status === 'timedOut');
    // Bun exits non-zero for a worker that got no tests of its own: not a failure.
    const noTests = !mine.length && /No tests found|0 pass/.test(stderr[k]) && !/error:|SyntaxError|Cannot find/.test(stderr[k]);
    if (code !== 0 && !testFailed && !stopped && !noTests) {
      failures++;
      console.log(red(`\n  worker ${k} exited with ${code}:`));
      console.log(stderr[k].split('\n').filter((l) => l.trim()).slice(-40).map((l) => `    ${l}`).join('\n'));
    }
  });

  const failed = results.filter((r) => r.status === 'failed' || r.status === 'timedOut');
  failed.forEach((r, i) => {
    console.log(`\n  ${i + 1}) ${r.titlePath.join(' › ')} ${'─'.repeat(Math.max(0, 60 - r.titlePath.join(' › ').length))}\n`);
    const text = r.error?.stack && r.error.stack.includes(r.error.message) ? r.error.stack : `${r.error?.message ?? ''}\n${r.error?.stack ?? ''}`;
    console.log(text.split('\n').map((l) => `    ${l}`).join('\n'));
  });
  const count = (f: (r: Outcome) => boolean) => results.filter(f).length;
  const passed = count((r) => r.status === 'passed' && !r.flaky);
  const flaky = count((r) => r.status === 'passed' && !!r.flaky);
  const skipped = count((r) => r.status === 'skipped');
  const secs = ((Date.now() - started) / 1000).toFixed(1);
  console.log('');
  if (failed.length) console.log(red(`  ${failed.length} failed`));
  if (flaky) console.log(yellow(`  ${flaky} flaky`));
  if (skipped) console.log(dim(`  ${skipped} skipped`));
  console.log(green(`  ${passed} passed`) + dim(` (${secs}s)`));
  return failures ? 1 : 0;
}
