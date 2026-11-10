import { describe, test } from '@povio/lightning-panda/e2e';
import { tracked } from '../log';
describe('mixed', () => {
  test('t1', () => tracked('t1', 60));
  test('t2', () => tracked('t2', 60));
  test.serial('s3', () => tracked('s3', 60));
  test('t4', () => tracked('t4', 60));
  test('t5', () => tracked('t5', 60));
});
describe.serial('all serial', () => {
  for (const name of ['u1', 'u2', 'u3']) test(name, () => tracked(name, 30));
});
