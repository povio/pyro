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
// The arm64 browser is the fork-capable build and loads its patched
// JavaScriptCore from beside it (scripts/release/bundle-jsc.sh).
{
  const lib = new URL('native/darwin-arm64/lib/libJavaScriptCore.dylib', pkg);
  const header = statSync(lib, { throwIfNoEntry: false })?.isFile() ? readFileSync(lib).subarray(0, 8) : Buffer.alloc(0);
  if (header.length < 8 || header.readUInt32LE(0) !== 0xfeedfacf || header.readUInt32LE(4) !== 0x0100000c) {
    throw new Error(`Missing or not an arm64 library: ${fileURLToPath(lib)}. The arm64 browser must be the fork build with its patched JavaScriptCore (see docs/NPM_PUBLISHING.md).`);
  }
}
// Linux browsers are fork builds too, with the patched JavaScriptCore and its
// ICU in lib/ beside them (scripts/release/bundle-jsc-linux.sh).
for (const [arch, machine] of [['x64', 62], ['arm64', 183]]) {
  for (const rel of [`native/linux-${arch}/lightningpanda`, `native/linux-${arch}/lib/libJavaScriptCore.so`]) {
    const file = new URL(rel, pkg);
    if (!statSync(file, { throwIfNoEntry: false })?.isFile()) {
      throw new Error(`Missing ${fileURLToPath(file)}. Build both Linux browsers before packing (see docs/NPM_PUBLISHING.md).`);
    }
    const header = readFileSync(file).subarray(0, 20);
    // ELF magic, 64-bit, little-endian, then e_machine at offset 18.
    if (header.length < 20 || header.readUInt32BE(0) !== 0x7f454c46 || header[4] !== 2 || header[5] !== 1 || header.readUInt16LE(18) !== machine) {
      throw new Error(`Expected a Linux ${arch} ELF file: ${fileURLToPath(file)}`);
    }
  }
  chmodSync(new URL(`native/linux-${arch}/lightningpanda`, pkg), 0o755);
}
for (const name of ['LICENSE', 'NOTICE']) copyFileSync(new URL(name, root), new URL(name, pkg));
chmodSync(new URL('dist/cli.js', pkg), 0o755);
