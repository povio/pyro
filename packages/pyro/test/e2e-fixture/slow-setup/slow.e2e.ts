import { beforeAll, describe, test } from '@povio/pyro/e2e';
import { tracked } from '../log';
// While `a` sets up, every running test runs long: a2 and a3 must start when
// a's beforeAll is done, not when one of those long tests ends.
describe('long', () => { test('long', () => tracked('long', 600)); });
describe('a', () => {
  beforeAll(() => Bun.sleep(100));
  test('a1', () => tracked('a1', 600));
  for (const name of ['a2', 'a3']) test(name, () => tracked(name, 50));
});
