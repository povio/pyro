// Fixture specs record what ran, in order, for test/e2e-executor.test.ts.
import { appendFileSync } from 'node:fs';
let active = 0;
export const log = (event: string, data: Record<string, unknown> = {}) =>
  appendFileSync(process.env.E2E_LOG!, `${JSON.stringify({ event, t: performance.now(), ...data })}\n`);
/** Runs `fn` as one tracked test body: logs start and end with how many ran at once. */
export async function tracked(name: string, ms: number) {
  active++;
  log(`start ${name}`, { active });
  await Bun.sleep(ms);
  active--;
  log(`end ${name}`);
}
