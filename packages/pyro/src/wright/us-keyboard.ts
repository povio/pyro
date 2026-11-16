/**
 * Key names as Playwright's keyboard takes them: its US layout
 * (`usKeyboardLayout.ts`). A key is named by its `KeyboardEvent.code`
 * (`KeyD`, `Digit1`, `Minus`, `Numpad1`, `ShiftRight`), by what it types (`d`,
 * `D`, `!`), or as `Shift`, `Control`, `Alt`, `Meta`, `Enter` (also `\n`,
 * `\r`), `ArrowDown`, `F5`... Shift picks a code's shifted key (`Shift+KeyA`
 * is `A`); a key named by what it types stays that key (`Shift+a` is `a`).
 * The browser keeps the same layout for `press` and `type`
 * (crates/pyro-cdp/src/engine/keyboard.rs).
 */

type KeyDef = { keyCode: number; key: string; shiftKey?: string; text?: string; location?: number };

/** By code, in Playwright's order (a later key wins a name). Key codes as Chromium reports them. */
const LAYOUT: [code: string, def: KeyDef][] = [
  // Functions row
  ['Escape', { keyCode: 27, key: 'Escape' }],
  ...Array.from({ length: 12 }, (_, i): [string, KeyDef] => [`F${i + 1}`, { keyCode: 112 + i, key: `F${i + 1}` }]),
  // Numbers row
  ['Backquote', { keyCode: 192, shiftKey: '~', key: '`' }],
  ['Digit1', { keyCode: 49, shiftKey: '!', key: '1' }],
  ['Digit2', { keyCode: 50, shiftKey: '@', key: '2' }],
  ['Digit3', { keyCode: 51, shiftKey: '#', key: '3' }],
  ['Digit4', { keyCode: 52, shiftKey: '$', key: '4' }],
  ['Digit5', { keyCode: 53, shiftKey: '%', key: '5' }],
  ['Digit6', { keyCode: 54, shiftKey: '^', key: '6' }],
  ['Digit7', { keyCode: 55, shiftKey: '&', key: '7' }],
  ['Digit8', { keyCode: 56, shiftKey: '*', key: '8' }],
  ['Digit9', { keyCode: 57, shiftKey: '(', key: '9' }],
  ['Digit0', { keyCode: 48, shiftKey: ')', key: '0' }],
  ['Minus', { keyCode: 189, shiftKey: '_', key: '-' }],
  ['Equal', { keyCode: 187, shiftKey: '+', key: '=' }],
  ['Backslash', { keyCode: 220, shiftKey: '|', key: '\\' }],
  ['Backspace', { keyCode: 8, key: 'Backspace' }],
  // First row
  ['Tab', { keyCode: 9, key: 'Tab' }],
  ...letters('QWERTYUIOP'),
  ['BracketLeft', { keyCode: 219, shiftKey: '{', key: '[' }],
  ['BracketRight', { keyCode: 221, shiftKey: '}', key: ']' }],
  // Second row
  ['CapsLock', { keyCode: 20, key: 'CapsLock' }],
  ...letters('ASDFGHJKL'),
  ['Semicolon', { keyCode: 186, shiftKey: ':', key: ';' }],
  ['Quote', { keyCode: 222, shiftKey: '"', key: "'" }],
  ['Enter', { keyCode: 13, key: 'Enter', text: '\r' }],
  // Third row
  ['ShiftLeft', { keyCode: 16, key: 'Shift', location: 1 }],
  ...letters('ZXCVBNM'),
  ['Comma', { keyCode: 188, shiftKey: '<', key: ',' }],
  ['Period', { keyCode: 190, shiftKey: '>', key: '.' }],
  ['Slash', { keyCode: 191, shiftKey: '?', key: '/' }],
  ['ShiftRight', { keyCode: 16, key: 'Shift', location: 2 }],
  // Last row
  ['ControlLeft', { keyCode: 17, key: 'Control', location: 1 }],
  ['MetaLeft', { keyCode: 91, key: 'Meta', location: 1 }],
  ['AltLeft', { keyCode: 18, key: 'Alt', location: 1 }],
  ['Space', { keyCode: 32, key: ' ' }],
  ['AltRight', { keyCode: 18, key: 'Alt', location: 2 }],
  ['AltGraph', { keyCode: 225, key: 'AltGraph' }],
  ['MetaRight', { keyCode: 92, key: 'Meta', location: 2 }],
  ['ContextMenu', { keyCode: 93, key: 'ContextMenu' }],
  ['ControlRight', { keyCode: 17, key: 'Control', location: 2 }],
  // Center block
  ['PrintScreen', { keyCode: 44, key: 'PrintScreen' }],
  ['ScrollLock', { keyCode: 145, key: 'ScrollLock' }],
  ['Pause', { keyCode: 19, key: 'Pause' }],
  ['PageUp', { keyCode: 33, key: 'PageUp' }],
  ['PageDown', { keyCode: 34, key: 'PageDown' }],
  ['Insert', { keyCode: 45, key: 'Insert' }],
  ['Delete', { keyCode: 46, key: 'Delete' }],
  ['Home', { keyCode: 36, key: 'Home' }],
  ['End', { keyCode: 35, key: 'End' }],
  ['ArrowLeft', { keyCode: 37, key: 'ArrowLeft' }],
  ['ArrowUp', { keyCode: 38, key: 'ArrowUp' }],
  ['ArrowRight', { keyCode: 39, key: 'ArrowRight' }],
  ['ArrowDown', { keyCode: 40, key: 'ArrowDown' }],
  // Numpad
  ['NumLock', { keyCode: 144, key: 'NumLock' }],
  ['NumpadDivide', { keyCode: 111, key: '/', location: 3 }],
  ['NumpadMultiply', { keyCode: 106, key: '*', location: 3 }],
  ['NumpadSubtract', { keyCode: 109, key: '-', location: 3 }],
  ['Numpad7', { keyCode: 36, key: 'Home', shiftKey: '7', location: 3 }],
  ['Numpad8', { keyCode: 38, key: 'ArrowUp', shiftKey: '8', location: 3 }],
  ['Numpad9', { keyCode: 33, key: 'PageUp', shiftKey: '9', location: 3 }],
  ['Numpad4', { keyCode: 37, key: 'ArrowLeft', shiftKey: '4', location: 3 }],
  ['Numpad5', { keyCode: 12, key: 'Clear', shiftKey: '5', location: 3 }],
  ['Numpad6', { keyCode: 39, key: 'ArrowRight', shiftKey: '6', location: 3 }],
  ['NumpadAdd', { keyCode: 107, key: '+', location: 3 }],
  ['Numpad1', { keyCode: 35, key: 'End', shiftKey: '1', location: 3 }],
  ['Numpad2', { keyCode: 40, key: 'ArrowDown', shiftKey: '2', location: 3 }],
  ['Numpad3', { keyCode: 34, key: 'PageDown', shiftKey: '3', location: 3 }],
  ['Numpad0', { keyCode: 45, key: 'Insert', shiftKey: '0', location: 3 }],
  ['NumpadDecimal', { keyCode: 46, key: '\u0000', shiftKey: '.', location: 3 }],
  ['NumpadEnter', { keyCode: 13, key: 'Enter', text: '\r', location: 3 }],
];

