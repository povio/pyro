import { appendFileSync } from 'node:fs';
import type { Setup } from '@povio/pyro/e2e';
const log = (event: string) => appendFileSync(process.env.E2E_LOG!, `${JSON.stringify({ event })}\n`);
export default {
  async setup() { log('setup'); return { token: 'from-setup' }; },
  async teardown(result) { log(`teardown ${result.token}`); },
} satisfies Setup<{ token: string }>;
