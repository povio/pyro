import { expect, test } from '../../../src/playwright/index';

test('uses its project', async ({ baseURL }) => {
  expect(baseURL).toBe('http://root.test');
});
test('skips at run time', async () => {
  test.skip(true, 'not today');
});
test.skip('skipped when declared', async () => {});
test('fails', async () => {
  expect(1).toBe(2);
});
