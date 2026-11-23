import { execFileSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// The former Linux and PGO workflow path filters, owned in one place. A PGO
// check includes the Linux packaging/client checks, so it supersedes the plain
// Linux build when both match. WPT still tests its independent wptiter build.
export function plan(paths) {
  const linux = paths.some((p) => /^(?:scripts\/(?:linux|jsc)\/|crates\/pyro-jsc-sys\/)/.test(p)
    || ['.github/workflows/linux.yml', '.github/workflows/linux-build.yml',
      '.github/workflows/linux-platform.yml', '.github/workflows/jsc-linux.yml',
      '.github/workflows/jsc.yml', 'scripts/release/bundle-icu-linux.sh'].includes(p));
  const pgo = paths.some((p) => /^(?:\.github\/actions\/pgo-build\/|scripts\/(?:pgo|jsc)\/|scripts\/macos\/linker\/|crates\/pyro-jsc-sys\/webkit\/)/.test(p)
    || ['.github/workflows/pgo.yml', '.github/workflows/linux-build.yml',
      '.github/workflows/linux-platform.yml', '.github/workflows/jsc-linux.yml',
      '.github/workflows/jsc.yml', 'crates/pyro-cdp/src/pgo_training.rs'].includes(p));
  const orchestration = paths.some((p) => p === '.github/workflows/wpt.yml' || p.startsWith('scripts/ci/'));
  return { 'linux-check': linux || pgo || orchestration, 'pgo-check': pgo || orchestration };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const base = process.argv[2];
  if (!base || !/^[a-f0-9]{40}$/.test(base)) throw new Error('Expected a base commit SHA');
  const paths = execFileSync('git', ['diff', '--name-only', '-z', base, 'HEAD'], { encoding: 'utf8' }).split('\0').filter(Boolean);
  const outputs = Object.entries(plan(paths)).map(([k, v]) => `${k}=${v}`).join('\n') + '\n';
  process.stdout.write(outputs);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, outputs);
}
