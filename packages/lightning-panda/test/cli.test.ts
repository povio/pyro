// `lightningpanda test` on a TOS-shaped config: discovery of `.e2e.ts` files per
// project, each file with its project's `use`, parallel tests spread over worker
// processes (own module state), skips reported as skips, the exit code.
import { expect, test } from 'bun:test';
import { join } from 'node:path';

const dir = join(import.meta.dir, 'cli-fixture');
const cli = join(import.meta.dir, '../src/cli.ts');
const run = (...args: string[]) => {
  const r = Bun.spawnSync(['bun', cli, 'test', '-c', join(dir, 'playwright.config.ts'), ...args], { cwd: dir, env: { ...process.env, NO_COLOR: '1' } });
  return { code: r.exitCode, out: r.stdout.toString() + r.stderr.toString() };
};

test('runs every project and reports passes, failures and skips', () => {
  const { code, out } = run();
  expect(out).toContain('Running 3 files using 2 workers');
  expect(out).toContain('✓');
  expect(out).toMatch(/✘ +\d+ e2e\/root\.e2e\.ts › fails/);
  expect(out).toMatch(/- +\d+ e2e\/root\.e2e\.ts › skips at run time \(not today\)/);
  expect(out).toMatch(/- +\d+ e2e\/root\.e2e\.ts › skipped when declared/);
  expect(out).toContain('uses its own project');
  expect(out).toContain('1 failed');
  expect(out).toContain('2 skipped');
  expect(out).toContain('6 passed');
  expect(out).toContain('expect(received).toBe(expected)');
  expect(code).toBe(1);
}, 60_000);

test('spreads parallel tests over the workers, each with its own module state', () => {
  const { out } = run('parallel', '-j', '2');
  expect(out).toContain('4 passed');
  expect(out).toMatch(/\[w0\] ran p\d in worker 0/);
  expect(out).toMatch(/\[w1\] ran p\d in worker 1/);
}, 60_000);

test('filters by file and grep, lists without running', () => {
  expect(run('sub').out).toContain('1 passed');
  const grep = run('-g', 'uses its');
  expect(grep.out).toContain('2 passed');
  expect(grep.code).toBe(0);
  const list = run('--list');
  expect(list.out).toContain('Total: 9 tests in 3 files');
}, 60_000);
