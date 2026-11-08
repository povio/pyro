#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolveBinary } from './binary.js';

// `lightningpanda test`: the Playwright-style test runner (it runs on Bun).
if (process.argv[2] === 'test') {
  if ((globalThis as { Bun?: unknown }).Bun) {
    const { main } = await import('./playwright/cli.js');
    process.exitCode = await main(process.argv.slice(3));
  } else {
    const child = spawn('bun', [fileURLToPath(import.meta.url), ...process.argv.slice(2)], { stdio: 'inherit' });
    child.once('error', () => {
      console.error('lightningpanda test: needs Bun (https://bun.sh) on PATH');
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
    console.error(`lightningpanda: ${error.message}`);
    process.exitCode = 1;
  });
  child.once('exit', (code, signal) => {
    cleanup();
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
} catch (error) {
  console.error(`lightningpanda: ${error instanceof Error ? error.message : error}`);
  process.exitCode = 1;
}
