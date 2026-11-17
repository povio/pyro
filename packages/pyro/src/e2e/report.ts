// Console output (a line per test as it finishes, failures in full) and the
// JUnit XML report of `pyro e2e`.
import { hostname } from 'node:os';
import { titlePath, type TestCase } from './registry.js';

export type Failure = { title: string; file: string; error: unknown };

export type Summary = {
  tests: TestCase[];
  files: string[];
  passed: number;
  failed: number;
  skipped: number;
  todo: number;
  filtered: number;
  /** Queued tests that never started (a bail or an interruption). */
  notRun: number;
  loadFailures: number;
  hookFailures: number;
  failures: Failure[];
  ms: number;
  interrupted: boolean;
};

const tty = () => process.stdout.isTTY && !process.env.NO_COLOR;
const paint = (code: number) => (s: string) => (tty() ? `\x1b[${code}m${s}\x1b[0m` : s);
const green = paint(32);
const red = paint(31);
const yellow = paint(33);
const dim = paint(2);

/** Frames of the executor itself, which say nothing about the failure. */
const internal = /\/src\/e2e\/(run|registry)\.ts|\/dist\/e2e\/|node:async_hooks|native:\d|^\s+at new Promise|^\s+at call /;

const errorText = (error: unknown): string => {
  if (!(error instanceof Error)) return String(error);
  const text = error.stack && error.stack.includes(error.message) ? error.stack : `${error.name}: ${error.message}\n${error.stack ?? ''}`;
  return text.split('\n').filter((line) => !(line.trimStart().startsWith('at ') && internal.test(line))).join('\n');
};

const indent = (s: string) => s.trimEnd().split('\n').map((line) => `    ${line}`).join('\n');

