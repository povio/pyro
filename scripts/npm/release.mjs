import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';

export function releaseInfo(tag) {
  const match = /^v((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(-rc(?:\.(?:0|[1-9]\d*))?)?)$/.exec(tag ?? '');
  if (!match || match[0] !== tag) throw new Error('Release tags must be vX.Y.Z, vX.Y.Z-rc, or vX.Y.Z-rc.N.');
  const version = match[1];
  return { version, distTag: match[2] ? 'next' : 'latest' };
}

/** Stamp the CI checkout; Cargo updates its own lockfile in the release job. */
export function stampRelease(tag, root) {
  const info = releaseInfo(tag);
  const packagePath = join(root, 'packages/lightning-panda/package.json');
  const cargoPath = join(root, 'Cargo.toml');
  const pkg = JSON.parse(readFileSync(packagePath, 'utf8'));
  const lines = readFileSync(cargoPath, 'utf8').split('\n');
  const start = lines.findIndex(line => line.trim() === '[workspace.package]');
  const versionLines = [];
  if (start >= 0) {
    for (let i = start + 1; i < lines.length && !/^\s*\[/.test(lines[i]); i++) {
      if (/^\s*version\s*=\s*"[^"]+"/.test(lines[i])) versionLines.push(i);
    }
  }
  if (versionLines.length !== 1) throw new Error('Expected exactly one version in Cargo.toml [workspace.package].');
  const index = versionLines[0];
  lines[index] = lines[index].replace(/^(\s*version\s*=\s*")[^"]+(".*)$/, (_, before, after) => `${before}${info.version}${after}`);
  pkg.version = info.version;
  writeFileSync(packagePath, `${JSON.stringify(pkg, null, 2)}\n`);
  writeFileSync(cargoPath, lines.join('\n'));
  return info;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length > 4 || (process.argv[3] && process.argv[3] !== '--write')) {
    throw new Error('Usage: node scripts/npm/release.mjs vX.Y.Z[-rc.N] [--write]');
  }
  const info = process.argv[3] === '--write'
    ? stampRelease(process.argv[2], fileURLToPath(new URL('../../', import.meta.url)))
    : releaseInfo(process.argv[2]);
  console.log(JSON.stringify(info));
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `version=${info.version}\ndist-tag=${info.distTag}\n`);
  }
}
