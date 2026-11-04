import { appendFileSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export function releaseInfo(tag, packageVersion, cargoVersion) {
  const match = /^v((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(-rc(?:\.(?:0|[1-9]\d*))?)?)$/.exec(tag ?? '');
  if (!match || match[0] !== tag) throw new Error('Release tags must be vX.Y.Z, vX.Y.Z-rc, or vX.Y.Z-rc.N.');
  const version = match[1];
  if (version !== packageVersion || version !== cargoVersion) {
    throw new Error(`Tag ${version}, package.json ${packageVersion}, and Cargo.toml ${cargoVersion} must match exactly.`);
  }
  return { version, distTag: match[2] ? 'next' : 'latest' };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = new URL('../../', import.meta.url);
  const pkg = JSON.parse(readFileSync(new URL('packages/lightning-panda/package.json', root), 'utf8'));
  const cargo = readFileSync(new URL('Cargo.toml', root), 'utf8');
  const cargoVersion = cargo.match(/\[workspace\.package\][\s\S]*?^version = "([^"]+)"/m)?.[1];
  const info = releaseInfo(process.argv[2], pkg.version, cargoVersion);
  console.log(JSON.stringify(info));
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `version=${info.version}\ndist-tag=${info.distTag}\n`);
  }
}
