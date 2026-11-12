import { statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Shared by the CLI and launch(); never fall back to our own CLI on PATH. */
export function resolveBinary({
  platform = process.platform as string,
  arch = process.arch as string,
  root = fileURLToPath(new URL('../', import.meta.url)),
  env = process.env,
}: { platform?: string; arch?: string; root?: string; env?: NodeJS.ProcessEnv } = {}): string {
  if (env.PYRO_BIN) return env.PYRO_BIN;
  // A release archive unpacks the client beside the browser it ships (the
  // standard or the fork build); that one wins over the package's own.
  const sibling = join(root, '..', 'pyro');
  if (statSync(sibling, { throwIfNoEntry: false })?.isFile()) return sibling;
  const bundled = join(root, 'native', `${platform}-${arch}`, 'pyro');
  if (statSync(bundled, { throwIfNoEntry: false })?.isFile()) return bundled;
  if (!['darwin', 'linux'].includes(platform) || !['arm64', 'x64'].includes(arch)) {
    throw new Error(`Pyro has no bundled browser for ${platform}-${arch}. The browser supports macOS 14+ and Linux (glibc 2.39+) on arm64 and x64. Use Pyro.connect() for a remote browser, or set PYRO_BIN.`);
  }
  throw new Error(`Missing bundled browser: ${bundled}. Reinstall @povio/pyro or set PYRO_BIN to a built browser.`);
}
