import { expect, test } from '../../../../src/playwright/index';

test('uses its own project', async ({ baseURL }) => {
  expect(baseURL).toBe('http://sub.test');
});
