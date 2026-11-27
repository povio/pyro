// The commands testdriver-vendor.js has always had: coordinates arrive in the calling frame's viewport (or, for an
// element origin, in the viewport of the frame the element is in, named by `at.frame`) and are mapped to the page's
// viewport here, through the frame elements around the frame (`ctx.off()`, `frameMap`). They carry their fields at the
// top level of the message (`{cmd:'click', at}`), so a handler receives that whole message as `args`.

// WebDriver's key codes (the Private Use Area values testdriver.js and test pages send) as Playwright key names. The
// right-hand modifiers and the numpad keys keep their location (Playwright's `ShiftRight`, `Numpad7`: location 2 and 3).
const KEYS = {
  '': 'Unidentified', '': 'Cancel', '': 'Help', '': 'Backspace', '': 'Tab', '': 'Clear',
  '': 'Enter', '': 'NumpadEnter', '': 'Shift', '': 'Control', '': 'Alt', '': 'Pause',
  '': 'Escape', '': ' ', '': 'PageUp', '': 'PageDown', '': 'End', '': 'Home',
  '': 'ArrowLeft', '': 'ArrowUp', '': 'ArrowRight', '': 'ArrowDown', '': 'Insert',
  '': 'Delete', '': ';', '': '=',
  '': 'F1', '': 'F2', '': 'F3', '': 'F4', '': 'F5',
  '': 'F6', '': 'F7', '': 'F8', '': 'F9', '': 'F10', '': 'F11', '': 'F12',
  '': 'Meta', '': 'ZenkakuHankaku', '': 'ShiftRight', '': 'ControlRight', '': 'AltRight',
  '': 'MetaRight',
  // The numpad's navigation keys (NumLock off, which is how Playwright's layout has them: key PageUp, code Numpad9).
  '': 'Numpad9', '': 'Numpad3', '': 'Numpad1', '': 'Numpad7', '': 'Numpad4',
  '': 'Numpad8', '': 'Numpad6', '': 'Numpad2', '': 'Numpad0',
};
// The keys Playwright's layout cannot say as WebDriver means them, sent as the key events they are: the numpad with
// NumLock on (key "0", code Numpad0, location 3, keypad) and the numpad's Delete (its layout gives "\0").
const RAW_KEYS = {
  '': ['0', 'Numpad0', 96], '': ['1', 'Numpad1', 97], '': ['2', 'Numpad2', 98], '': ['3', 'Numpad3', 99],
  '': ['4', 'Numpad4', 100], '': ['5', 'Numpad5', 101], '': ['6', 'Numpad6', 102], '': ['7', 'Numpad7', 103],
  '': ['8', 'Numpad8', 104], '': ['9', 'Numpad9', 105], '': ['*', 'NumpadMultiply', 106],
  '': ['+', 'NumpadAdd', 107], '': [',', 'NumpadComma', 108], '': ['-', 'NumpadSubtract', 109],
  '': ['.', 'NumpadDecimal', 110], '': ['/', 'NumpadDivide', 111],
  '': ['Delete', 'NumpadDecimal', 46, false],
};
const MODIFIER = /^(Shift|Control|Alt|Meta)(Left|Right)?$/;
const MODIFIER_BIT = { Alt: 1, Control: 2, Meta: 4, Shift: 8 };
// Playwright's name for a WebDriver key value (a character stays itself).
const keyName = (v) => KEYS[v] ?? v;
const modifierOf = (name) => MODIFIER.exec(name)?.[1];
const BUTTONS = ['left', 'middle', 'right'];
// WebDriver's pointer buttons (0 left, 1 middle, 2 right, 3 back, 4 forward) as CDP names and `buttons` bits.
const CDP_BUTTONS = ['left', 'middle', 'right', 'back', 'forward'];
const BUTTON_BITS = [1, 4, 2, 8, 16];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// The keyboard of a page for the key actions: Playwright's keyboard, plus the keys it cannot say (RAW_KEYS) over CDP.
// `held` (the page's, it outlives a call) is the set of modifiers down, which the pointer events sent over CDP carry.
function keyboard(page, session, held) {
  const mask = () => [...held].reduce((m, k) => m | MODIFIER_BIT[modifierOf(k)], 0);
  const raw = async (type, v) => {
    const [key, code, vk, text = true] = RAW_KEYS[v];
    await (await session()).send('Input.dispatchKeyEvent', {
      type: type === 'keyDown' && text ? 'keyDown' : type === 'keyDown' ? 'rawKeyDown' : 'keyUp', key, code, location: 3, isKeypad: true,
      windowsVirtualKeyCode: vk, modifiers: mask(), ...(type === 'keyDown' && text ? { text: key } : {}),
    });
  };
  return {
    mask,
    async down(v) {
      if (RAW_KEYS[v]) return raw('keyDown', v);
      const k = keyName(v);
      if (modifierOf(k)) held.add(k);
      await page.keyboard.down(k);
    },
    async up(v) {
      if (RAW_KEYS[v]) return raw('keyUp', v);
      const k = keyName(v);
      held.delete(k);
      await page.keyboard.up(k);
    },
    async press(v) { await this.down(v); await this.up(v); },
    async type(text) { await page.keyboard.type(text); },
  };
}

