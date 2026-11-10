import { describe, test } from '@povio/lightning-panda/e2e';
import { tracked } from '../log';
describe('one', () => {
  for (let i = 0; i < 4; i++) test('t' + i, () => tracked('one/t' + i, 100));
});
