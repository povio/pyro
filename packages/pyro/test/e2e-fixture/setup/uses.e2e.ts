import { expect, setupResult, test } from '@povio/pyro/e2e';
import { log } from '../log';
const runtime = setupResult<{ token: string }>(); // readable while the spec loads
test('reads the setup result', () => { expect(runtime.token).toBe('from-setup'); log('read'); });
test('long', async () => { log('long started'); await Bun.sleep(Number(process.env.E2E_LONG ?? 0)); });
