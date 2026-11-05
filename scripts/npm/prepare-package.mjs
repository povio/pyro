import { chmodSync, copyFileSync, readFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = new URL('../../', import.meta.url);
const pkg = new URL('packages/lightning-panda/', root);
// Prevent publishing a client-only or incomplete multi-architecture package.
for (const arch of ['arm64', 'x64']) {
  const binary = new URL(`native/darwin-${arch}/lightningpanda`, pkg);
  if (!statSync(binary, { throwIfNoEntry: false })?.isFile()) {
    throw new Error(`Missing ${fileURLToPath(binary)}. Build both macOS binaries before packing (see docs/NPM_PUBLISHING.md).`);
  }
  // GitHub artifact downloads do not retain executable permissions.
  chmodSync(binary, 0o755);
  const header = readFileSync(binary).subarray(0, 8);
  const cpu = arch === 'arm64' ? 0x0100000c : 0x01000007;
  if (header.length < 8 || header.readUInt32LE(0) !== 0xfeedfacf || header.readUInt32LE(4) !== cpu) {
    throw new Error(`Expected a macOS ${arch} Mach-O executable: ${fileURLToPath(binary)}`);
  }
}
for (const name of ['LICENSE', 'NOTICE']) copyFileSync(new URL(name, root), new URL(name, pkg));
chmodSync(new URL('dist/cli.js', pkg), 0o755);
