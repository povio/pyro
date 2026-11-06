// Playwright's runner on lightningpanda: `bunx playwright test -c test/playwright`.
// The specs import `test` from `fixtures.ts`, which is `@povio/lightning-panda/test`
// unless PW_ENGINE=chromium-cdp, which runs the same specs with stock
// Playwright connected over CDP to the same lightningpanda (for comparison).
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { defineConfig } from 'playwright/test';

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
