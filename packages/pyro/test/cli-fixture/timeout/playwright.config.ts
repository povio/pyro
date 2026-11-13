// A test that times out while its page waits, and the `screenshot` option:
// the page stays open for afterEach hooks, the timeout names the action it
// waited on, screenshots go to `outputDir` (`TIMEOUT_OUT`) as asked by
// `TIMEOUT_SHOT`. stuck.e2e.ts hangs its browser for good: run it alone.
import { defineConfig } from '../../../src/wright/config';

export default defineConfig({
  testDir: '.',
  testMatch: '*.e2e.ts',
  workers: 1,
  timeout: 5_000,
  outputDir: process.env.TIMEOUT_OUT,
  use: { screenshot: (process.env.TIMEOUT_SHOT ?? 'off') as 'off' },
});
