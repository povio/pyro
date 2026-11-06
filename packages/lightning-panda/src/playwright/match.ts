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

const unsupported = (what: string) =>
  new Error(`lightning-panda: ${what} is not supported; use a CSS selector or a getBy* locator`);

/**
 * A selector value: `"text"` / `'text'` with an optional `i` (substring, any
 * case) or `s` (exact) suffix, a `/regex/flags`, or bare text (substring).
 * Quoted values without a suffix are exact, as Playwright's `text="…"`.
 */
function value(body: string): { pattern: Pattern; exact: boolean } {
  const v = body.trim();
  const re = /^\/(.*)\/([a-z]*)$/s.exec(v);
  if (re) return { pattern: pattern(new RegExp(re[1], re[2].replace(/[gy]/g, ''))), exact: false };
  const q = /^(["'])((?:\\.|(?!\1)[\s\S])*)\1([is])?$/.exec(v);
  if (q) {
    const text = q[1] === '"' ? JSON.parse(`"${q[2]}"`) : q[2].replace(/\\(.)/g, '$1');
    return { pattern: text, exact: q[3] !== 'i' };
  }
  return { pattern: v, exact: false };
}
const textSpec = (body: string): LocatorSpec => {
  const { pattern: p, exact } = value(body);
  return typeof p === 'string' && exact ? { text: p, exact: true } : { text: p };
};

/** Splits at `sep` outside quotes, brackets and parentheses. */
function splitTop(s: string, sep: (s: string, i: number) => number): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote = '';
  let from = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = '';
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    else if (c === '(' || c === '[') depth++;
    else if (c === ')' || c === ']') depth--;
    else if (depth === 0) {
      const n = sep(s, i);
      if (n > 0) {
        out.push(s.slice(from, i));
        i += n - 1;
        from = i + 1;
      }
    }
  }
  out.push(s.slice(from));
  return out;
}

