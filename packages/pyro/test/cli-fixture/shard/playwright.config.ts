// Sharding: `--shard i/n` splits the tests project files hold as Playwright
// does (whole files, or single tests when fully parallel); the setup project
// they depend on runs in every shard. `SHARD_IN_CONFIG` sets `shard` here instead.
import { defineConfig } from '../../../src/wright/config';

export default defineConfig({
  testDir: '.',
  workers: 2,
  ...(process.env.SHARD_IN_CONFIG ? { shard: { current: 2, total: 2 } } : {}),
  projects: [
    { name: 'setup', testMatch: /.*\.setup\.ts/ },
    { name: 'tests', testMatch: '*.e2e.ts', dependencies: ['setup'] },
  ],
});
