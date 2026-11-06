// The one import to swap: `@povio/lightning-panda/test` (default) or stock
// Playwright over CDP to a lightningpanda launched per worker (PW_ENGINE=chromium-cdp).
import * as pw from 'playwright/test';
import * as lp from '../../src/playwright/test';
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

export const test = process.env.PW_ENGINE === 'chromium-cdp' ? stock : lp.test;
export const expect = pw.expect;

export const isStock = process.env.PW_ENGINE === 'chromium-cdp';
