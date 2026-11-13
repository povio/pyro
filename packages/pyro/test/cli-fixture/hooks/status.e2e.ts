import { expect, test as base } from '../../../src/wright/index';

const test = base.extend<{ watcher: void }>({
  watcher: [
    async ({}, use, info) => {
      await use();
      console.log(`FIXTURE TEARDOWN ${info.title}: ${info.status}`);
    },
    { auto: true },
  ],
});

test.afterEach(async ({}, info) => {
  console.log(`AFTEREACH ${info.title}: ${info.status}, expected ${info.expectedStatus}, ${info.errors.length} error(s)${info.error ? `, ${info.error.message.split('\n')[0]}` : ''}`);
});

test('fails', async () => {
  expect(1).toBe(2);
});
test('times out', async () => {
  test.setTimeout(200);
  await new Promise((r) => setTimeout(r, 1_000));
});
test('fails as expected', async () => {
  test.fail();
  expect(1).toBe(2);
});
test('passes', async () => {});
