// Stock Playwright (its runner, expect and client) over CDP to a lightningpanda
// launched per worker: the parity run, `PW_ENGINE=chromium-cdp bunx playwright test -c test/playwright`.
import * as pw from 'playwright/test';
import { Lightningpanda } from '../../src/index';

const stock = pw.test.extend<{}, { _lp: Lightningpanda; lightningpanda: object }>({
  lightningpanda: [{}, { scope: 'worker', option: true }],
  _lp: [
    async ({}, use) => {
      const browser = await Lightningpanda.launch({ bin: process.env.LIGHTNINGPANDA_BIN });
      await use(browser);
      browser.close();
    },
    { scope: 'worker' },
  ],
  browser: [
    async ({ playwright, _lp }, use) => {
      const url = (_lp as any).wsUrl as string;
      const browser = await playwright.chromium.connectOverCDP(url);
      await use(browser);
      await browser.close();
    },
    { scope: 'worker' },
  ],
});

export const test = stock;
export const expect = pw.expect;
