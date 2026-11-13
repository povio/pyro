import { expect, test } from 'bun:test';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { resolveBinary } from '../src/binary';
import { Pyro } from '../src/index';

test('resolves each bundled architecture and gives an explicit override precedence', () => {
  const root = mkdtempSync(join(tmpdir(), 'pyro-package-'));
  try {
    for (const platform of ['darwin', 'linux']) {
      for (const arch of ['arm64', 'x64']) {
        const dir = join(root, 'native', `${platform}-${arch}`);
        mkdirSync(dir, { recursive: true });
        const binary = join(dir, 'pyro');
        writeFileSync(binary, 'fixture');
        expect(resolveBinary({ root, platform, arch, env: {} })).toBe(binary);
        expect(resolveBinary({ root, platform, arch, env: { PYRO_BIN: '/custom/browser' } })).toBe('/custom/browser');
      }
    }
    expect(() => resolveBinary({ root, platform: 'win32', arch: 'x64', env: {} })).toThrow('connect()');
    expect(() => resolveBinary({ root, platform: 'darwin', arch: 'unknown', env: {} })).toThrow('no bundled browser');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('a release archive client takes the browser beside it over its bundled one', () => {
  const archive = mkdtempSync(join(tmpdir(), 'pyro-archive-'));
  try {
    const root = join(archive, 'pyro-client');
    const bundledDir = join(root, 'native', 'darwin-arm64');
    mkdirSync(bundledDir, { recursive: true });
    writeFileSync(join(bundledDir, 'pyro'), 'fixture');
    const beside = join(archive, 'pyro');
    writeFileSync(beside, 'fixture');
    expect(resolveBinary({ root, platform: 'darwin', arch: 'arm64', env: {} })).toBe(beside);
    expect(resolveBinary({ root, platform: 'darwin', arch: 'arm64', env: { PYRO_BIN: '/custom/browser' } })).toBe('/custom/browser');
    rmSync(beside);
    expect(resolveBinary({ root, platform: 'darwin', arch: 'arm64', env: {} })).toBe(join(bundledDir, 'pyro'));
    rmSync(join(root, 'native'), { recursive: true });
    expect(() => resolveBinary({ root, platform: 'darwin', arch: 'arm64', env: {} })).toThrow('Missing bundled browser');
  } finally { rmSync(archive, { recursive: true, force: true }); }
});

test('CLI forwards arguments and exit status to the selected browser', () => {
  const root = mkdtempSync(join(tmpdir(), 'pyro-cli-'));
  try {
    const binary = join(root, 'browser');
    writeFileSync(binary, '#!/usr/bin/env node\nconsole.log(JSON.stringify(process.argv.slice(2))); process.exitCode = 7;\n');
    chmodSync(binary, 0o755);
    const result = spawnSync('node', [join(import.meta.dir, '../dist/cli.js'), 'eval', '-e', 'console.log("hello world")'], {
      encoding: 'utf8', env: { ...process.env, PYRO_BIN: binary },
    });
    expect(result.status).toBe(7);
    expect(JSON.parse(result.stdout)).toEqual(['eval', '-e', 'console.log("hello world")']);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('launch reports a missing override without an unhandled spawn error', async () => {
  await expect(Pyro.launch({ bin: '/missing/pyro-browser' })).rejects.toThrow('ENOENT');
});

test('CLI forwards termination to the browser', async () => {
  const root = mkdtempSync(join(tmpdir(), 'pyro-signal-'));
  const binary = join(root, 'browser');
  writeFileSync(binary, '#!/usr/bin/env node\nprocess.on("SIGTERM", () => process.exit(23)); setInterval(() => {}, 1000); console.log("ready");\n');
  chmodSync(binary, 0o755);
  const child = spawn('node', [join(import.meta.dir, '../dist/cli.js')], {
    env: { ...process.env, PYRO_BIN: binary }, stdio: ['ignore', 'pipe', 'pipe'],
  });
  try {
    await once(child.stdout!, 'data', { signal: AbortSignal.timeout(3000) });
    const exited = once(child, 'exit', { signal: AbortSignal.timeout(3000) });
    child.kill('SIGTERM');
    expect((await exited)[0]).toBe(23);
  } finally {
    if (child.exitCode === null) child.kill('SIGTERM');
    rmSync(root, { recursive: true, force: true });
  }
});
