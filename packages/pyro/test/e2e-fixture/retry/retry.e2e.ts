import { afterEach, beforeEach, browser, evaluate, test } from '../browser/sdk';
import { log } from '../log';

let runs = 0;
test('passes on its second run', () => {
  runs++;
  log('flaky', { run: runs });
  if (runs < 2) throw new Error(`run ${runs} fails`);
}, { retries: 2 });

test('fails every run', () => {
  log('always');
  throw new Error('never passes');
}, { retries: 1 });

test('skips itself at run time', () => {
  log('before skip');
  test.skip(true, 'not on this backend');
  log('after skip');
});

test('a false condition does not skip', () => {
  test.skip(false, 'never');
  log('ran on');
});

beforeEach(() => {
  if (process.env.E2E_SKIP_IN_HOOK) test.skip();
});
afterEach(() => log('afterEach'));

test('a failure screenshots its page', async () => {
  const page = await (await browser()).page();
  await page.run([evaluate('document.body.innerHTML = "<h1 style=\\"color:red\\">failed here</h1>"')]);
  throw new Error('fails with a page open');
});

test('a failure screenshots the page it disposed', async () => {
  // `await using` closes the page while the error propagates: the screenshot must still see it.
  await using page = await (await browser()).page();
  await page.run([evaluate('document.body.innerHTML = "<h1>disposed here</h1>"')]);
  throw new Error('fails with an await using page');
});
