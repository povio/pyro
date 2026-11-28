// The runner's half of testdriver (testdriver-vendor.js is the page's half; README "testdriver"): a binding on every
// browser context that performs what the page asks over CDP (Playwright's input APIs, CDP methods), as wptrunner
// performs WebDriver commands for Chrome. The commands live in td/ (td/index.mjs: how a group is added); this file
// only routes a message of the page to its handler with the context it needs.
import { run } from './td/index.mjs';
import { frameOffset } from './td/core.mjs';

export async function installTestdriver(context) {
  const sessions = new WeakMap(), pageState = new WeakMap(), contextState = new Map();
  const sessionOf = async (p) => sessions.get(p) ?? sessions.set(p, await context.newCDPSession(p)).get(p);
  const keyed = (map, key, init) => (map.has(key) ? map.get(key) : map.set(key, init()).get(key));
  await context.exposeBinding('__pyroTestdriver', async ({ page, frame }, json) => {
    try {
      const c = JSON.parse(json);
      let off;
      const ctx = {
        context, page, frame,
        off: () => (off ??= frameOffset(frame)),
        session: () => sessionOf(page),
        sessionOf,
        state: (key, init) => keyed(keyed(pageState, page, () => new Map()), key, init),
        shared: (key, init) => keyed(contextState, key, init),
        emit: (to, name, data) => to.evaluate(([n, d]) => window.__pyroTdEvent?.(n, d), [name, data]).catch(() => {}),
        element: async (id) => frame.locator(`[data-pyro-testdriver="${id}"]`).elementHandle({ timeout: 5000 }),
        windowPage: (arg) => windowPage(context, sessionOf, arg),
      };
      const result = await run(ctx, c);
      return JSON.stringify(result === undefined ? { ok: true } : { ok: true, result });
    } catch (e) {
      return JSON.stringify({ error: String(e?.message ?? e) });
    }
  });
}

// The page of a window argument (`{ window: id }`, td/index.mjs): the page whose top window carries the mark
// testdriver-vendor.js put on it. Read over the page's own CDP session: Playwright's evaluate in a popup waits for the
// popup's initialization, which Pyro's popups do not complete.
// A page that was only just opened by `window.open()` reaches `context.pages()` a moment later: wait for it.
async function windowPage(context, sessionOf, arg) {
  if (!arg.window) return null;
  for (let tries = 0; tries < 40; tries++) {
    for (const page of context.pages()) {
      const mark = await sessionOf(page).then((s) => s.send('Runtime.evaluate', { expression: 'window.__pyroTdWindow ?? null', returnByValue: true })).then((r) => r.result.value, () => null);
      if (mark === arg.window) return page;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  return null;
}
