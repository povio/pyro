// Project dependencies as Playwright's docs set them up: the setup project
// signs in and saves the storage state, the app project depends on it and
// starts from that file, and a teardown project runs after the app project.
// The test starts the server (`DEPS_BASE_URL`) and names the file (`DEPS_STATE`).
import { defineConfig } from '../../../src/wright/config';

export default defineConfig({
  testDir: '.',
  workers: 2,
  timeout: 10_000,
  globalSetup: './global-setup.ts',
  use: { baseURL: process.env.DEPS_BASE_URL },
  projects: [
    { name: 'setup', testMatch: /.*\.setup\.ts/, teardown: 'cleanup' },
    { name: 'cleanup', testMatch: /.*\.teardown\.ts/ },
    { name: 'app', testMatch: '*.e2e.ts', dependencies: ['setup'], use: { storageState: process.env.DEPS_STATE } },
  ],
});
