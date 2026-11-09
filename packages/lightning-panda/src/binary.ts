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
  if (env.LIGHTNINGPANDA_BIN) return env.LIGHTNINGPANDA_BIN;
  // A release archive unpacks the client beside the browser it ships (the
  // standard or the fork build); that one wins over the package's own.
  const sibling = join(root, '..', 'lightningpanda');
  if (statSync(sibling, { throwIfNoEntry: false })?.isFile()) return sibling;
  const bundled = join(root, 'native', `${platform}-${arch}`, 'lightningpanda');
  if (statSync(bundled, { throwIfNoEntry: false })?.isFile()) return bundled;
  if (!['darwin', 'linux'].includes(platform) || !['arm64', 'x64'].includes(arch)) {
    throw new Error(`Lightningpanda has no bundled browser for ${platform}-${arch}. The browser supports macOS 14+ and Linux (glibc 2.39+) on arm64 and x64. Use Lightningpanda.connect() for a remote browser, or set LIGHTNINGPANDA_BIN.`);
  }
  throw new Error(`Missing bundled browser: ${bundled}. Reinstall @povio/lightning-panda or set LIGHTNINGPANDA_BIN to a built browser.`);
}
