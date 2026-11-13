// The ToS ("Shippy") stack the fidelity harness captures: ToS's own FE E2E
// setup (`scripts/fe-e2e/setup.ts` in the ToS checkout) started on its own: a
// run database copied from the migrated template, the compiled BE, the
// fixture worker and the gateway that serves the production FE build and
// proxies `/api`. Nothing of ToS is copied here: the setup, the fixtures and
// the sign-in tokens all come from the checkout at run time.
//
// Build the checkout first (in it: `bun install --frozen-lockfile`, then
// `FE_E2E_PORT=<port> bun scripts/fe-e2e/build.mts`; the build bakes the port
// into the FE).
//
//   bun bench/fidelity/tos-stack.ts --tos <ToS checkout> --port <port> --state <file>
//
// Writes the runtime (gateway URL, RPC token) to `--state` once the stack is
// up and keeps it running until SIGINT/SIGTERM, then tears it down (the run
// database is dropped; the cluster keeps running: stop it with
// `FE_E2E_PORT=<port> bun e2e:postgres:stop` in the checkout).
import { rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

export type StackRuntime = { app: string; rpcToken: string; port: number; tos: string };
type Setup = { setup(): Promise<{ app: string; rpcToken: string }>; teardown(): Promise<void> };

/** Starts the stack in this process; `stop` tears it down. */
export async function startStack(tos: string, port: number): Promise<{ runtime: StackRuntime; stop: () => Promise<void> }> {
  process.env.FE_E2E_PORT = String(port);
  process.env.TZ ??= 'Europe/Ljubljana';
  // No PDF/EML workers (they would start a second browser) and no mail capture: the screens need neither.
  process.env.FE_E2E_GENERATION ??= '0';
  process.env.FE_E2E_EMAIL_SINK ??= '0';
  const { default: setup } = (await import(join(resolve(tos), 'scripts/fe-e2e/setup.ts'))) as { default: Setup };
  const started = await setup.setup();
  let stopped: Promise<void> | undefined;
  return {
    runtime: { app: started.app, rpcToken: started.rpcToken, port, tos: resolve(tos) },
    stop: () => (stopped ??= setup.teardown()),
  };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const opt = (name: string) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
  const tos = opt('--tos');
  const port = Number(opt('--port'));
  const state = opt('--state');
  if (!tos || !Number.isInteger(port) || !state) {
    console.error('usage: bun bench/fidelity/tos-stack.ts --tos <ToS checkout> --port <port> --state <file>');
    process.exit(2);
  }
  const { runtime, stop } = await startStack(tos, port);
  writeFileSync(state, JSON.stringify(runtime, null, 2));
  console.log(`ToS stack ready at ${runtime.app}; state in ${state}`);
  const shutdown = async () => {
    rmSync(state, { force: true });
    await stop();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);
  await new Promise(() => {});
}
