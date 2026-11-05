import { Lightningpanda, button, check, evaluate, type StepLike } from '@povio/lightning-panda';

const steps: StepLike[] = [button('Save').click(), check(button('Save')).visible(), evaluate('1', 'one')];
async function scenario(browser: Lightningpanda) {
  const page = await browser.page();
  const result = await page.run(steps);
  await page[Symbol.asyncDispose]();
  return result.values.one;
}
void scenario;
