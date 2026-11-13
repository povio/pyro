import { afterAll, beforeAll, defer, describe, test, testInfo } from '@povio/pyro/e2e';
import { log } from '../log';
describe('scoped', () => {
  beforeAll(() => { defer(() => log('describe defer 1')); defer(() => log('describe defer 2')); });
  afterAll(() => log('describe afterAll'));
  test('t', () => { defer(() => log('test defer')); log(`in ${testInfo()!.titlePath.join(' > ')}`); });
});
