// Module state set in beforeEach, as TOS files do: each worker has its own.
import { expect, test } from '../../../src/wright/index';

test.describe('parallel', () => {
  test.describe.configure({ mode: 'parallel' });
  let mine: string | undefined;
  test.beforeEach(async ({}, info) => {
    expect(mine).toBeUndefined();
    mine = info.title;
  });
  test.afterEach(async () => {
    mine = undefined;
  });
  for (let i = 0; i < 4; i++) {
    test(`p${i}`, async ({}, info) => {
      await new Promise((r) => setTimeout(r, 150));
      expect(mine).toBe(info.title);
      console.log(`ran p${i} in worker ${info.workerIndex}`);
    });
  }
});
