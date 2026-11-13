import { test as base } from '../../../src/wright/index';

const test = base.extend<{}, { tornDownBadly: string }>({
  tornDownBadly: [
    async ({}, use) => {
      await use('x');
      throw new Error('worker fixture teardown failed');
    },
    { scope: 'worker' },
  ],
});

test.describe('an afterAll that throws', () => {
  test.afterAll(async () => {
    throw new Error('afterAll failed');
  });
  test('passes, and is the last of its describe', async ({ tornDownBadly }) => {
    void tornDownBadly;
  });
});
