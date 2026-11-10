import { afterAll, beforeAll, describe, test } from '@povio/lightning-panda/e2e';
import { log } from '../log';
describe('broken setup', () => {
  beforeAll(() => { throw new Error('setup exploded'); });
  afterAll(() => log('afterAll still runs'));
  test('x', () => log('x ran'));
  test('y', () => log('y ran'));
});
test('unaffected', () => log('unaffected'));
