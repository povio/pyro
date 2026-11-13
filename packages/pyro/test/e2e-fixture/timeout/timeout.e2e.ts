import { test } from '@povio/pyro/e2e';
test('too slow', () => Bun.sleep(400));
test('slow with its own timeout', () => Bun.sleep(300), 2000);
test('options timeout', () => Bun.sleep(300), { timeout: 2000 });
