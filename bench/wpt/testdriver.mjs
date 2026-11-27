// The runner's half of testdriver (testdriver-vendor.js is the page's half; README "testdriver"): a binding on every
// browser context that performs what the page asks with Playwright's input APIs over CDP, as wptrunner performs
// WebDriver commands for Chrome. Coordinates arrive in the calling frame's viewport; the frame's offset in the page is
// added here.

// WebDriver's key codes (the Private Use Area values testdriver.js and test pages send) as Playwright key names.
const KEYS = {
  '': 'Unidentified', '': 'Cancel', '': 'Help', '': 'Backspace', '': 'Tab', '': 'Clear',
  '': 'Enter', '': 'Enter', '': 'Shift', '': 'Control', '': 'Alt', '': 'Pause',
  '': 'Escape', '': ' ', '': 'PageUp', '': 'PageDown', '': 'End', '': 'Home',
  '': 'ArrowLeft', '': 'ArrowUp', '': 'ArrowRight', '': 'ArrowDown', '': 'Insert',
  '': 'Delete', '': ';', '': '=', '': '0', '': '1', '': '2', '': '3', '': '4',
  '': '5', '': '6', '': '7', '': '8', '': '9', '': '*', '': '+', '': ',',
  '': '-', '': '.', '': '/', '': 'F1', '': 'F2', '': 'F3', '': 'F4', '': 'F5',
  '': 'F6', '': 'F7', '': 'F8', '': 'F9', '': 'F10', '': 'F11', '': 'F12',
  '': 'Meta', '': 'ZenkakuHankaku', '': 'Shift', '': 'Control', '': 'Alt', '': 'Meta',
  '': 'PageUp', '': 'PageDown', '': 'End', '': 'Home', '': 'ArrowLeft', '': 'ArrowUp',
  '': 'ArrowRight', '': 'ArrowDown', '': 'Insert', '': 'Delete',
};
const MODIFIERS = new Set(['Shift', 'Control', 'Alt', 'Meta']);
const keyName = (v) => KEYS[v] ?? v;
const BUTTONS = ['left', 'middle', 'right'];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// The top-left of the frame's viewport in the page's (a frame element's content box).
async function frameOffset(frame) {
  if (!frame.parentFrame()) return { x: 0, y: 0 };
  const el = await frame.frameElement();
  const box = await el.boundingBox();
  const inset = await el.evaluate((e) => {
    const s = getComputedStyle(e);
    return { x: e.clientLeft + parseFloat(s.paddingLeft), y: e.clientTop + parseFloat(s.paddingTop) };
  });
  return { x: (box?.x ?? 0) + inset.x, y: (box?.y ?? 0) + inset.y };
}

// Element Send Keys: modifiers held until the end (or a second press), the rest typed key by key.
async function sendKeys(page, keys) {
  const held = [];
  for (const ch of keys) {
    const k = keyName(ch);
    if (MODIFIERS.has(k)) {
      const i = held.indexOf(k);
      if (i >= 0) { held.splice(i, 1); await page.keyboard.up(k); } else { held.push(k); await page.keyboard.down(k); }
    } else if (KEYS[ch]) await page.keyboard.press(k);
    else await page.keyboard.type(ch);
  }
  for (const k of held.reverse()) await page.keyboard.up(k);
}

