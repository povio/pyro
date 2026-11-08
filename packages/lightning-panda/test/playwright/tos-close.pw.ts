// TOS's "closes an open Position from the Position overview", written as TOS
// writes it (`tos-flow.ts`, from bench/scripts/cdp-perf.mjs `pw.tos.close`):
// stock Playwright code, only the `test` import differs.
import { test } from './fixtures';
import { closeFlow, REPEAT } from './tos-flow';

test.describe.configure({ mode: 'parallel' });
for (let i = 0; i < REPEAT; i++) {
  test(`closes an open Position from the Position overview #${i}`, async ({ browser, request, baseURL }) => {
    await closeFlow(browser, request, baseURL!);
  });
}
