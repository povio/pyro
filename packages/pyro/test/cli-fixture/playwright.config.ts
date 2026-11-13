// A TOS-shaped config: one project per directory, each listing its `.e2e.ts` files by name.
import { defineConfig } from '../../src/wright/config';

export default defineConfig({
  fullyParallel: false,
  workers: 2,
  timeout: 5_000,
  use: { baseURL: 'http://config.test' },
  projects: [
    { name: '', testDir: 'e2e', testMatch: ['root.e2e.ts', 'parallel.e2e.ts'], use: { baseURL: 'http://root.test' } },
    { name: '', testDir: 'e2e/sub', testMatch: ['sub.e2e.ts'], use: { baseURL: 'http://sub.test' } },
  ],
});