const duration = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${ms.toFixed(2)}ms`);

export function consoleReporter() {
  return {
    test(test: TestCase) {
      const run = test.run!;
      const title = `${test.scope.file} > ${titlePath(test).join(' > ')}`;
      const tries = run.attempts > 1 ? ` ${yellow(`(run ${run.attempts} times)`)}` : '';
      const shots = run.screenshots.length ? `\n${indent(run.screenshots.map((p) => `screenshot: ${p}`).join('\n'))}` : '';
      if (run.status === 'pass') console.log(`${green('✓')} ${title}${tries} ${dim(`[${duration(run.ms)}]`)}${shots}`);
      else if (run.status === 'skip') console.log(`${yellow('»')} ${title} ${dim(run.skipReason ? `(skipped: ${run.skipReason})` : '(skipped)')}`);
      else console.log(`${red('✗')} ${title}${tries} ${dim(`[${duration(run.ms)}]`)}\n${indent(errorText(run.error))}${shots}`);
    },
    /** A test failed and runs again (`attempt` runs so far). */
    retry(test: TestCase, attempt: number, error: unknown) {
      const title = `${test.scope.file} > ${titlePath(test).join(' > ')}`;
      const first = (error instanceof Error ? error.message : String(error)).split('\n')[0];
      console.log(`${yellow('↻')} ${title} ${dim(`failed on run ${attempt}, runs again: ${first}`)}`);
    },
    loadFailure(file: string, error: unknown) {
      console.log(`${red('✗')} ${file} failed to load\n${indent(errorText(error))}`);
    },
    hookFailure(file: string, title: string, error: unknown) {
      console.log(`${red('✗')} ${file ? `${file} > ` : ''}${title}\n${indent(errorText(error))}`);
    },
    summary(s: Summary) {
      if (s.failures.length) {
        console.log(`\n${red(`${s.failures.length} failure${s.failures.length === 1 ? '' : 's'}:`)}`);
        for (const f of s.failures) {
          const first = (f.error instanceof Error ? f.error.message : String(f.error)).split('\n')[0];
          console.log(`  ${red('✗')} ${f.file ? `${f.file} > ` : ''}${f.title}: ${first}`);
        }
      }
      console.log('');
      console.log(` ${green(`${s.passed} pass`)}`);
      if (s.failed) console.log(` ${red(`${s.failed} fail`)}`);
      if (s.skipped) console.log(` ${yellow(`${s.skipped} skip`)}`);
      const flaky = s.tests.filter((t) => t.run?.status === 'pass' && t.run.attempts > 1).length;
      if (flaky) console.log(` ${yellow(`${flaky} flaky (passed on a retry)`)}`);
      if (s.todo) console.log(` ${yellow(`${s.todo} todo`)}`);
      if (s.filtered) console.log(` ${dim(`${s.filtered} filtered out`)}`);
      if (s.notRun) console.log(` ${yellow(`${s.notRun} not run`)}`);
      if (s.loadFailures) console.log(` ${red(`${s.loadFailures} file${s.loadFailures === 1 ? '' : 's'} failed to load`)}`);
      if (s.hookFailures) console.log(` ${red(`${s.hookFailures} hook failure${s.hookFailures === 1 ? '' : 's'}`)}`);
      const ran = s.passed + s.failed;
      console.log(`Ran ${ran} test${ran === 1 ? '' : 's'} across ${s.files.length} file${s.files.length === 1 ? '' : 's'}. ${dim(`[${duration(s.ms)}]`)}${s.interrupted ? red(' (interrupted)') : ''}`);
    },
  };
}

const xml = (s: string) =>
  s.replace(/[<>&"']/g, (c) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;', "'": '&apos;' })[c]!).replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '');

/** JUnit XML: a testsuite per file, a testcase per test, hook and load failures as testcases of their own. */
export function junitReport(s: Summary): string {
  const byFile = new Map<string, string[]>();
  const stats = new Map<string, { tests: number; failures: number; skipped: number; ms: number }>();
  const add = (file: string, xmlCase: string, kind: 'pass' | 'fail' | 'skip', ms: number) => {
    if (!byFile.has(file)) {
      byFile.set(file, []);
      stats.set(file, { tests: 0, failures: 0, skipped: 0, ms: 0 });
    }
    byFile.get(file)!.push(xmlCase);
    const st = stats.get(file)!;
    st.tests++;
    st.ms += ms;
    if (kind === 'fail') st.failures++;
    if (kind === 'skip') st.skipped++;
  };
  const failure = (error: unknown) => {
    const message = (error instanceof Error ? error.message : String(error)).split('\n')[0];
    const type = error instanceof Error ? error.name : 'Error';
    return `<failure message="${xml(message)}" type="${xml(type)}">${xml(errorText(error))}</failure>`;
  };
  for (const t of s.tests) {
    const run = t.run;
    if (!run) continue; // filtered out or never started
    const head = `<testcase name="${xml(titlePath(t).join(' > '))}" classname="${xml(t.scope.file)}" time="${(run.ms / 1000).toFixed(3)}"`;
    if (run.status === 'pass' && run.attempts > 1) add(t.scope.file, `${head}><properties><property name="flaky" value="${run.attempts}" /></properties></testcase>`, 'pass', run.ms);
    else if (run.status === 'pass') add(t.scope.file, `${head} />`, 'pass', run.ms);
    else if (run.status === 'fail') {
      const attachments = run.screenshots.map((p) => `[[ATTACHMENT|${p}]]`).join('\n');
      add(t.scope.file, `${head}>${failure(run.error)}${attachments ? `<system-out>${xml(attachments)}</system-out>` : ''}</testcase>`, 'fail', run.ms);
    }
    else add(t.scope.file, `${head}><skipped${run.status === 'todo' ? ' message="todo"' : run.skipReason ? ` message="${xml(run.skipReason)}"` : ''} /></testcase>`, 'skip', 0);
  }
  const tested = new Set(s.tests.filter((t) => t.run && t.run.status !== 'skip' && t.run.status !== 'todo').map((t) => t.run));
  for (const f of s.failures) {
    if (s.tests.some((t) => tested.has(t.run) && t.run?.error === f.error)) continue; // a test's own failure
    add(f.file || 'pyro e2e', `<testcase name="${xml(f.title)}" classname="${xml(f.file)}" time="0">${failure(f.error)}</testcase>`, 'fail', 0);
  }
  let tests = 0, failures = 0, skipped = 0;
  const suites = [...byFile].map(([file, cases]) => {
    const st = stats.get(file)!;
    tests += st.tests;
    failures += st.failures;
    skipped += st.skipped;
    return `  <testsuite name="${xml(file)}" file="${xml(file)}" tests="${st.tests}" failures="${st.failures}" skipped="${st.skipped}" time="${(st.ms / 1000).toFixed(3)}" hostname="${xml(hostname())}">\n    ${cases.join('\n    ')}\n  </testsuite>`;
  });
  return `<?xml version="1.0" encoding="UTF-8"?>\n<testsuites name="pyro e2e" tests="${tests}" failures="${failures}" skipped="${skipped}" time="${(s.ms / 1000).toFixed(3)}">\n${suites.join('\n')}\n</testsuites>\n`;
}
