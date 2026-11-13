import { existsSync, rmSync } from 'node:fs';
import { test } from '../../../src/wright/index';

test('removes the storage state', async () => {
  console.log(`TEARDOWN finds the state ${existsSync(process.env.DEPS_STATE!) ? 'present' : 'missing'}`);
  rmSync(process.env.DEPS_STATE!, { force: true });
});
