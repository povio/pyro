// The specs' config, for both runs:
// - `PYRO_CONFIG=test/wright/playwright.config.ts bun test ./test/wright/*.pw.ts`:
//   `@povio/pyro/wright` (our runner, expect and client);
// - `PW_ENGINE=chromium-cdp bunx playwright test -c test/wright`: stock
//   Playwright driving the same pyro over CDP (the parity check).
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { defineConfig } from '../../src/wright/config';

const root = join(import.meta.dirname, '../../../..');
process.env.PYRO_BIN ??= [join(root, 'target/release/pyro'), join(root, 'target/debug/pyro')].find(existsSync);
const port = Number(process.env.PYRO_SITE_PORT ?? 4517);

export default defineConfig({
  testDir: '.',
  testMatch: '*.pw.ts',
  fullyParallel: true,
  workers: Number(process.env.WORKERS ?? 2),
  reporter: process.env.CI ? 'dot' : 'list',
  use: { baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    command: `node ${join(root, 'bench/scripts/cdp-perf-site.mjs')} ${port}`,
    url: `http://127.0.0.1:${port}/simple`,
    reuseExistingServer: true,
  },
});