/** `[name="Save"i][checked=true][level=2][include-hidden]`: role attributes. */
function roleAttrs(spec: LocatorSpec, attrs: string): LocatorSpec {
  const re = /\[\s*([a-z-]+)\s*(?:=\s*("(?:\\.|[^"])*"[is]?|'(?:\\.|[^'])*'[is]?|[^\]]*?))?\s*\]/gy;
  let m: RegExpExecArray | null;
  let at = 0;
  const out: any = { ...spec };
  while ((m = re.exec(attrs))) {
    at = re.lastIndex;
    const [, key, raw] = m;
    const v = raw === undefined ? true : /^["']/.test(raw) ? value(raw).pattern : raw === 'true' ? true : raw === 'false' ? false : raw === 'mixed' ? 'mixed' : raw;
    switch (key) {
      case 'name': {
        const { pattern: p, exact } = value(raw ?? '""');
        out.name = p;
        if (exact && typeof p === 'string') out.exact = true;
        break;
      }
      case 'checked':
      case 'pressed':
        out[key] = v;
        break;
      case 'disabled':
      case 'expanded':
      case 'selected':
        out[key] = v === true || v === 'true';
        break;
      case 'level':
        out.level = Number(v);
        break;
      case 'include-hidden':
        out.includeHidden = v === true || v === 'true';
        break;
      default:
        throw unsupported(`the role attribute [${key}]`);
    }
  }
  if (attrs.slice(at).trim()) throw unsupported(`role selector attributes ${attrs}`);
  return out;
}

/** `[placeholder="x"i]` (Playwright's `internal:attr` and `internal:testid`). */
function attrSpec(body: string): LocatorSpec {
  const m = /^\[\s*([a-zA-Z_:-][\w:.-]*)\s*=\s*(.*)\]$/s.exec(body.trim());
  if (!m) throw unsupported(`attribute selector ${body}`);
  const [, name, raw] = m;
  const { pattern: p, exact } = value(raw);
  if (name === 'placeholder') return typeof p === 'string' && exact ? { placeholder: p, exact: true } : { placeholder: p };
  if (typeof p !== 'string') throw unsupported(`a RegExp for the ${name} attribute`);
  return { css: exact ? `[${name}=${JSON.stringify(p)}]` : `[${name}*=${JSON.stringify(p)} i]` };
}

/** The spec narrowed further after it picked an `nth`: it becomes the base set (`of`). */
const base = (spec: LocatorSpec): LocatorSpec => {
  if (spec.nth === undefined) return spec;
  const { frame, ...picked } = spec;
  return { of: picked, ...(frame ? { frame } : {}) };
};
const filtered = (spec: LocatorSpec, filter: NonNullable<LocatorSpec['filter']>): LocatorSpec => {
  const b = base(spec);
  return { ...b, filter: { ...b.filter, ...filter } };
};

const PW_PSEUDOS = ['has-text', 'text', 'text-is', 'text-matches', 'visible', 'nth-match', 'light', 'left-of', 'right-of', 'above', 'below', 'near'];
const LAYOUT = ['left-of', 'right-of', 'above', 'below', 'near'];

/** The index of the `)` closing the `(` at `open` (quotes and nesting respected). */
function closing(s: string, open: number): number {
  let depth = 0;
  let quote = '';
  for (let i = open; i < s.length; i++) {
    const c = s[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = '';
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '(') depth++;
    else if (c === ')' && --depth === 0) return i;
  }
  throw new Error(`unbalanced parentheses in ${JSON.stringify(s)}`);
}

/** A compound's Playwright pseudo-classes taken out: `button:has-text("x"):visible` → `button` and two pseudos. */
function pseudos(compound: string): { css: string; found: { name: string; arg?: string }[] } {
  const found: { name: string; arg?: string }[] = [];
  let css = '';
  let depth = 0;
  let quote = '';
  for (let i = 0; i < compound.length; i++) {
    const c = compound[i];
    if (quote) {
      css += c;
      if (c === '\\') css += compound[++i] ?? '';
      else if (c === quote) quote = '';
      continue;
    }
    if (c === '"' || c === "'") quote = c;
    if (c === '(' || c === '[') depth++;
    if (c === ')' || c === ']') depth--;
    if (c === ':' && depth === 0 && compound[i + 1] !== ':') {
      const m = /^:([a-z-]+)/.exec(compound.slice(i));
      if (m && PW_PSEUDOS.includes(m[1])) {
        let j = i + m[0].length;
        let arg: string | undefined;
        if (compound[j] === '(') {
          const close = closing(compound, j);
          arg = compound.slice(j + 1, close);
          j = close + 1;
        }
        found.push({ name: m[1], arg });
        i = j - 1;
        continue;
      }
    }
    css += c;
  }
  return { css, found };
}

/** CSS with Playwright's pseudo-classes (`:has-text`, `:text`, `:text-is`, `:text-matches`, `:visible`, `:nth-match`, `:light`). */
function cssPart(sel: string, testIdAttribute: string): LocatorSpec {
  const list = splitTop(sel, (s, i) => (s[i] === ',' ? 1 : 0)).map((x) => x.trim()).filter(Boolean);
  if (list.length > 1) {
    const [first, ...rest] = list.map((x) => cssPart(x, testIdAttribute));
    return { of: first, or: rest };
  }
  const one = list[0] ?? '*';
  if (!PW_PSEUDOS.some((p) => one.includes(`:${p}`))) return { css: one };
  // Compounds and the combinators between them.
  const pieces: { comb: string; compound: string }[] = [];
  let comb = '';
  let current = '';
  {
    let depth = 0;
    let quote = '';
    for (let i = 0; i < one.length; i++) {
      const c = one[i];
      if (quote) {
        current += c;
        if (c === '\\') current += one[++i] ?? '';
        else if (c === quote) quote = '';
        continue;
      }
      if (c === '"' || c === "'") quote = c;
      if (c === '(' || c === '[') depth++;
      if (c === ')' || c === ']') depth--;
      if (depth === 0 && (/\s/.test(c) || c === '>' || c === '+' || c === '~')) {
        let j = i;
        let found = ' ';
        while (j < one.length && (/\s/.test(one[j]) || '>+~'.includes(one[j]))) {
          if ('>+~'.includes(one[j])) found = one[j];
          j++;
        }
        if (current) pieces.push({ comb, compound: current });
        comb = found;
        current = '';
        i = j - 1;
        continue;
      }
      current += c;
    }
    if (current) pieces.push({ comb, compound: current });
  }
  let spec: LocatorSpec | undefined;
  let seg = '';
  let segComb = '';
  const finish = (extra: string): LocatorSpec => {
    if (spec && (segComb === '+' || segComb === '~')) throw unsupported(`a ${segComb} combinator after a Playwright pseudo-class`);
    const css = (seg + extra).trim() || '*';
    const scoped = spec && segComb === '>' ? `:scope > ${css}` : css;
    return { css: scoped, ...(spec ? { within: spec } : {}) };
  };
  for (const { comb: c, compound } of pieces) {
    const { css, found } = pseudos(compound);
    if (!found.length) {
      if (!seg) segComb = c;
      seg += (seg ? (c === ' ' ? ' ' : ` ${c} `) : '') + compound;
      continue;
    }
    if (!seg) segComb = c;
    const joined = seg ? (c === ' ' ? ' ' : ` ${c} `) : '';
    let next: LocatorSpec;
    const nthMatch = found.find((f) => f.name === 'nth-match');
    const light = found.find((f) => f.name === 'light');
    if (nthMatch) {
      if (css.trim() || found.length > 1 || seg) throw unsupported(':nth-match() combined with other selectors');
      const [inner, n] = splitTop(nthMatch.arg ?? '', (s, i) => (s[i] === ',' ? 1 : 0));
      next = { ...base(parseSelector(inner.trim(), testIdAttribute)), nth: Number(n) - 1 };
      if (spec) next = { ...next, within: spec };
    } else {
      next = finish(joined + (light ? (light.arg ?? '') : '') + (css || (light ? '' : '*')));
    }
    for (const f of found) {
      if (LAYOUT.includes(f.name)) throw unsupported(`the layout pseudo-class :${f.name}() (deprecated in Playwright)`);
      if (f.name === 'visible') next = filtered(next, { visible: true });
      else if (f.name === 'has-text') next = filtered(next, { hasText: value(f.arg ?? '').pattern });
      else if (f.name === 'text') next = { ...base(next), and: [...(next.and ?? []), { text: value(f.arg ?? '').pattern }] };
      else if (f.name === 'text-is') {
        const { pattern: p } = value(f.arg ?? '');
        next = { ...base(next), and: [...(next.and ?? []), typeof p === 'string' ? { text: p, exact: true } : { text: p }] };
      } else if (f.name === 'text-matches') {
        const [re, flags] = splitTop(f.arg ?? '', (s, i) => (s[i] === ',' ? 1 : 0)).map((x) => x && (value(x).pattern as string));
        next = { ...base(next), and: [...(next.and ?? []), { text: { regex: re, ...(flags ? { flags } : {}) } }] };
      }
    }
    spec = next;
    seg = '';
    segComb = '';
  }
  if (!spec) return { css: one };
  return seg ? finish('') : spec;
}

/** One `>>` part of a Playwright selector, applied to the spec so far. */
function part(prev: LocatorSpec | undefined, sel: string, testIdAttribute: string): LocatorSpec {
  const s = sel.trim();
  const nested = (body: string) => parseSelector(JSON.parse(body.trim()), testIdAttribute);
  const engine = /^([a-zA-Z_:-]+)=(.*)$/s.exec(s);
  const fresh = (next: LocatorSpec) => (prev ? { ...next, within: prev } : next);
  if (engine && !/^[a-z-]+\[/.test(s)) {
    const [, name, body] = engine;
    const need = () => {
      if (!prev) throw unsupported(`a selector starting with ${name}=`);
      return prev;
    };
    switch (name) {
      case 'css':
      case 'css:light':
        return fresh(cssPart(body, testIdAttribute));
      case 'text':
      case 'internal:text':
        return fresh(textSpec(body));
      case 'id':
        return fresh({ css: `[id=${JSON.stringify(body)}]` });
      case 'data-testid':
      case 'data-test-id':
      case 'data-test':
        return fresh({ testId: value(body).pattern as string, testIdAttribute: name });
      case 'role':
      case 'internal:role': {
        const m = /^([a-z]+)(.*)$/s.exec(body.trim());
        if (!m) throw unsupported(`${name}=${body}`);
        return fresh(roleAttrs({ role: m[1] }, m[2].trim()));
      }
      case 'internal:label': {
        const { pattern: p, exact } = value(body);
        return fresh(typeof p === 'string' && exact ? { label: p, exact: true } : { label: p });
      }
      case 'internal:attr':
        return fresh(attrSpec(body));
      case 'internal:testid': {
        const m = /^\[\s*([^=\s]+)\s*=\s*(.*)\]$/s.exec(body.trim());
        if (!m) throw unsupported(`${name}=${body}`);
        const id = value(m[2]).pattern;
        if (typeof id !== 'string') throw unsupported('a RegExp test id');
        return fresh(m[1] === 'data-testid' ? { testId: id } : { testId: id, testIdAttribute: m[1] });
      }
      case 'xpath':
        return fresh({ xpath: body });
      case 'nth':
        return { ...base(need()), nth: Number(body) };
      case 'visible':
        return filtered(need(), { visible: body.trim() !== 'false' });
      case 'internal:has':
        return filtered(need(), { has: nested(body) });
      case 'internal:has-not':
        return filtered(need(), { hasNot: nested(body) });
      case 'internal:has-text':
        return filtered(need(), { hasText: hasTextPattern(body) });
      case 'internal:has-not-text':
        return filtered(need(), { hasNotText: hasTextPattern(body) });
      case 'internal:and':
        return { of: base(need()), and: [nested(body)] };
      case 'internal:or':
        return { of: base(need()), or: [nested(body)] };
      case 'internal:chain':
        return { ...nested(body), within: need() };
      case 'internal:describe':
        return need();
      default:
        if (name === testIdAttribute) return fresh({ testId: value(body).pattern as string, testIdAttribute: name });
        if (name.startsWith('internal:')) throw unsupported(`the ${name} selector engine`);
    }
  }
  if (s.startsWith('//') || s.startsWith('..') || s.startsWith('(//')) return fresh({ xpath: s });
  if (/^["'].*["'][is]?$/s.test(s)) return fresh(textSpec(s));
  return fresh(cssPart(s, testIdAttribute));
}

/** `internal:has-text="x"s` is exact: as a whole-text RegExp (has-text matches substrings). */
function hasTextPattern(body: string): Pattern {
  const { pattern: p, exact } = value(body);
  if (typeof p !== 'string' || !exact || !/s$/.test(body.trim())) return p;
  const esc = p.trim().replace(/[.*+?^${}()|[\]\\]/g, '\\$&').replace(/\s+/g, '\\s+');
  return { regex: `^\\s*${esc}\\s*$` };
}

/**
 * A Playwright selector string as a locator spec: `a >> b` chains, engines
 * (`css=`, `text=`, `role=`, `id=`, `xpath=`, `data-testid=`, `nth=`,
 * `visible=`) and the `internal:` ones Playwright's locators serialize to
 * (`internal:role`, `internal:text`, `internal:label`, `internal:attr`,
 * `internal:testid`, `internal:has`, `internal:has-not`, `internal:has-text`,
 * `internal:has-not-text`, `internal:and`, `internal:or`, `internal:chain`,
 * `internal:control=enter-frame`), and CSS with Playwright's pseudo-classes.
 */
export function parseSelector(selector: string, testIdAttribute = 'data-testid'): LocatorSpec {
  const parts = splitTop(selector, (s, i) => {
    if (!/[\s>]/.test(s[i])) return 0;
    const m = /^\s*>>\s*/.exec(s.slice(i));
    return m ? m[0].length : 0;
  });
  let spec: LocatorSpec | undefined;
  // The `<iframe>` chain of `internal:control=enter-frame` parts.
  let frame: LocatorSpec | undefined;
  for (const p of parts) {
    if (p.trim() === 'internal:control=enter-frame') {
      if (!spec) throw unsupported('a selector starting with enter-frame');
      frame = frame ? { ...spec, frame } : spec;
      spec = undefined;
      continue;
    }
    spec = part(spec, p, testIdAttribute);
  }
  if (!spec) throw new Error('empty selector');
  return frame ? { ...spec, frame } : spec;
}

// ---- descriptions (Playwright's `locator.toString()`) ----

export const quote = (t: string | RegExp) => (typeof t === 'string' ? `'${t.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'` : String(t));
export const optsString = (o: Record<string, unknown>) => {
  const entries = Object.entries(o).filter(([, v]) => v !== undefined);
  if (!entries.length) return '';
  return `, { ${entries.map(([k, v]) => `${k}: ${typeof v === 'string' || v instanceof RegExp ? quote(v) : String(v)}`).join(', ')} }`;
};
