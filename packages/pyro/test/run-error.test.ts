import { expect, test } from 'bun:test';
import { RunError } from '../src/index';

test('RunError names itself, so runners print RunError: step …', () => {
  const e = new RunError('step 2 (Click): timed out', 2);
  expect(e.name).toBe('RunError');
  expect(String(e)).toBe('RunError: step 2 (Click): timed out');
  expect(e).toBeInstanceOf(Error);
  expect(e.step).toBe(2);
});
