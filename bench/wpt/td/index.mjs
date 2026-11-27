// The runner's testdriver command registry (testdriver.mjs routes every message of testdriver-vendor.js through it).
//
// ADDING A COMMAND GROUP (one file and one line here, one line in testdriver-vendor.js):
//   1. Create `td/<group>.mjs` exporting `commands`, an object of `async (ctx, args) => result` by command name
//      (`'get_named_cookie'`, or a dotted name for the bidi modules: `'bidi.emulation.set_locale_override'`).
//      `args` is the array the page passed (`def` below); `result` must be JSON (`undefined` is fine). To fail, throw:
//      the page's promise rejects with the message. A command the browser cannot really carry out must NOT be
//      registered (testdriver.js then rejects it as "not implemented"): never a handler that resolves and does nothing.
//   2. Add `import * as <group> from './<group>.mjs';` and `<group>` to GROUPS below. A duplicate name is an error.
//   3. In testdriver-vendor.js add `def('<name>')` (or `def('<name>', { elements: [0] })` when argument 0 is an
//      element: it reaches the runner as `{ element: id }` and `await ctx.element(id)` returns its ElementHandle).
//      Windows in the arguments (the `context` parameter, `contexts: [popup]`) reach the runner as `{ window: id }`;
//      `await ctx.windowPage(arg)` finds the Playwright page of a window argument.
//
// `ctx` (built per message in testdriver.mjs):
//   context, page, frame    the browser context, and the page and frame whose document sent the message
//   off()                   async, the frame's {x, y} offset in its page (what the input commands add to coordinates)
//   session()               async, the page's CDPSession (Playwright's newCDPSession, one per page, reused);
//   sessionOf(page)         the same for any page of the context
//   state(key, init)        per-page state that outlives one message (Map by key, created with `init()`)
//   shared(key, init)       the same per browser context (e.g. a subscription table)
//   emit(frame, name, data) delivers an event to the page-side `window.__pyroTdEvent(name, data)` (testdriver-vendor.js
//                           `onEvent(name, fn)` registers listeners); a frame that is gone is ignored
//   element(id), windowPage(arg), see above
import * as core from './core.mjs';
import * as cookies from './cookies.mjs';
import * as bidi from './bidi.mjs';
import * as report from './report.mjs';
import * as pressure from './pressure.mjs';
import * as webauthn from './webauthn.mjs';
import * as sensors from './sensors.mjs';
import * as aria from './aria.mjs';

const GROUPS = [core, cookies, bidi, report, pressure, webauthn, sensors, aria];

export const commands = new Map();
for (const g of GROUPS) {
  for (const [name, fn] of Object.entries(g.commands)) {
    if (commands.has(name)) throw new Error(`testdriver: command ${name} is registered twice`);
    commands.set(name, fn);
  }
}

// Runs the command of a page's message: `{ cmd, args }` from `def`, or (the core commands, as they always were) `{ cmd,
// ...fields }`, which the handler then receives whole.
export async function run(ctx, c) {
  const fn = commands.get(c.cmd);
  if (!fn) throw new Error(`testdriver: ${c.cmd} is not supported`);
  return fn(ctx, 'args' in c ? c.args : c);
}
