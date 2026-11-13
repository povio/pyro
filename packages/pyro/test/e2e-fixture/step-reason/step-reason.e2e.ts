// A failing step under the default test timeout: the step's own timeout (5 s)
// fires first, so the run reports the step and why it was waiting.
import { browser, test } from '../browser/sdk';
import { css } from '@povio/pyro';
test('a missing element reports its step', async () => {
  await using page = await (await browser()).page({ url: 'data:text/html,<p>hi</p>' });
  await page.run([css('#nope').click()]);
});
