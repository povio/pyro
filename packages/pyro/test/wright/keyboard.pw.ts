// Keys as Playwright's US layout names them: KeyboardEvent.code names
// (`KeyD`, `Digit1`, `Minus`...), shifted keys, modifiers held with
// keyboard.down(), and unknown keys failing at once. Passes on stock Playwright too.
import { expect, test } from './fixtures';

const LOG = `<input id=i><textarea id=t></textarea><script>
  window.log = [];
  for (const t of ['keydown', 'keyup']) addEventListener(t, (e) => log.push(t + ':' + e.key + '|' + e.code + '|' + e.keyCode + '|' + e.location + '|' + (e.shiftKey ? 'S' : '') + (e.altKey ? 'A' : '') + (e.ctrlKey ? 'C' : '') + (e.metaKey ? 'M' : '')), true);
</script>`;

async function pressed(page: any, keys: string) {
  await page.locator('#t').fill('');
  await page.locator('#t').focus();
  await page.evaluate(() => ((window as any).log = []));
  await page.keyboard.press(keys);
  return page.evaluate(() => ({ log: (window as any).log as string[], value: (document.getElementById('t') as HTMLTextAreaElement).value }));
}

test.describe('keyboard', () => {
  test('code names press the key they name', async ({ page }) => {
    await page.setContent(LOG);
    expect(await pressed(page, 'KeyD')).toEqual({ log: ['keydown:d|KeyD|68|0|', 'keyup:d|KeyD|68|0|'], value: 'd' });
    // The Dev panel shortcut Danex binds: key "d", code "KeyD", altKey, and no text.
    expect(await pressed(page, 'Alt+KeyD')).toEqual({
      log: ['keydown:Alt|AltLeft|18|1|A', 'keydown:d|KeyD|68|0|A', 'keyup:d|KeyD|68|0|A', 'keyup:Alt|AltLeft|18|1|'],
      value: '',
    });
    expect(await pressed(page, 'Shift+KeyA')).toEqual({
      log: ['keydown:Shift|ShiftLeft|16|1|S', 'keydown:A|KeyA|65|0|S', 'keyup:A|KeyA|65|0|S', 'keyup:Shift|ShiftLeft|16|1|'],
      value: 'A',
    });
    expect((await pressed(page, 'Digit1')).value).toBe('1');
    expect((await pressed(page, 'Shift+Digit1')).value).toBe('!');
    expect((await pressed(page, 'Shift+Minus')).value).toBe('_');
    for (const [code, text] of [['Equal', '='], ['BracketLeft', '['], ['BracketRight', ']'], ['Backslash', '\\'], ['Semicolon', ';'], ['Quote', "'"], ['Backquote', '`'], ['Comma', ','], ['Period', '.'], ['Slash', '/'], ['NumpadAdd', '+'], ['Space', ' ']]) {
      expect((await pressed(page, code)).value, code).toBe(text);
    }
    // A key named by what it types is not shifted; `+` is the key that types it.
    expect((await pressed(page, 'Shift+d')).value).toBe('d');
    expect((await pressed(page, 'Shift+D')).value).toBe('D');
    expect((await pressed(page, 'Shift++')).value).toBe('+');
    expect((await pressed(page, 'Numpad1')).log[0]).toBe('keydown:End|Numpad1|35|3|');
    expect((await pressed(page, 'ShiftRight')).log[0]).toBe('keydown:Shift|ShiftRight|16|2|S');
  });

  test('modifiers held with keyboard.down apply to presses, without events of their own', async ({ page }) => {
    await page.setContent(LOG);
    await page.locator('#t').focus();
    await page.keyboard.down('Shift');
    await page.evaluate(() => ((window as any).log = []));
    await page.keyboard.press('KeyB');
    await page.keyboard.up('Shift');
    const r = await page.evaluate(() => ({ log: (window as any).log as string[], value: (document.getElementById('t') as HTMLTextAreaElement).value }));
    expect(r).toEqual({ log: ['keydown:B|KeyB|66|0|S', 'keyup:B|KeyB|66|0|S', 'keyup:Shift|ShiftLeft|16|1|'], value: 'B' });
    await page.keyboard.down('KeyC');
    await page.keyboard.up('KeyC');
    expect(await page.locator('#t').inputValue()).toBe('Bc');
  });

  test('an unknown key fails at once', async ({ page }) => {
    await page.setContent(LOG);
    page.setDefaultTimeout(10_000);
    for (const [name, call] of [
      ['keyboard.press', () => page.keyboard.press('NoSuchKey')],
      ['keyboard.down', () => page.keyboard.down('NoSuchKey')],
      ['keyboard.up', () => page.keyboard.up('NoSuchKey')],
      ['locator.press', () => page.locator('#t').press('Shift+NoSuchKey')],
      ['a character off the layout', () => page.keyboard.press('é')],
    ] as const) {
      const started = Date.now();
      const error = await call().then(() => null, (e: Error) => e);
      expect(error?.message, name).toMatch(/Unknown key: "(NoSuchKey|é)"/);
      expect(Date.now() - started, name).toBeLessThan(2_000);
    }
  });

  test('type presses the keys of the layout and inserts other characters', async ({ page }) => {
    await page.setContent(LOG);
    await page.locator('#t').focus();
    await page.evaluate(() => ((window as any).log = []));
    await page.keyboard.type('aé\nB');
    const r = await page.evaluate(() => ({ log: (window as any).log as string[], value: (document.getElementById('t') as HTMLTextAreaElement).value }));
    expect(r.value).toBe('aé\nB');
    expect(r.log.filter((l) => l.startsWith('keydown'))).toEqual(['keydown:a|KeyA|65|0|', 'keydown:Enter|Enter|13|0|', 'keydown:B|KeyB|66|0|']);
    await page.locator('#i').pressSequentially('xÿ');
    expect(await page.locator('#i').inputValue()).toBe('xÿ');
  });
});
