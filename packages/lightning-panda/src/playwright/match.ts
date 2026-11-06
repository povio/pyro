// Playwright's text, URL and selector rules, client side: what its matchers
// send as `expectedText`, its URL globs, and the selector strings
// `page.locator()` takes, turned into lightningpanda locator specs.
import type { LocatorSpec } from '../steps.js';

/** One entry of Playwright's `expectedText` (`serializeExpectedTextValues`). */
export type ExpectedText = {
  string?: string;
  regexSource?: string;
  regexFlags?: string;
  matchSubstring?: boolean;
  ignoreCase?: boolean;
  normalizeWhiteSpace?: boolean;
};

export const normalizeWhiteSpace = (s: string) => s.replace(/​/g, '').trim().replace(/\s+/g, ' ');

export function textMatches(e: ExpectedText, received: unknown): boolean {
  if (typeof received !== 'string') return false;
  let have = e.normalizeWhiteSpace ? normalizeWhiteSpace(received) : received;
  if (e.string !== undefined) {
    let want = e.normalizeWhiteSpace ? normalizeWhiteSpace(e.string) : e.string;
    if (e.ignoreCase) {
      have = have.toLowerCase();
      want = want.toLowerCase();
    }
    return e.matchSubstring ? have.includes(want) : have === want;
  }
  if (e.regexSource !== undefined) {
    let flags = e.regexFlags ?? '';
    if (e.ignoreCase && !flags.includes('i')) flags += 'i';
    return new RegExp(e.regexSource, flags.replace(/[gy]/g, '')).test(have);
  }
  return false;
}

/** `toHaveText([...])`: one element per expected value, in order. */
export const textsMatch = (expected: ExpectedText[], received: string[]) =>
  received.length === expected.length && expected.every((e, i) => textMatches(e, received[i]));

/** `toContainText([...])`: the expected values match elements in order (a subsequence). */
export function textsContain(expected: ExpectedText[], received: string[]): boolean {
  let i = 0;
  for (const e of expected) {
    while (i < received.length && !textMatches(e, received[i])) i++;
    if (i === received.length) return false;
    i++;
  }
  return true;
}

// ---- URLs ----

const escapeRegex = (s: string) => s.replace(/[.+?^${}()|[\]\\/]/g, '\\$&');

/** Playwright's URL glob: `**` any characters, `*` any but `/`, `{a,b}` alternatives. */
export function globToRegex(glob: string): RegExp {
  let out = '^';
  let inGroup = false;
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '\\' && i + 1 < glob.length) {
      out += escapeRegex(glob[++i]);
    } else if (c === '*') {
      const star2 = glob[i + 1] === '*';
      if (star2) i++;
      out += star2 ? '.*' : '[^/]*';
    } else if (c === '{') {
      inGroup = true;
      out += '(';
    } else if (c === '}' && inGroup) {
      inGroup = false;
      out += ')';
    } else if (c === ',' && inGroup) {
      out += '|';
    } else {
      out += escapeRegex(c);
    }
  }
  return new RegExp(out + '$');
}

/** `url` resolved against `baseURL` as Playwright does (only a relative URL changes). */
export function resolveUrl(baseURL: string | undefined, url: string): string {
  if (!baseURL) return url;
  try {
    return new URL(url, baseURL).toString();
  } catch {
    return url;
  }
}

export type UrlMatch = string | RegExp | ((url: URL) => boolean | Promise<boolean>);

/** Playwright's `urlMatches`: a glob (relative to `baseURL`), a RegExp, or a predicate of the URL. */
export async function urlMatches(baseURL: string | undefined, url: string, match: UrlMatch): Promise<boolean> {
  if (typeof match === 'function') {
    try {
      return !!(await match(new URL(url)));
    } catch {
      return false;
    }
  }
  if (match instanceof RegExp) return match.test(url);
  const glob = match.startsWith('*') ? match : resolveUrl(baseURL, match);
  return globToRegex(glob).test(url);
}

// ---- selectors ----

type Pattern = LocatorSpec['text'];
export const pattern = (t: string | RegExp): Pattern => {
  if (typeof t === 'string') return t;
  const flags = t.flags.replace(/[gyu]/g, '');
  return flags ? { regex: t.source, flags } : { regex: t.source };
};

