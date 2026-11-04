import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: /suite\.spec\.mjs/,
  workers: 1,
  retries: 0,
  timeout: 20000,
  expect: { timeout: 5000 },
  snapshotPathTemplate: '{testDir}/baseline/{arg}{ext}',
  outputDir: process.env.OUT ? `${process.env.OUT}/test-results` : undefined,
  reporter: [['list'], ['json', { outputFile: `${process.env.OUT ?? 'results'}/${process.env.TARGET ?? 'bin'}.json` }]],
  use: { viewport: { width: 1280, height: 720 } },
});
