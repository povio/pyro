// `@povio/lightning-panda/test`: `@playwright/test` with its browser, context
// and page fixtures on lightningpanda. Swap the import and keep the runner:
//
//   import { test, expect } from '@povio/lightning-panda/test'; // was '@playwright/test'
//
// Everything else (`expect`, `describe`, hooks, the `request` fixture,
// config, reporters, workers) is Playwright's own. Each worker launches one
// lightningpanda (or connects to `LIGHTNINGPANDA_WS`) and every test gets a
// fresh context and page on it.
import { test as base } from 'playwright/test';
import type { Browser as PwBrowser, BrowserContext as PwContext, Page as PwPage } from 'playwright/test';
import { Lightningpanda } from '../index.js';
import { Browser, setTestDefaults, type ContextOptions } from './browser.js';

// Playwright's own exports and types: `Page`, `Locator`, … stay its types.
export * from 'playwright/test';

export type LightningpandaOptions = {
  /** Connect to a running `lightningpanda serve` (default: `$LIGHTNINGPANDA_WS`, else launch one per worker). */
  wsEndpoint?: string;
  /** The binary to launch (default: `$LIGHTNINGPANDA_BIN`, else the bundled one). */
  bin?: string;
  /** Extra `lightningpanda serve` arguments. */
  args?: string[];
};

type WorkerFixtures = { lightningpanda: LightningpandaOptions; _lightningpanda: Lightningpanda };

export const test = base.extend<{ _lpContextDefaults: void }, WorkerFixtures>({
  lightningpanda: [{}, { scope: 'worker', option: true }],
  _lightningpanda: [
    async ({ lightningpanda: o }, use) => {
      const ws = o.wsEndpoint ?? process.env.LIGHTNINGPANDA_WS;
      const lp = ws ? await Lightningpanda.connect(ws) : await Lightningpanda.launch({ bin: o.bin, args: o.args });
      try {
        await use(lp);
      } finally {
        lp.close();
      }
    },
    { scope: 'worker' },
  ],
  browser: [
    async ({ _lightningpanda }, use) => {
      const browser = new Browser(_lightningpanda, false);
      await use(browser as unknown as PwBrowser);
      await browser.close();
    },
    { scope: 'worker' },
  ],
  // `browser.newContext()` in a test starts from the config's `use`, as in
  // Playwright. A worker runs one test at a time, so the defaults are the
  // worker's (and this fixture launches nothing for tests without a page).
  _lpContextDefaults: [
    async ({ _combinedContextOptions, actionTimeout, navigationTimeout, testIdAttribute }: any, use) => {
      setTestDefaults({
        options: _combinedContextOptions as ContextOptions,
        timeout: actionTimeout || undefined,
        navigationTimeout: navigationTimeout || undefined,
        testIdAttribute: testIdAttribute || 'data-testid',
      });
      await use();
      setTestDefaults(undefined);
    },
    { auto: true },
  ],
  context: async ({ browser, _lpContextDefaults }: any, use) => {
    const context = await (browser as Browser).newContext();
    await use(context as unknown as PwContext);
    await context.close();
  },
  page: async ({ context }, use) => {
    await use((await context.newPage()) as unknown as PwPage);
  },
});
