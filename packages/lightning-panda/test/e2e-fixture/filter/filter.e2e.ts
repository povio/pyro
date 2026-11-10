import { describe, test } from '@povio/lightning-panda/e2e';
import { log } from '../log';
describe('group', () => {
  test('alpha one', () => log('alpha one'));
  test('beta two', () => log('beta two'));
  test.skip('skipped', () => log('skipped'));
  test.todo('later');
  test.only('focused', () => log('focused'));
});
describe.only('focused group', () => {
  test('inside', () => log('inside'));
});
