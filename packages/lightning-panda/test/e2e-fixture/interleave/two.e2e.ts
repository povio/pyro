import { describe, test } from '@povio/lightning-panda/e2e';
import { tracked } from '../log';
describe('two', () => {
  for (let i = 0; i < 4; i++) test('t' + i, () => tracked('two/t' + i, 100));
});
