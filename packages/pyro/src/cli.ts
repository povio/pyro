#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolveBinary } from './binary.js';

// `pyro mcp`: the MCP server for AI agents (Node or Bun). It ends the
// process when its client goes away: the browser's connection would keep it.
if (process.argv[2] === 'mcp') {
  const { main } = await import('./mcp/cli.js');
  process.exit(await main(process.argv.slice(3)));
}
// `pyro run`: the Playwright-style test runner, and `pyro
// e2e`: the one-process executor of @povio/pyro/e2e specs (both run on Bun).
if (process.argv[2] === 'run' || process.argv[2] === 'e2e') {
  if ((globalThis as { Bun?: unknown }).Bun) {
    if (process.argv[2] === 'e2e') {
      const { cli } = await import('./e2e/cli.js');
      process.exitCode = await cli(process.argv.slice(3));
    } else {
      const { main } = await import('./wright/cli.js');
      process.exitCode = await main(process.argv.slice(3));
    }
  } else {
    const child = spawn('bun', [fileURLToPath(import.meta.url), ...process.argv.slice(2)], { stdio: 'inherit' });
    child.once('error', () => {
      console.error(`pyro ${process.argv[2]}: needs Bun (https://bun.sh) on PATH`);
      process.exitCode = 1;
    });
    child.once('exit', (code) => (process.exitCode = code ?? 1));
  }
} else try {
  const child = spawn(resolveBinary(), process.argv.slice(2), { stdio: 'inherit' });
  const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'] as const;
  const forward = (signal: NodeJS.Signals) => { child.kill(signal); };
  for (const signal of signals) process.on(signal, forward);
  const cleanup = () => { for (const signal of signals) process.off(signal, forward); };
  child.once('error', (error) => {
    cleanup();
    console.error(`pyro: ${error.message}`);
    process.exitCode = 1;
  });
  child.once('exit', (code, signal) => {
    cleanup();
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
} catch (error) {
  console.error(`pyro: ${error instanceof Error ? error.message : error}`);
  process.exitCode = 1;
}
