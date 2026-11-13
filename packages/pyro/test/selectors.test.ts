// parseSelector: Playwright selector strings as pyro locator specs.
import { expect, test } from 'bun:test';
import { parseSelector as p } from '../src/wright/match';

test('engines and chains', () => {
  expect(p('button')).toEqual({ css: 'button' });
  expect(p('text="Save"')).toEqual({ text: 'Save', exact: true });
  expect(p('text=Save')).toEqual({ text: 'Save' });
  expect(p('text=/sa+ve/i')).toEqual({ text: { regex: 'sa+ve', flags: 'i' } });
  expect(p('form >> button')).toEqual({ css: 'button', within: { css: 'form' } });
  expect(p('li >> nth=1')).toEqual({ css: 'li', nth: 1 });
  expect(p('li >> nth=0 >> nth=0')).toEqual({ of: { css: 'li', nth: 0 }, nth: 0 });
  expect(p('li >> visible=true')).toEqual({ css: 'li', filter: { visible: true } });
  expect(p('//li[b]')).toEqual({ xpath: '//li[b]' });
  expect(p('a, b')).toEqual({ of: { css: 'a' }, or: [{ css: 'b' }] });
});

test('internal engines', () => {
  expect(p('internal:role=button[name="Save"i]')).toEqual({ role: 'button', name: 'Save' });
  expect(p('internal:role=button[name="Save"s][pressed=true][level=2][include-hidden=true]')).toEqual({
    role: 'button', name: 'Save', exact: true, pressed: true, level: 2, includeHidden: true,
  });
  expect(p('internal:role=checkbox[checked="mixed"]')).toEqual({ role: 'checkbox', checked: 'mixed' });
  expect(p('internal:text="Hello"i')).toEqual({ text: 'Hello' });
  expect(p('internal:text="Hello"s')).toEqual({ text: 'Hello', exact: true });
  expect(p('internal:label="Name"i')).toEqual({ label: 'Name' });
  expect(p('internal:attr=[placeholder="Search"s]')).toEqual({ placeholder: 'Search', exact: true });
  expect(p('internal:attr=[alt="Logo"i]')).toEqual({ css: '[alt*="Logo" i]' });
  expect(p('internal:testid=[data-testid="total"s]')).toEqual({ testId: 'total' });
  expect(p('li >> internal:has-text="Banana"i')).toEqual({ css: 'li', filter: { hasText: 'Banana' } });
  expect(p('li >> internal:has="b"')).toEqual({ css: 'li', filter: { has: { css: 'b' } } });
  expect(p('li >> internal:has-not="b"')).toEqual({ css: 'li', filter: { hasNot: { css: 'b' } } });
  expect(p('h1 >> internal:or="h2"')).toEqual({ of: { css: 'h1' }, or: [{ css: 'h2' }] });
  expect(p('li >> internal:and="b"')).toEqual({ of: { css: 'li' }, and: [{ css: 'b' }] });
  expect(p('li >> internal:chain="b"')).toEqual({ css: 'b', within: { css: 'li' } });
  expect(p('#f >> internal:control=enter-frame >> button')).toEqual({ css: 'button', frame: { css: '#f' } });
  expect(p('#a >> internal:control=enter-frame >> #b >> internal:control=enter-frame >> p')).toEqual({ css: 'p', frame: { css: '#b', frame: { css: '#a' } } });
});

test('Playwright CSS pseudo-classes', () => {
  expect(p('button:has-text("Save")')).toEqual({ css: 'button', filter: { hasText: 'Save' } });
  expect(p('li:visible')).toEqual({ css: 'li', filter: { visible: true } });
  expect(p('div.card:has-text("x") button')).toEqual({ css: 'button', within: { css: 'div.card', filter: { hasText: 'x' } } });
  expect(p('div:has-text("x") > span')).toEqual({ css: ':scope > span', within: { css: 'div', filter: { hasText: 'x' } } });
  expect(p('button:text("Save")')).toEqual({ css: 'button', and: [{ text: 'Save' }] });
  expect(p('button:text-is("Save")')).toEqual({ css: 'button', and: [{ text: 'Save', exact: true }] });
  expect(p('p:text-matches("^a.c$", "i")')).toEqual({ css: 'p', and: [{ text: { regex: '^a.c$', flags: 'i' } }] });
  expect(p(':nth-match(li, 2)')).toEqual({ css: 'li', nth: 1 });
  expect(p('section ul li:has-text("A")')).toEqual({ css: 'section ul li', filter: { hasText: 'A' } });
  expect(() => p('a:right-of(b)')).toThrow('deprecated');
  expect(() => p('a:has-text("x") + b')).toThrow('combinator');
});
