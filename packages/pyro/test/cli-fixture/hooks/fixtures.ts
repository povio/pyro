import { test as base } from '../../../src/wright/index';

export { expect } from '../../../src/wright/index';
let setups = 0;
export const test = base.extend<{}, { shared: { id: number } }>({
  shared: [
    async ({}, use) => {
      console.log('WORKER FIXTURE SETUP');
      await use({ id: ++setups });
      console.log('WORKER FIXTURE TEARDOWN');
    },
    { scope: 'worker' },
  ],
});
