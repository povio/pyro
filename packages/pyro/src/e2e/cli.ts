// `pyro e2e [options] [paths...]` (runs on Bun).
import { availableParallelism } from 'node:os';
import { Executor, type Options } from './run.js';

const HELP = `usage: pyro e2e [options] [path | dir | glob | filter ...]

Runs spec files written against @povio/pyro/e2e in one process:
tests from every file share one queue (up to --concurrency at a time, so
cases of different files interleave) and one fork-mode browser, which the
run launches on first use and always closes. Without paths it runs every
*.test.* / *.spec.* file under the working directory; an argument that is
not a file, directory or glob filters the spec paths, as with bun test.

  -j, --concurrency <n>           tests at a time across all files (default: the CPU count)
      --max-concurrency <n>       tests of one file at a time (as bun test's; default: no limit)
      --timeout <ms>              default timeout of tests and hooks (default 5000)
  -t, --test-name-pattern <re>    only tests whose name (describes and test,
                                  joined by spaces) matches
      --only                      only tests and describes marked .only
      --shard <i>/<n>             run shard i of n: files split by --timings
      --timings <file>            per-file durations, {"version":1,"files":{"<path>":ms}}
      --update-timings            write this run's per-file durations to --timings
      --setup <module>            exports setup(), browser(result), teardown(result)
                                  (or a default export with them); see the README
      --preload <module>          loaded before the specs; its top-level hooks
                                  apply to every test (repeatable)
      --browser-arg <arg>         extra pyro serve argument (repeatable)
      --reporter junit            also write a JUnit report ...
      --reporter-outfile <file>   ... to this file
      --junit <file>              both at once
  -u, --update-snapshots          write screenshot baselines instead of comparing
      --bail [n]                  stop after n failures (default 1)
  -h, --help
`;

export async function main(argv: string[]): Promise<number> {
  const o: Options = {
    cwd: process.cwd(),
    paths: [],
    concurrency: availableParallelism(),
    timeout: 5_000,
    only: false,
    updateTimings: false,
    preload: [],
    browserArgs: [],
    updateSnapshots: false,
  };
  let reporter: string | undefined;
  let outfile: string | undefined;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const eq = arg.startsWith('--') ? arg.indexOf('=') : -1;
    const flag = eq > 0 ? arg.slice(0, eq) : arg;
    const inline = eq > 0 ? arg.slice(eq + 1) : undefined;
    const value = () => {
      const v = inline ?? argv[++i];
      if (v === undefined) throw new Usage(`${flag} needs a value`);
      return v;
    };
    const number = () => {
      const v = Number(value());
      if (!Number.isFinite(v) || v < 0) throw new Usage(`${flag} needs a number`);
      return v;
    };
    switch (flag) {
      case '-h':
      case '--help':
        console.log(HELP);
        return 0;
      case '-j':
      case '--concurrency':
        o.concurrency = Math.max(1, Math.floor(number()));
        break;
      case '--max-concurrency':
        o.perFile = Math.max(1, Math.floor(number()));
        break;
      case '--timeout':
        o.timeout = number();
        break;
      case '-t':
      case '--test-name-pattern':
        o.testNamePattern = new RegExp(value());
        break;
      case '--only':
        o.only = true;
        break;
      case '--shard': {
        const m = /^(\d+)\/(\d+)$/.exec(value());
        if (!m || Number(m[1]) < 1 || Number(m[1]) > Number(m[2])) throw new Usage('--shard needs <i>/<n> with 1 <= i <= n');
        o.shard = { index: Number(m[1]), count: Number(m[2]) };
        break;
      }
      case '--timings':
        o.timings = value();
        break;
      case '--update-timings':
        o.updateTimings = true;
        break;
      case '--setup':
        o.setup = value();
        break;
      case '--preload':
        o.preload.push(value());
        break;
      case '--browser-arg':
        o.browserArgs.push(value());
        break;
      case '--reporter':
        reporter = value();
        if (reporter !== 'junit' && reporter !== 'console') throw new Usage(`unknown reporter ${reporter} (console or junit)`);
        break;
      case '--reporter-outfile':
        outfile = value();
        break;
      case '--junit':
        reporter = 'junit';
        outfile = value();
        break;
      case '-u':
      case '--update-snapshots':
        o.updateSnapshots = true;
        break;
      case '--bail':
        if (inline !== undefined || /^\d+$/.test(argv[i + 1] ?? '')) o.bail = Math.max(1, number());
        else o.bail = 1;
        break;
      default:
        if (flag.startsWith('-')) throw new Usage(`unknown option ${flag}`);
        o.paths.push(arg);
    }
  }
  if (reporter === 'junit') {
    if (!outfile) throw new Usage('--reporter junit needs --reporter-outfile <file>');
    o.junit = outfile;
  }
  if (o.updateTimings && !o.timings) throw new Usage('--update-timings needs --timings <file>');

  const executor = new Executor(o);
  let interrupted = false;
  const onSignal = (signal: NodeJS.Signals) => {
    if (interrupted) process.exit(128 + (signal === 'SIGINT' ? 2 : signal === 'SIGHUP' ? 1 : 15));
    interrupted = true;
    console.error(`\npyro e2e: ${signal}: stopping (the browser closes, teardown runs)`);
    void executor.abort().finally(() => process.exit(128 + (signal === 'SIGINT' ? 2 : signal === 'SIGHUP' ? 1 : 15)));
  };
  const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'] as const;
  for (const signal of signals) process.on(signal, onSignal);
  try {
    return await executor.run();
  } finally {
    for (const signal of signals) process.off(signal, onSignal);
  }
}

class Usage extends Error {}

/** `main` with usage errors printed (exit code 2). */
export async function cli(argv: string[]): Promise<number> {
  try {
    return await main(argv);
  } catch (error) {
    if (error instanceof Usage) {
      console.error(`pyro e2e: ${error.message}\n\n${HELP}`);
      return 2;
    }
    throw error;
  }
}
