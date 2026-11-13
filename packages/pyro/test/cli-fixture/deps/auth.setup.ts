import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { expect, test as setup } from '../../../src/wright/index';

setup('signs in', async ({ request }) => {
  // Slow on purpose: an app test running alongside would find no file.
  await new Promise((r) => setTimeout(r, 300));
  if (process.env.DEPS_FAIL_SETUP) throw new Error('sign-in failed');
  expect((await request.get('/login')).ok()).toBe(true);
  mkdirSync(dirname(process.env.DEPS_STATE!), { recursive: true });
  await request.storageState({ path: process.env.DEPS_STATE! });
});