/** Unquotes `"text"` / `'text'` (exact) or returns the bare text (substring). */
function quoted(body: string): { text: string; exact: boolean } {
  const m = /^(["'])([\s\S]*)\1(s|i)?$/.exec(body.trim());
  if (m) return { text: m[2].replace(/\\(.)/g, '$1'), exact: m[3] !== 'i' };
  return { text: body.trim(), exact: false };
}

const unsupported = (what: string) =>
  new Error(`lightning-panda: ${what} is not supported yet; use a CSS selector or a getBy* locator`);

/** One `>>` part of a Playwright selector. */
function part(sel: string, testIdAttribute: string): LocatorSpec {
  const s = sel.trim();
  const engine = /^([a-zA-Z_-]+)=(.*)$/s.exec(s);
  if (engine && !/^[a-z-]+\[/.test(s)) {
    const [, name, body] = engine;
    switch (name) {
      case 'css':
        return cssPart(body);
      case 'text': {
        const { text, exact } = quoted(body);
        return exact ? { text, exact: true } : { text };
      }
      case 'id':
        return { css: `[id=${JSON.stringify(body)}]` };
      case 'data-testid':
      case 'data-test-id':
      case 'data-test':
        return { testId: quoted(body).text, testIdAttribute: name };
      case 'role':
        return rolePart(body);
      case 'xpath':
        throw unsupported('an xpath= selector');
      default:
        if (name === testIdAttribute) return { testId: quoted(body).text, testIdAttribute: name };
        if (name.startsWith('internal:')) throw unsupported(`the ${name} selector engine`);
    }
  }
  if (s.startsWith('//') || s.startsWith('..') || s.startsWith('(//')) throw unsupported('an XPath selector');
  if (/^["'].*["']$/s.test(s)) {
    const { text, exact } = quoted(s);
    return exact ? { text, exact: true } : { text };
  }
  return cssPart(s);
}

/** CSS, with Playwright's `:has-text("…")` and `:visible` at its end. */
function cssPart(sel: string): LocatorSpec {
  let css = sel.trim();
  const filter: NonNullable<LocatorSpec['filter']> = {};
  for (;;) {
    const visible = /:visible$/.exec(css);
    if (visible) {
      filter.visible = true;
      css = css.slice(0, visible.index);
      continue;
    }
    const hasText = /:has-text\((["'])((?:\\.|(?!\1).)*)\1\)$/.exec(css);
    if (hasText) {
      filter.hasText = hasText[2].replace(/\\(.)/g, '$1');
      css = css.slice(0, hasText.index);
      continue;
    }
    break;
  }
  if (/:(has-text|text|text-is|text-matches|visible|nth-match|light|right-of|left-of|above|below|near)\(/.test(css)) {
    throw unsupported(`the Playwright pseudo-class in ${JSON.stringify(sel)} (only a trailing :has-text() or :visible)`);
  }
  if (!css || /[\s>+~]$/.test(css)) css = `${css}*`;
  return Object.keys(filter).length ? { css, filter } : { css };
}

/** `role=button[name="Save"i]`. */
function rolePart(body: string): LocatorSpec {
  const m = /^([a-z]+)(.*)$/s.exec(body.trim());
  if (!m) throw unsupported(`role=${body}`);
  const spec: LocatorSpec = { role: m[1] };
  const attrs = m[2].trim();
  const name = /^\[name=(["'])((?:\\.|(?!\1).)*)\1(i|s)?\]$/.exec(attrs);
  if (name) {
    spec.name = name[2].replace(/\\(.)/g, '$1');
    if (name[3] !== 'i') spec.exact = true;
  } else if (attrs) {
    throw unsupported(`role selector attributes ${attrs}`);
  }
  return spec;
}

/** A Playwright selector string (`a >> b` chains) as a locator spec. */
export function parseSelector(selector: string, testIdAttribute = 'data-testid'): LocatorSpec {
  const parts = selector.split(/\s+>>\s+/);
  let spec: LocatorSpec | undefined;
  for (const p of parts) {
    if (/^nth=(-?\d+)$/.test(p.trim())) {
      if (!spec) throw unsupported('a selector starting with nth=');
      spec = { ...spec, nth: Number(p.trim().slice(4)) };
      continue;
    }
    const next = part(p, testIdAttribute);
    spec = spec ? { ...next, within: spec } : next;
  }
  if (!spec) throw new Error('empty selector');
  return spec;
}

// ---- descriptions (Playwright's `locator.toString()`) ----

export const quote = (t: string | RegExp) => (typeof t === 'string' ? `'${t.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'` : String(t));
export const optsString = (o: Record<string, unknown>) => {
  const entries = Object.entries(o).filter(([, v]) => v !== undefined);
  if (!entries.length) return '';
  return `, { ${entries.map(([k, v]) => `${k}: ${typeof v === 'string' || v instanceof RegExp ? quote(v) : String(v)}`).join(', ')} }`;
};
