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
// The arm64 browser is the fork-capable build: the patched JavaScriptCore is
// linked into it (a static archive, scripts/jsc/build.sh). A library left
// beside it is from a build before that: the browser would not load it.
{
  const lib = new URL('native/darwin-arm64/lib/libJavaScriptCore.dylib', pkg);
  if (statSync(lib, { throwIfNoEntry: false })) {
    throw new Error(`Stale ${fileURLToPath(lib)}: the arm64 browser links JavaScriptCore statically. Remove native/darwin-arm64/lib and stage the current build (see docs/NPM_PUBLISHING.md).`);
  }
}
// Linux browsers are fork builds too, with the ICU libraries they load in lib/
// beside them (scripts/release/bundle-icu-linux.sh).
for (const [arch, machine] of [['x64', 62], ['arm64', 183]]) {
  const stale = new URL(`native/linux-${arch}/lib/libJavaScriptCore.so`, pkg);
  if (statSync(stale, { throwIfNoEntry: false })) {
    throw new Error(`Stale ${fileURLToPath(stale)}: the Linux browsers link JavaScriptCore statically (see docs/NPM_PUBLISHING.md).`);
  }
  for (const rel of [`native/linux-${arch}/lightningpanda`]) {
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
