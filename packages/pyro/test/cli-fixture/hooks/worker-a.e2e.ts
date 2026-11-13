import { test } from './fixtures';

test('one file uses the worker fixture', async ({ shared }) => {
  console.log(`A sees worker fixture ${shared.id}`);
});
