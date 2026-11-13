// Hooks, worker fixtures and testInfo in one worker: a worker fixture two
// files share, a file's afterAll that sees its tests' browser (`use.pyro`
// set), a beforeAll that asks for a test's page, an afterAll and a worker
// fixture teardown that throw, and the status afterEach hooks and fixture
// teardown see.
import { defineConfig } from '../../../src/wright/config';

export default defineConfig({
  testDir: '.',
  testMatch: '*.e2e.ts',
  workers: 1,
  timeout: 5_000,
  use: { pyro: { args: [] } },
});
