// The specs' config, for both runs:
// - `LP_CONFIG=test/playwright/playwright.config.ts bun test ./test/playwright/*.pw.ts`:
//   `@povio/lightning-panda/playwright` (our runner, expect and client);
// - `PW_ENGINE=chromium-cdp bunx playwright test -c test/playwright`: stock
//   Playwright driving the same lightningpanda over CDP (the parity check).
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { defineConfig } from '../../src/playwright/config';

const root = join(import.meta.dirname, '../../../..');
process.env.LIGHTNINGPANDA_BIN ??= [join(root, 'target/release/lightningpanda'), join(root, 'target/debug/lightningpanda')].find(existsSync);
const port = Number(process.env.LP_SITE_PORT ?? 4517);

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