// Perform Actions: tick by tick across the sources, each tick as long as its longest pause or move.
//
// A pointer source with `parameters.pointerType: 'touch'` plays as touch through Input.dispatchTouchEvent (`touch`: the
// page's CDP session): its own position, one stable touch id, and every active touch in each message, so several touch
// sources are one multi-touch gesture. Moves of down touches in one tick are interpolated together, in real time (the
// engine's gesture velocity reads the event times). Mouse and pen sources play as mouse.
async function actions(page, off, sources, touch, touchState) {
  const ticks = Math.max(0, ...sources.map((s) => s.actions.length));
  const pos = { x: 0, y: 0 };
  const pressed = new Set(), buttons = new Set();
  // Click count, as Chrome's WebDriver counts it: a press of the same button at the same spot within 500 ms of the
  // last one counts one more (a double click's second press carries detail 2).
  let last = null, clicks = 1;
  // Touch state outlives the call (a gesture may continue in the next one): per source id, one stable touch id.
  const touches = new Map();
  for (const s of sources) {
    if (s.type !== 'pointer' || s.parameters?.pointerType !== 'touch') continue;
    if (!touchState.has(s.id)) touchState.set(s.id, { id: touchState.size, x: 0, y: 0, down: false });
    touches.set(s, touchState.get(s.id));
  }
  const points = () => [...touchState.values()].filter((t) => t.down).map((t) => ({ x: t.x, y: t.y, id: t.id }));
  for (let i = 0; i < ticks; i++) {
    let wait = 0;
    const moves = [];
    for (const s of sources) {
      const a = s.actions[i];
      if (!a) continue;
      const tp = touches.get(s);
      const target = () => {
        const x = a.x ?? 0, y = a.y ?? 0;
        if (a.origin === 'pointer') return tp ? { x: tp.x + x, y: tp.y + y } : { x: pos.x + x, y: pos.y + y };
        if (a.origin === 'element') return { x: off.x + a.at.x + x, y: off.y + a.at.y + y };
        return { x: off.x + x, y: off.y + y };
      };
      if (tp && (a.type === 'pointerMove' || a.type === 'pointerDown' || a.type === 'pointerUp' || a.type === 'pointerCancel')) {
        if (a.type === 'pointerMove') {
          const t = target(), d = a.duration ?? 0;
          if (tp.down) moves.push({ tp, from: { x: tp.x, y: tp.y }, to: t, d });
          else { tp.x = t.x; tp.y = t.y; }
          wait = Math.max(wait, d);
        } else if (a.type === 'pointerDown') {
          if (!tp.down) { tp.down = true; await touch('touchStart', points()); }
        } else if (a.type === 'pointerUp') {
          if (tp.down) {
            // The released touch alone; none at all when it was the last one.
            const rest = points().filter((p) => p.id !== tp.id);
            await touch('touchEnd', rest.length ? [{ x: tp.x, y: tp.y, id: tp.id }] : []);
            tp.down = false;
          }
        } else if (points().length) {
          await touch('touchCancel', []);
          for (const t of touchState.values()) t.down = false;
        }
        continue;
      }
      switch (a.type) {
        case 'pause': wait = Math.max(wait, a.duration ?? 0); break;
        case 'pointerMove': {
          const t = target();
          const d = a.duration ?? 0;
          await page.mouse.move(t.x, t.y, { steps: Math.max(1, Math.round(d / 16)) });
          pos.x = t.x; pos.y = t.y;
          wait = Math.max(wait, d);
          break;
        }
        case 'pointerDown': {
          const b = a.button ?? 0, now = Date.now();
          clicks = last && last.b === b && last.x === pos.x && last.y === pos.y && now - last.t < 500 ? clicks + 1 : 1;
          last = { b, x: pos.x, y: pos.y, t: now };
          buttons.add(b);
          await page.mouse.down({ button: BUTTONS[b] ?? 'left', clickCount: clicks });
          break;
        }
        case 'pointerUp': buttons.delete(a.button ?? 0); await page.mouse.up({ button: BUTTONS[a.button ?? 0] ?? 'left', clickCount: clicks }); break;
        case 'keyDown': pressed.add(keyName(a.value)); await page.keyboard.down(keyName(a.value)); break;
        case 'keyUp': pressed.delete(keyName(a.value)); await page.keyboard.up(keyName(a.value)); break;
        case 'scroll': {
          const t = target();
          await page.mouse.move(t.x, t.y);
          pos.x = t.x; pos.y = t.y;
          await page.mouse.wheel(a.deltaX ?? 0, a.deltaY ?? 0);
          wait = Math.max(wait, a.duration ?? 0);
          break;
        }
        default: throw new Error(`testdriver: action ${a.type} is not supported`);
      }
    }
    if (moves.length) {
      const d = Math.max(...moves.map((m) => m.d));
      const steps = Math.max(1, Math.round(d / 16));
      for (let k = 1; k <= steps; k++) {
        for (const m of moves) {
          const f = m.d ? Math.min(1, (k * d) / steps / m.d) : 1;
          m.tp.x = m.from.x + (m.to.x - m.from.x) * f;
          m.tp.y = m.from.y + (m.to.y - m.from.y) * f;
        }
        await touch('touchMove', points());
        if (k < steps) await sleep(d / steps);
      }
      wait = Math.max(0, wait - (d - d / steps));
    }
    if (wait) await sleep(wait);
  }
}

export async function installTestdriver(context) {
  const sessions = new WeakMap(), touchStates = new WeakMap();
  await context.exposeBinding('__pyroTestdriver', async ({ page, frame }, json) => {
    try {
      const c = JSON.parse(json);
      const off = await frameOffset(frame);
      if (c.cmd === 'click') {
        await page.mouse.click(off.x + c.at.x, off.y + c.at.y);
      } else if (c.cmd === 'send_keys') {
        await sendKeys(page, c.keys);
      } else if (c.cmd === 'actions') {
        const session = async () => sessions.get(page) ?? sessions.set(page, await context.newCDPSession(page)).get(page);
        const touch = async (type, touchPoints) => (await session()).send('Input.dispatchTouchEvent', { type, touchPoints });
        if (!touchStates.has(page)) touchStates.set(page, new Map());
        await actions(page, off, c.sources, touch, touchStates.get(page));
      } else if (c.cmd === 'aria') {
        // The snapshot's first line is the element's own node: `- role "name" [...]` (or `- role: text`).
        const snap = await frame.locator(`[data-pyro-testdriver="${c.id}"]`).ariaSnapshot({ timeout: 5000 });
        const m = snap.split('\n')[0].match(/^- ([\w-]+)(?: ("(?:[^"\\]|\\.)*"))?/);
        // Nothing in the accessibility tree (hidden, or presentational with no content): no role, no name.
        if (!m) return JSON.stringify({ role: '', name: '' });
        return JSON.stringify({ role: m[1], name: m[2] ? JSON.parse(m[2]) : '' });
      } else if (c.cmd === 'delete_all_cookies') {
        await context.clearCookies();
      } else if (c.cmd === 'set_permission') {
        // Browser.setPermission on the page's session applies to the page's browser context.
        const s = (sessions.get(page) ?? sessions.set(page, await context.newCDPSession(page)).get(page));
        await s.send('Browser.setPermission', { permission: c.descriptor, setting: c.state, origin: c.origin });
      } else throw new Error(`testdriver: ${c.cmd} is not supported`);
      return JSON.stringify({ ok: true });
    } catch (e) {
      return JSON.stringify({ error: String(e?.message ?? e) });
    }
  });
}
