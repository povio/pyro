#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { resolveBinary } from './binary.js';

try {
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