function letters(row: string): [string, KeyDef][] {
  return [...row].map((c) => [`Key${c}`, { keyCode: c.charCodeAt(0), shiftKey: c, key: c.toLowerCase() }]);
}

/** One key event's fields (CDP `Input.dispatchKeyEvent`). */
export type KeyDescription = { key: string; code: string; keyCode: number; text: string; location: number; shifted?: KeyDescription };

const ALIASES: Record<string, string[]> = { ShiftLeft: ['Shift'], ControlLeft: ['Control'], AltLeft: ['Alt'], MetaLeft: ['Meta'], Enter: ['\n', '\r'] };

/** Playwright's `buildLayoutClosure`: codes (with their shifted keys), aliases, and what keys off the numpad type. */
const KEYS = (() => {
  const keys = new Map<string, KeyDescription>();
  for (const [code, def] of LAYOUT) {
    const base: KeyDescription = { key: def.key, code, keyCode: def.keyCode, text: def.text ?? (def.key.length === 1 && def.key !== '\u0000' ? def.key : ''), location: def.location ?? 0 };
    const shifted = def.shiftKey ? { ...base, key: def.shiftKey, text: def.shiftKey } : undefined;
    keys.set(code, { ...base, shifted });
    for (const alias of ALIASES[code] ?? []) keys.set(alias, base);
    if (def.location) continue;
    if (base.key.length === 1) keys.set(base.key, base);
    if (shifted) keys.set(shifted.key, shifted);
  }
  return keys;
})();

export const MODIFIERS = ['Alt', 'Control', 'Meta', 'Shift'] as const;
export type Modifier = (typeof MODIFIERS)[number];

/** `ControlOrMeta`: Meta on macOS, Control elsewhere. */
function smart(key: string): string {
  return key === 'ControlOrMeta' ? (process.platform === 'darwin' ? 'Meta' : 'Control') : key;
}

/**
 * The event fields of `key` with `modifiers` held (before it goes down);
 * throws Playwright's error for a key the layout does not have (`api` is
 * its prefix: `press`, `down`, `up`).
 */
export function describeKey(api: string, key: string, modifiers: ReadonlySet<string>): KeyDescription {
  const name = smart(key);
  const found = KEYS.get(name);
  if (!found) throw new Error(`${api}: Unknown key: "${name}"`);
  const d = modifiers.has('Shift') && found.shifted ? found.shifted : found;
  // With a modifier other than Shift held, a key types nothing.
  const typing = [...modifiers].every((m) => m === 'Shift');
  return { ...d, shifted: undefined, text: typing ? d.text : '' };
}

/** The keys of a chord, split as Playwright splits it (`Shift++` is Shift and `+`). */
export function splitChord(chord: string): string[] {
  const keys: string[] = [];
  let building = '';
  for (const c of chord) {
    if (c === '+' && building) {
      keys.push(building);
      building = '';
    } else building += c;
  }
  keys.push(building);
  return keys;
}

/** Throws Playwright's `Unknown key` error (prefixed `api`) for the first key of `chord` the layout does not have. */
export function checkChord(api: string, chord: string): void {
  for (const key of splitChord(chord)) describeKey(api, key, new Set());
}

/** CDP modifier bits: Alt 1, Control 2, Meta 4, Shift 8. */
export function modifierBits(modifiers: Iterable<string>): number {
  let bits = 0;
  for (const m of modifiers) bits |= m === 'Alt' ? 1 : m === 'Control' ? 2 : m === 'Meta' ? 4 : m === 'Shift' ? 8 : 0;
  return bits;
}