// The affine map (x' = a x + c y + e, y' = b x + d y + f) from a frame's viewport to the page's viewport.
const IDENTITY = { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 };
const apply = (m, x, y) => ({ x: m.a * x + m.c * y + m.e, y: m.b * x + m.d * y + m.f });
// `p` after `l`.
const compose = (p, l) => ({
  a: p.a * l.a + p.c * l.b, b: p.b * l.a + p.d * l.b, c: p.a * l.c + p.c * l.d, d: p.b * l.c + p.d * l.d,
  e: p.a * l.e + p.c * l.f + p.e, f: p.b * l.e + p.d * l.f + p.f,
});

// The map of a frame element's content box into its own document's viewport (runs in that document): the content box's
// origin (border plus padding), through the CSS transforms of the element and its ancestors. A transform changes the
// element's box into a parallelogram whose center is the center of its client rect; its linear part is the product of
// the transforms of the element and its ancestors (an ancestor's transform-origin only moves the box).
function measureFrameElement(e) {
  const s = getComputedStyle(e), r = e.getBoundingClientRect();
  const ix = e.clientLeft + parseFloat(s.paddingLeft), iy = e.clientTop + parseFloat(s.paddingTop);
  let m = null;
  try {
    for (let n = e; n && n.nodeType === 1; n = n.parentElement) {
      const t = getComputedStyle(n).transform;
      if (t && t !== 'none') m = new DOMMatrix(t).multiply(m ?? new DOMMatrix());
    }
  } catch { m = null; }
  if (!m || (m.a === 1 && m.b === 0 && m.c === 0 && m.d === 1)) return { a: 1, b: 0, c: 0, d: 1, e: r.left + ix, f: r.top + iy };
  const cx = r.left + r.width / 2, cy = r.top + r.height / 2, hx = e.offsetWidth / 2, hy = e.offsetHeight / 2;
  return { a: m.a, b: m.b, c: m.c, d: m.d, e: cx + m.a * (ix - hx) + m.c * (iy - hy), f: cy + m.b * (ix - hx) + m.d * (iy - hy) };
}

// A frame's viewport in the page's: its frame elements' maps composed, outermost last.
export async function frameMap(frame) {
  const parent = frame.parentFrame();
  if (!parent) return IDENTITY;
  const el = await frame.frameElement();
  const local = await el.evaluate(measureFrameElement);
  return compose(await frameMap(parent), local);
}
// What the call's `ctx.off()` is: the calling frame's map, with its origin as `x`, `y` (the offset of old).
export async function frameOffset(frame) {
  const m = await frameMap(frame);
  return { ...m, x: m.e, y: m.f };
}

// The frame the page marked with `data-pyro-td-frame` (testdriver-vendor.js: the frame element of an action's element
// origin that is in another frame than the caller's), found in whichever frame holds the marked element.
async function markedFrame(page, id) {
  if (id === 'main') return page.mainFrame();
  for (const f of page.frames()) {
    const handle = await f.$(`[data-pyro-td-frame="${id}"]`).catch(() => null);
    const inner = await handle?.contentFrame();
    if (inner) return inner;
  }
  throw new Error('testdriver: the frame of the element is gone');
}

// Element Send Keys: modifiers held until the end (or a second press), the rest typed key by key.
async function sendKeys(kbd, keys) {
  const held = [];
  for (const ch of keys) {
    if (modifierOf(keyName(ch))) {
      const i = held.indexOf(ch);
      if (i >= 0) { held.splice(i, 1); await kbd.up(ch); } else { held.push(ch); await kbd.down(ch); }
    } else if (KEYS[ch] || RAW_KEYS[ch]) await kbd.press(ch);
    else await kbd.type(ch);
  }
  for (const k of held.reverse()) await kbd.up(k);
}

// A pen's tilt (degrees, as CDP takes it) from altitude and azimuth angles (radians), as Pointer Events converts them.
function tiltOf(altitude, azimuth) {
  const t = Math.tan(altitude);
  const f = (v) => (Math.abs(t) < 1e-12 ? (Math.abs(v) < 1e-12 ? 0 : Math.sign(v) * 90) : (Math.atan(v / t) * 180) / Math.PI);
  return { tiltX: Math.round(f(Math.cos(azimuth))), tiltY: Math.round(f(Math.sin(azimuth))) };
}

