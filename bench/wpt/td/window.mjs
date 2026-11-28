// WebDriver's window commands (https://w3c.github.io/webdriver/#windows): Minimize Window, Set / Get Window Rect, New
// Window and Navigate To, carried out with Browser.getWindowForTarget / Browser.setWindowBounds on the page's own CDP
// session (the browser's window state: a minimized window's documents are hidden, `visibilitychange` fires) and a
// new page of the browser context. `context` is the window of the call: null (the page that asked), a window of the
// page's script (`{ window: id }`, td/index.mjs), or the handle string New Window returned.

// A WebDriver window handle: 32 uppercase hex digits, as ChromeDriver's are.
const newHandle = () => [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('').toUpperCase();

// The page a `context` argument names.
async function pageOf(ctx, context) {
  if (context == null) return ctx.page;
  if (typeof context === 'string') {
    const page = ctx.shared('windows', () => new Map()).get(context);
    if (!page || page.isClosed()) throw new Error('no such window');
    return page;
  }
  const page = await ctx.windowPage(context);
  if (!page) throw new Error('no such window');
  return page;
}

const rectOf = (b) => ({ x: b.left, y: b.top, width: b.width, height: b.height });

async function windowOf(ctx, page) {
  const session = await ctx.sessionOf(page);
  const { windowId, bounds } = await session.send('Browser.getWindowForTarget');
  return { session, windowId, bounds };
}

export const commands = {
  // Fulfilled with the window rect from before, once the window is minimized (a minimized window keeps its rect).
  'minimize_window': async (ctx, [context]) => {
    const { session, windowId, bounds } = await windowOf(ctx, await pageOf(ctx, context));
    if (bounds.windowState !== 'minimized') await session.send('Browser.setWindowBounds', { windowId, bounds: { windowState: 'minimized' } });
    return rectOf(bounds);
  },
  // Set Window Rect: a minimized, maximized or full screen window is restored first, then the members the rect has
  // are applied (`x`, `y`, `width`, `height`: integers, or null for "leave as is"). Answers the rect that results.
  'set_window_rect': async (ctx, [rect, context]) => {
    const wanted = {};
    for (const [from, to] of [['x', 'left'], ['y', 'top'], ['width', 'width'], ['height', 'height']]) {
      const v = rect?.[from];
      if (v == null) continue;
      if (!Number.isInteger(v) || v < (from === 'x' || from === 'y' ? -(2 ** 31) : 0) || v > 2 ** 31 - 1) throw new Error(`invalid argument: ${from}`);
      wanted[to] = v;
    }
    const { session, windowId, bounds } = await windowOf(ctx, await pageOf(ctx, context));
    if (bounds.windowState && bounds.windowState !== 'normal') await session.send('Browser.setWindowBounds', { windowId, bounds: { windowState: 'normal' } });
    if (Object.keys(wanted).length) await session.send('Browser.setWindowBounds', { windowId, bounds: wanted });
    return rectOf((await windowOf(ctx, await pageOf(ctx, context))).bounds);
  },
  'get_window_rect': async (ctx, [context]) => rectOf((await windowOf(ctx, await pageOf(ctx, context))).bounds),
  // New Window / New Tab: a page of the same browser context, answered by a handle `navigate` and the other window
  // commands take as their `context`. `type` ("tab" or "window") is a hint the browser may ignore.
  'create_window': async (ctx) => {
    const page = await ctx.context.newPage();
    const handle = newHandle();
    ctx.shared('windows', () => new Map()).set(handle, page);
    return handle;
  },
  // Navigate To in the window of `context`: fulfilled when the page has loaded. A document that closes its own window
  // while loading ends the navigation as well.
  'navigate': async (ctx, [url, context]) => {
    const page = await pageOf(ctx, context);
    try {
      await page.goto(url, { waitUntil: 'load' });
    } catch (e) {
      if (!page.isClosed()) throw e;
    }
  },
};
