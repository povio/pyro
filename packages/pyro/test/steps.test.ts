// The native builders' wire shapes (no browser).
import { expect, test } from 'bun:test';
import { button, check, css, goto, heading, keyboard, role, text } from '../src/steps';

const step = (s: { toStep(): unknown }) => s.toStep();

test('a second hasText narrows the first', () => {
  expect(css('li').hasText('a').hasText('b').spec).toEqual({ of: { css: 'li', filter: { hasText: 'a' } }, filter: { hasText: 'b' } });
  expect(css('li').hasText('a').visible().spec).toEqual({ css: 'li', filter: { hasText: 'a', visible: true } });
});

test('filters after first() and nth()', () => {
  expect(css('li').first().hasText('a').spec).toEqual({ of: { css: 'li', nth: 0 }, filter: { hasText: 'a' } });
  expect(css('li').nth(1).nth(0).spec).toEqual({ of: { css: 'li', nth: 1 }, nth: 0 });
});

test('or / and', () => {
  expect(button('Save').or(text('Saved')).spec).toEqual({ of: { role: 'button', name: 'Save' }, or: [{ text: 'Saved' }] });
  expect(css('.a').and(role('tab', undefined, { selected: true })).spec).toEqual({ of: { css: '.a' }, and: [{ role: 'tab', selected: true }] });
});

test('role states and heading level', () => {
  expect(heading('Sub', { level: 2 }).spec).toEqual({ role: 'heading', name: 'Sub', level: 2 });
  expect(button(undefined, { pressed: true, exact: true }).spec).toEqual({ role: 'button', exact: true, pressed: true });
  expect(css('main').heading(undefined, { level: 1 }).spec).toEqual({ role: 'heading', level: 1, within: { css: 'main' } });
});

test('RegExp and attribute checks', () => {
  expect(step(check(css('p')).text(/^Order #\d+$/i))).toEqual({ check: { locator: { css: 'p' }, assert: 'text', expected: { regex: '^Order #\\d+$', flags: 'i' } } });
  expect(step(check(css('p')).containsText(/x/))).toEqual({ check: { locator: { css: 'p' }, assert: 'containsText', expected: { regex: 'x' } } });
  expect(step(check(css('i')).value(/^\d/))).toEqual({ check: { locator: { css: 'i' }, assert: 'value', expected: { regex: '^\\d' } } });
  expect(step(check(css('i')).attribute('data-state', 'open'))).toEqual({ check: { locator: { css: 'i' }, assert: 'attribute', expected: 'open', property: 'data-state' } });
  expect(step(check(css('i')).attribute('hidden'))).toEqual({ check: { locator: { css: 'i' }, assert: 'attribute', expected: null, not: true, property: 'hidden' } });
  expect(step(check(css('i')).not.attribute('hidden'))).toEqual({ check: { locator: { css: 'i' }, assert: 'attribute', expected: null, property: 'hidden' } });
});

test('keyboard and goto timeout', () => {
  expect(step(keyboard.press('Escape'))).toEqual({ keyboard: { press: 'Escape' } });
  expect(step(keyboard.type('ab', { settle: false }))).toEqual({ keyboard: { type: 'ab', settle: false } });
  expect(step(goto('/x', { timeout: 60_000 }))).toEqual({ goto: { url: '/x', waitUntil: 'load', timeout: 60_000 } });
  expect(step(goto('/x', 'domcontentloaded'))).toEqual({ goto: { url: '/x', waitUntil: 'domcontentloaded' } });
});