// Perform Actions: tick by tick across the sources, each tick as long as its longest pause or move.
//
// A pointer source with `parameters.pointerType: 'touch'` plays as touch through Input.dispatchTouchEvent (`touch`: the
// page's CDP session): its own position, one stable touch id, and every active touch in each message, so several touch
// sources are one multi-touch gesture. Moves of down touches in one tick are interpolated together, in real time (the
// engine's gesture velocity reads the event times). A mouse source plays through Playwright's mouse (its back and
// forward buttons, which Playwright's mouse has not, over CDP). A pen source plays as pen over CDP
// (Input.dispatchMouseEvent with `pointerType: 'pen'`: the action's pressure, tangential pressure, tilt and twist, and
// the cumulative `buttons`). Pointer positions and pressed buttons outlive the call (`devices`), as WebDriver's input
// state does. Wheel sources send the wheel event at the point without moving the pointer.
async function actions(env, sources) {
  const { page, off, touch, touchState, devices, kbd, session, frameOf } = env;
  const ticks = Math.max(0, ...sources.map((s) => s.actions.length));
  const frameMaps = new Map();
  // A frame's viewport in the page's, per call (an element origin names its frame by the page's marker).
  const mapOf = async (id) => {
    if (id === undefined) return off;
    if (!frameMaps.has(id)) frameMaps.set(id, frameMap(await frameOf(id)));
    return frameMaps.get(id);
  };
  // Touch state outlives the call (a gesture may continue in the next one): per source id, one stable touch id.
  const touches = new Map(), pointers = new Map();
  for (const s of sources) {
    if (s.type !== 'pointer') continue;
    if (s.parameters?.pointerType === 'touch') {
      if (!touchState.has(s.id)) touchState.set(s.id, { id: touchState.size, x: 0, y: 0, down: false });
      touches.set(s, touchState.get(s.id));
    } else {
      // Click count, as Chrome's WebDriver counts it: a press of the same button at the same spot within 500 ms of the
      // last one counts one more (a double click's second press carries detail 2).
      if (!devices.has(s.id)) devices.set(s.id, { x: 0, y: 0, buttons: new Set(), button: 'none', last: null, clicks: 1 });
      pointers.set(s, devices.get(s.id));
    }
  }
  const points = () => [...touchState.values()].filter((t) => t.down).map((t) => ({ x: t.x, y: t.y, id: t.id }));
  // A pointer event over CDP (a pen, or a mouse button Playwright's mouse has not).
  const cdp = async (dev, pen, type, a, x, y, button, extra = {}) => {
    const buttons = [...dev.buttons].reduce((m, b) => m | (BUTTON_BITS[b] ?? 0), 0);
    const p = { type, x, y, button, buttons, modifiers: kbd.mask(), ...extra };
    if (pen) {
      let { tiltX, tiltY } = a;
      if (tiltX === undefined && tiltY === undefined && (a.altitudeAngle !== undefined || a.azimuthAngle !== undefined)) {
        ({ tiltX, tiltY } = tiltOf(a.altitudeAngle ?? Math.PI / 2, a.azimuthAngle ?? 0));
      }
      Object.assign(p, {
        pointerType: 'pen', force: a.pressure ?? (buttons ? 0.5 : 0), tangentialPressure: a.tangentialPressure ?? 0,
        tiltX: tiltX ?? 0, tiltY: tiltY ?? 0, twist: a.twist ?? 0,
      });
    } else p.force = buttons ? 0.5 : 0;
    await (await session()).send('Input.dispatchMouseEvent', p);
  };
  for (let i = 0; i < ticks; i++) {
    let wait = 0;
    const moves = [];
    for (const s of sources) {
      const a = s.actions[i];
      if (!a) continue;
      const tp = touches.get(s), dev = pointers.get(s);
      const pen = s.parameters?.pointerType === 'pen';
      const target = async () => {
        const x = a.x ?? 0, y = a.y ?? 0;
        if (a.origin === 'pointer') {
          const from = tp ?? dev ?? { x: 0, y: 0 };
          return { x: from.x + x, y: from.y + y };
        }
        if (a.origin === 'element') {
          const at = apply(await mapOf(a.at.frame), a.at.x, a.at.y);
          return { x: at.x + x, y: at.y + y };
        }
        const at = apply(off, x, y);
        return { x: at.x, y: at.y };
      };
      if (tp && (a.type === 'pointerMove' || a.type === 'pointerDown' || a.type === 'pointerUp' || a.type === 'pointerCancel')) {
        if (a.type === 'pointerMove') {
          const t = await target(), d = a.duration ?? 0;
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
          const t = await target();
          const d = a.duration ?? 0;
          const steps = Math.max(1, Math.round(d / 16));
          if (pen) {
            const from = { x: dev.x, y: dev.y };
            for (let k = 1; k <= steps; k++) await cdp(dev, true, 'mouseMoved', a, from.x + ((t.x - from.x) * k) / steps, from.y + ((t.y - from.y) * k) / steps, dev.button);
          } else if (dev.buttons.size && [...dev.buttons].some((b) => b > 2)) {
            // Playwright's mouse does not know the back and forward buttons it would have to report as held.
            const from = { x: dev.x, y: dev.y };
            for (let k = 1; k <= steps; k++) await cdp(dev, false, 'mouseMoved', a, from.x + ((t.x - from.x) * k) / steps, from.y + ((t.y - from.y) * k) / steps, dev.button);
          } else await page.mouse.move(t.x, t.y, { steps });
          dev.x = t.x; dev.y = t.y;
          wait = Math.max(wait, d);
          break;
        }
        case 'pointerDown': {
          const b = a.button ?? 0, now = Date.now();
          dev.clicks = dev.last && dev.last.b === b && dev.last.x === dev.x && dev.last.y === dev.y && now - dev.last.t < 500 ? dev.clicks + 1 : 1;
          dev.last = { b, x: dev.x, y: dev.y, t: now };
          dev.buttons.add(b);
          dev.button = CDP_BUTTONS[b] ?? 'left';
          if (pen || b > 2) await cdp(dev, pen, 'mousePressed', a, dev.x, dev.y, dev.button, { clickCount: dev.clicks });
          else await page.mouse.down({ button: BUTTONS[b], clickCount: dev.clicks });
          break;
        }
        case 'pointerUp': {
          const b = a.button ?? 0;
          dev.buttons.delete(b);
          const name = CDP_BUTTONS[b] ?? 'left';
          if (pen || b > 2) await cdp(dev, pen, 'mouseReleased', a, dev.x, dev.y, name, { clickCount: dev.clicks });
          else await page.mouse.up({ button: BUTTONS[b], clickCount: dev.clicks });
          dev.button = 'none';
          break;
        }
        case 'keyDown': await kbd.down(a.value); break;
        case 'keyUp': await kbd.up(a.value); break;
        case 'scroll': {
          // A wheel does not move the pointer: the event goes to the point, to the innermost frame under it.
          const t = await target();
          await (await session()).send('Input.dispatchMouseEvent', {
            type: 'mouseWheel', x: t.x, y: t.y, deltaX: a.deltaX ?? 0, deltaY: a.deltaY ?? 0, modifiers: kbd.mask(), pointerType: 'mouse',
          });
          wait = Math.max(wait, a.duration ?? 0);
          break;
        }
        default:
          if (a.type === 'pointerCancel') throw new Error('testdriver: pointerCancel is only supported for touch pointers (Input.dispatchMouseEvent cannot cancel a mouse or pen pointer)');
          throw new Error(`testdriver: action ${a.type} is not supported`);
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

const keyboardOf = (ctx) => keyboard(ctx.page, ctx.session, ctx.shared('kbd-held', () => new Set()));

export const commands = {
  async click(ctx, c) {
    const m = c.at.frame === undefined ? await ctx.off() : await frameMap(await markedFrame(ctx.page, c.at.frame));
    const at = apply(m, c.at.x, c.at.y);
    await ctx.page.mouse.click(at.x, at.y);
  },
  async send_keys(ctx, c) {
    await ctx.off();
    await sendKeys(keyboardOf(ctx), c.keys);
  },
  async actions(ctx, c) {
    const off = await ctx.off();
    const touch = async (type, touchPoints) => (await ctx.session()).send('Input.dispatchTouchEvent', { type, touchPoints });
    await actions({
      page: ctx.page, off, touch, session: ctx.session, kbd: keyboardOf(ctx), frameOf: (id) => markedFrame(ctx.page, id),
      touchState: ctx.state('touch', () => new Map()), devices: ctx.state('pointers', () => new Map()),
    }, c.sources);
  },
  async delete_all_cookies(ctx) {
    await ctx.off();
    await ctx.context.clearCookies();
  },
  async set_permission(ctx, c) {
    await ctx.off();
    // Browser.setPermission on the page's session applies to the page's browser context.
    await (await ctx.session()).send('Browser.setPermission', { permission: c.descriptor, setting: c.state, origin: c.origin });
  },
};
