// The one import the specs share: `@povio/pyro/wright` (run with
// `bun test`), or stock Playwright over CDP for the parity run
// (`PW_ENGINE=chromium-cdp`, run with `bunx playwright test -c test/wright`).
import * as lp from '../../src/wright/index';

export const isStock = process.env.PW_ENGINE === 'chromium-cdp';
const stock = isStock ? await import('./stock') : undefined;
export const test: typeof lp.test = stock ? (stock.test as any) : lp.test;
export const expect: typeof lp.expect = stock ? (stock.expect as any) : lp.expect;
