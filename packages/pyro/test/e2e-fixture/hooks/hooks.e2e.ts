import { afterAll, afterEach, beforeAll, beforeEach, describe, test } from '@povio/pyro/e2e';
import { log } from '../log';
beforeAll(() => log('file beforeAll'));
afterAll(() => log('file afterAll'));
beforeEach(() => log('file beforeEach'));
afterEach(() => log('file afterEach'));
describe('outer', () => {
  beforeAll(async () => { await Bun.sleep(30); log('outer beforeAll'); });
  afterAll(() => log('outer afterAll'));
  beforeEach(() => log('outer beforeEach'));
  afterEach(() => log('outer afterEach'));
  test('a', () => log('test a'));
  describe('inner', () => {
    beforeAll(() => log('inner beforeAll'));
    afterAll(() => log('inner afterAll'));
    test('b', () => log('test b'));
  });
});
