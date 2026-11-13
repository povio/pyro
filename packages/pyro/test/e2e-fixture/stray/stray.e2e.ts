import { test } from '@povio/pyro/e2e';
test('leaves a rejection behind', () => { void Promise.reject(new Error('nobody awaited this')); });
test('runs after it', () => Bun.sleep(50));
