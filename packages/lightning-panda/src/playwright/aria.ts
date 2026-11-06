// ARIA snapshots: the engine's ARIA tree (`read: "aria"`) rendered as
// Playwright's YAML, and Playwright's template language for
// `toMatchAriaSnapshot()` (partial matching, regexes, `/children` modes).

/** A node of the engine's ARIA tree. */
export type AriaNode = {
  role: string;
  name: string;
  checked?: boolean | 'mixed';
  disabled?: boolean;
  expanded?: boolean;
  level?: number;
  pressed?: boolean | 'mixed';
  selected?: boolean;
  props: Record<string, string>;
  children: (AriaNode | string)[];
};

type TextTemplate = { raw: string; normalized: string };
type Pattern = string | { pattern: string };
export type AriaTemplate =
  | { kind: 'text'; text: TextTemplate }
  | {
      kind: 'role';
      role: string;
      name?: Pattern;
      checked?: boolean | 'mixed';
      disabled?: boolean;
      expanded?: boolean;
      level?: number;
      pressed?: boolean | 'mixed';
      selected?: boolean;
      props?: Record<string, TextTemplate>;
      children?: AriaTemplate[];
      containerMode?: 'contain' | 'equal' | 'deep-equal';
    };
type RoleTemplate = Extract<AriaTemplate, { kind: 'role' }>;

// ---- rendering ----

function needsQuotes(s: string): boolean {
  return (
    s.length === 0 ||
    /^\s|\s$/.test(s) ||
    /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/.test(s) ||
    /^-/.test(s) ||
    /[\n:](\s|$)/.test(s) ||
    /\s#/.test(s) ||
    /[\n\r]/.test(s) ||
    /^[&*\],?!>|@"'#%]/.test(s) ||
    /[{}`]/.test(s) ||
    /^\[/.test(s) ||
    !isNaN(Number(s)) ||
    ['y', 'n', 'yes', 'no', 'true', 'false', 'on', 'off', 'null'].includes(s.toLowerCase())
  );
}
const escapeKey = (s: string) => (needsQuotes(s) ? `'${s.replace(/'/g, "''")}'` : s);
const ESC: Record<string, string> = { '\\': '\\\\', '"': '\\"', '\b': '\\b', '\f': '\\f', '\n': '\\n', '\r': '\\r', '\t': '\\t' };
const escapeValue = (s: string) =>
  needsQuotes(s) ? `"${s.replace(/[\\"\x00-\x1f\x7f-\x9f]/g, (c) => ESC[c] ?? `\\x${c.charCodeAt(0).toString(16).padStart(2, '0')}`)}"` : s;

const escapeRegExp = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Playwright's `convertToBestGuessRegex`: numbers, sizes, durations and UUIDs as regexes. */
function bestGuessRegex(text: string): string {
  const dynamic: [RegExp, string][] = [
    [/\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b/, '[0-9a-fA-F-]+'],
    [/\b[\d,.]+[bkmBKM]+\b/, '[\\d,.]+[bkmBKM]+'],
    [/\b\d+[hmsp]+\b/, '\\d+[hmsp]+'],
    [/\b[\d,.]+[hmsp]+\b/, '[\\d,.]+[hmsp]+'],
    [/\b\d+,\d+\b/, '\\d+,\\d+'],
    [/\b\d+\.\d{2,}\b/, '\\d+\\.\\d+'],
    [/\b\d{2,}\.\d+\b/, '\\d+\\.\\d+'],
    [/\b\d{2,}\b/, '\\d+'],
  ];
  let pattern = '';
  let last = 0;
  const combined = new RegExp(dynamic.map(([r]) => '(' + r.source + ')').join('|'), 'g');
  text.replace(combined, (match: string, ...args: any[]) => {
    const offset = args[args.length - 2] as number;
    const groups = args.slice(0, -2);
    pattern += escapeRegExp(text.slice(last, offset));
    const i = groups.findIndex((g) => !!g);
    if (i >= 0) pattern += dynamic[i][1];
    last = offset + match.length;
    return match;
  });
  if (!pattern) return text;
  pattern += escapeRegExp(text.slice(last));
  return String(new RegExp(pattern));
}

function longestCommonSubstring(a: string, b: string): string {
  let max = 0;
  let end = 0;
  let prev = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i++) {
    const row = new Array<number>(b.length + 1).fill(0);
    for (let j = 1; j <= b.length; j++) {
      if (a[i - 1] === b[j - 1]) {
        row[j] = prev[j - 1] + 1;
        if (row[j] > max) {
          max = row[j];
          end = i;
        }
      }
    }
    prev = row;
  }
  return a.slice(end - max, end);
}

/** Playwright's `textContributesInfo`: text that mostly repeats the name is left out. */
function textContributesInfo(n: AriaNode, text: string): boolean {
  if (!text.length) return false;
  if (!n.name) return true;
  if (n.name.length > text.length) return false;
  const sub = text.length <= 200 && n.name.length <= 200 ? longestCommonSubstring(text, n.name) : '';
  let filtered = text;
  while (sub && filtered.includes(sub)) filtered = filtered.replace(sub, '');
  return filtered.trim().length / text.length > 0.1;
}

function key(n: AriaNode, str: (s: string) => string): string {
  let k = n.role;
  if (n.name && n.name.length <= 900) {
    const name = str(n.name);
    if (name) k += ' ' + (name.startsWith('/') && name.endsWith('/') ? name : JSON.stringify(name));
  }
  if (n.checked === 'mixed') k += ' [checked=mixed]';
  if (n.checked === true) k += ' [checked]';
  if (n.disabled) k += ' [disabled]';
  if (n.expanded) k += ' [expanded]';
  if (n.level) k += ` [level=${n.level}]`;
  if (n.pressed === 'mixed') k += ' [pressed=mixed]';
  if (n.pressed === true) k += ' [pressed]';
  if (n.selected === true) k += ' [selected]';
  return k;
}

/**
 * Playwright's YAML for a tree (the root `fragment` is not shown). `regex`
 * is its `codegen` mode, which `toMatchAriaSnapshot` writes to a snapshot
 * file: numbers and the like as regexes, text that repeats a name left out.
 */
export function renderAria(root: AriaNode, depth?: number, regex = false): string {
  const lines: string[] = [];
  const pad = (d: number) => '  '.repeat(d);
  const str = regex ? bestGuessRegex : (s: string) => s;
  const includeText = regex ? textContributesInfo : () => true;
  const text = (t: string, d: number) => {
    if (depth && d > depth) return;
    const v = escapeValue(str(t));
    if (v) lines.push(`${pad(d)}- text: ${v}`);
  };
  const visit = (n: AriaNode, d: number) => {
    if (depth && d > depth) return;
    const k = `${pad(d)}- ${escapeKey(key(n, str))}`;
    const props = Object.entries(n.props);
    const only = n.children.length === 1 && typeof n.children[0] === 'string' && !props.length ? (n.children[0] as string) : undefined;
    const atLimit = !!depth && d === depth;
    if (only === undefined && (!n.children.length || atLimit) && !props.length) lines.push(k);
    else if (only !== undefined) lines.push(includeText(n, only) ? `${k}: ${escapeValue(str(only))}` : k);
    else {
      lines.push(`${k}:`);
      for (const [name, value] of props) lines.push(`${pad(d + 1)}- /${name}: ${escapeValue(value)}`);
      for (const c of n.children) typeof c === 'string' ? text(includeText(n, c) ? c : '', d + 1) : visit(c, d + 1);
    }
  };
  for (const c of root.role === 'fragment' ? root.children : [root]) typeof c === 'string' ? text(c, 0) : visit(c, 0);
  return lines.join('\n');
}

// ---- YAML (1.2, core schema): what Playwright's `yaml` package parses ----
//
// Block sequences and maps (compact forms, `-` alone with the item below,
// `? key` entries), flow collections, plain / quoted / block scalars over
// several lines, comments, anchors and tags, and the `---` / `...`
// document markers. Plain scalars resolve as in the core schema (null,
// booleans, numbers), so the template rules can reject what Playwright
// rejects.

type YScalar = { kind: 'scalar'; value: string | number | boolean | null; line: number };
type Y = YScalar | { kind: 'seq'; items: Y[]; line: number } | { kind: 'map'; entries: { key: Y; value: Y }[]; line: number } | { kind: 'alias'; line: number };

class YamlError extends Error {}

const ESCAPES: Record<string, string> = {
  '0': '\0', a: '\x07', b: '\b', t: '\t', '\t': '\t', n: '\n', v: '\v', f: '\f', r: '\r', e: '\x1b', ' ': ' ',
  '"': '"', '/': '/', '\\': '\\', N: '\x85', _: '\xa0', L: ' ', P: ' ',
};
const FLOW = ',[]{}';

/** The core schema's resolution of a plain scalar. */
function resolvePlain(s: string, tag?: string): string | number | boolean | null {
  if (tag === '!!str') return s;
  if (tag && !tag.startsWith('!!')) return s;
  if (/^(?:~|null|Null|NULL)?$/.test(s)) return tag === '!!str' ? s : null;
  if (/^(?:true|True|TRUE)$/.test(s)) return true;
  if (/^(?:false|False|FALSE)$/.test(s)) return false;
  if (/^[-+]?[0-9]+$/.test(s)) return Number(s);
  if (/^0o[0-7]+$/.test(s)) return parseInt(s.slice(2), 8);
  if (/^0x[0-9a-fA-F]+$/.test(s)) return parseInt(s.slice(2), 16);
  if (/^[-+]?\.(?:inf|Inf|INF)$/.test(s)) return s.startsWith('-') ? -Infinity : Infinity;
  if (/^\.(?:nan|NaN|NAN)$/.test(s)) return NaN;
  if (/^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?$/.test(s)) return Number(s);
  return s;
}

class YamlParser {
  private src: string;
  private pos = 0;
  private starts: number[] = [0];

  constructor(text: string) {
    this.src = text.replace(/^﻿/, '').replace(/\r\n?/g, '\n');
    for (let i = 0; i < this.src.length; i++) if (this.src[i] === '\n') this.starts.push(i + 1);
  }

  // ---- positions ----
  private lineIndex(p = this.pos): number {
    let lo = 0;
    let hi = this.starts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.starts[mid] <= p) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }
  private line(p = this.pos): number {
    return this.lineIndex(p) + 1;
  }
  private col(p = this.pos): number {
    return p - this.starts[this.lineIndex(p)];
  }
  private fail(message: string, p = this.pos): never {
    const i = this.lineIndex(p);
    const end = this.src.indexOf('\n', this.starts[i]);
    const text = this.src.slice(this.starts[i], end < 0 ? undefined : end);
    throw new YamlError(`${message} at line ${i + 1}, column ${this.col(p) + 1}:\n\n${text}\n${' '.repeat(this.col(p))}^\n`);
  }
  private ch(p = this.pos): string {
    return this.src[p] ?? '';
  }
  private eol(p = this.pos): boolean {
    return p >= this.src.length || this.src[p] === '\n';
  }
  /** Whitespace, a line end or the end: what must follow `-`, `?` and `:`. */
  private sep(p: number): boolean {
    return this.eol(p) || this.src[p] === ' ' || this.src[p] === '\t';
  }
  private spaces(): void {
    while (this.src[this.pos] === ' ' || this.src[this.pos] === '\t') this.pos++;
  }
  private comment(): void {
    if (this.src[this.pos] === '#') while (!this.eol()) this.pos++;
  }
  /** `---` or `...` at the start of a line. */
  private marker(p = this.pos): '---' | '...' | undefined {
    if (this.col(p) !== 0) return undefined;
    const m = this.src.slice(p, p + 3);
    return (m === '---' || m === '...') && this.sep(p + 3) ? m : undefined;
  }

  /**
   * Skips blank lines, spaces and comments to the next content: its column,
   * or -1 at the end or at a document marker.
   */
  private next(): number {
    for (;;) {
      if (this.col() === 0) {
        if (this.marker()) return -1;
        let p = this.pos;
        while (this.src[p] === ' ') p++;
        if (this.src[p] === '\t') {
          let q = p;
          while (this.src[q] === ' ' || this.src[q] === '\t') q++;
          if (!this.eol(q) && this.src[q] !== '#') this.fail('Tabs are not allowed as indentation', p);
        }
      }
      this.spaces();
      this.comment();
      if (this.pos >= this.src.length) return -1;
      if (this.src[this.pos] === '\n') {
        this.pos++;
        continue;
      }
      return this.col();
    }
  }

  private nullAt(p: number): YScalar {
    return { kind: 'scalar', value: null, line: this.line(p) };
  }

  // ---- document ----
  parse(): Y | null {
    // Directives before `---`.
    while (this.col() === 0 && this.ch() === '%') {
      while (!this.eol()) this.pos++;
      this.pos++;
    }
    let c = this.next();
    let root: Y | null = null;
    if (c === -1 && this.marker() === '---') {
      this.pos += 3;
      this.spaces();
      this.comment();
      // A block collection cannot start on the `---` line.
      if (!this.eol()) root = this.node(-1, 'inline');
      else c = this.next();
    }
    if (!root && c !== -1) root = this.node(-1, 'block');
    c = this.next();
    if (c !== -1) this.fail('Unexpected content, possibly from bad indentation');
    if (this.marker() === '...') {
      this.pos += 3;
      this.spaces();
      this.comment();
      if (!this.eol()) this.fail('Unexpected content after the document end marker');
      c = this.next();
    }
    if (this.pos < this.src.length && (c !== -1 || this.marker())) this.fail('Source contains multiple documents; please use YAML.parseAllDocuments()');
    return root;
  }

  // ---- block nodes ----
  /**
   * A node starting at the current content; `indent` is its parent's
   * column. `inline` is a value on the line of its map key, where a block
   * sequence or another mapping cannot start.
   */
  private node(indent: number, ctx: 'block' | 'inline'): Y {
    const start = this.pos;
    let tag: string | undefined;
    while (this.ch() === '&' || this.ch() === '!') {
      const s = this.pos;
      while (!this.sep(this.pos)) this.pos++;
      if (this.src[s] === '!') tag = this.src.slice(s, this.pos);
      this.spaces();
      this.comment();
      if (this.eol()) {
        const c = this.next();
        if (c === -1 || c <= indent) return tag === '!!str' ? { kind: 'scalar', value: '', line: this.line(start) } : this.nullAt(start);
        ctx = 'block';
      }
    }
    const ch = this.ch();
    const at = this.pos;
    if (ch === '*') {
      while (!this.sep(this.pos)) this.pos++;
      return { kind: 'alias', line: this.line(at) };
    }
    if (ch === '-' && this.sep(at + 1)) {
      if (ctx === 'inline') this.fail('Sequence items are not allowed on the same line with map keys');
      return this.blockSeq(this.col());
    }
    if (ch === '?' && this.sep(at + 1)) {
      if (ctx === 'inline') this.fail('Nested mappings are not allowed in compact mappings');
      return this.blockMap(this.col());
    }
    if (ch === '|' || ch === '>') return { kind: 'scalar', value: this.blockScalar(indent), line: this.line(at) };
    if (ch === '[' || ch === '{') {
      const v = this.flow();
      this.spaces();
      if (this.ch() === ':' && this.sep(this.pos + 1)) this.fail('Only string keys are supported');
      this.comment();
      if (!this.eol()) this.fail('Unexpected content after a flow collection');
      return v;
    }
    if (this.isKey()) {
      if (ctx === 'inline') this.fail('Nested mappings are not allowed in compact mappings');
      return this.blockMap(this.col());
    }
    const line = this.line(at);
    if (ch === '"' || ch === "'") {
      // A block quoted scalar's continuation lines must be indented past its parent.
      this.quoteMin = indent;
      const v = ch === '"' ? this.doubleQuoted() : this.singleQuoted();
      this.quoteMin = -1;
      this.spaces();
      this.comment();
      if (!this.eol()) this.fail('Unexpected scalar at node end');
      return { kind: 'scalar', value: tag && tag !== '!!str' ? resolvePlain(v, tag) : v, line };
    }
    return { kind: 'scalar', value: resolvePlain(this.plain(indent), tag), line };
  }

  /** Whether the current line holds an implicit `key:` at the cursor. */
  private isKey(): boolean {
    const save = this.pos;
    try {
      const ch = this.ch();
      if (ch === '"' || ch === "'") {
        if (!this.quotedOnLine()) return false;
        this.spaces();
        return this.ch() === ':' && this.sep(this.pos + 1);
      }
      if (ch === '[' || ch === '{') return false;
      for (let p = this.pos; !this.eol(p); p++) {
        if (this.src[p] === '#' && (this.src[p - 1] === ' ' || this.src[p - 1] === '\t')) return false;
        if (this.src[p] === ':' && this.sep(p + 1)) return true;
      }
      return false;
    } finally {
      this.pos = save;
    }
  }

  /** Skips a quoted scalar that ends on this line; false when it does not. */
  private quotedOnLine(): boolean {
    const q = this.ch();
    for (let p = this.pos + 1; !this.eol(p); p++) {
      if (q === '"' && this.src[p] === '\\') {
        p++;
        continue;
      }
      if (this.src[p] === q) {
        if (q === "'" && this.src[p + 1] === "'") {
          p++;
          continue;
        }
        this.pos = p + 1;
        return true;
      }
    }
    return false;
  }

  private blockSeq(c: number): Y {
    const items: Y[] = [];
    const line = this.line();
    for (;;) {
      this.pos++;
      this.spaces();
      this.comment();
      if (this.eol()) {
        const at = this.pos;
        const cc = this.next();
        items.push(cc > c ? this.node(c, 'block') : this.nullAt(at));
      } else items.push(this.node(c, 'block'));
      const cc = this.next();
      if (cc === -1 || cc < c) break;
      if (cc > c) this.fail('Bad indentation of a sequence entry');
      if (!(this.ch() === '-' && this.sep(this.pos + 1))) break;
    }
    return { kind: 'seq', items, line };
  }

  private blockMap(c: number): Y {
    const entries: { key: Y; value: Y }[] = [];
    const seen = new Set<unknown>();
    const line = this.line();
    for (;;) {
      const keyAt = this.pos;
      let key: Y;
      let value: Y;
      if (this.ch() === '?' && this.sep(this.pos + 1)) {
        this.pos++;
        this.spaces();
        this.comment();
        if (this.eol()) {
          const at = this.pos;
          key = this.next() > c ? this.node(c, 'block') : this.nullAt(at);
        } else key = this.node(c, 'block');
        const at = this.pos;
        const cc = this.next();
        if (cc === c && this.ch() === ':' && this.sep(this.pos + 1)) {
          this.pos++;
          value = this.value(c);
        } else value = this.nullAt(at);
      } else {
        key = this.implicitKey();
        this.spaces();
        if (this.ch() !== ':') this.fail('Implicit map keys need to be followed by map values');
        this.pos++;
        value = this.value(c);
      }
      if (key.kind === 'scalar') {
        const k = typeof key.value === 'number' && isNaN(key.value) ? 'NaN' : key.value;
        if (seen.has(k)) this.fail('Map keys must be unique', keyAt);
        seen.add(k);
      }
      entries.push({ key, value });
      const cc = this.next();
      if (cc === -1 || cc < c) break;
      if (cc > c) this.fail('Bad indentation of a mapping entry');
      if (this.ch() === '-' && this.sep(this.pos + 1)) this.fail('A block sequence may not be used as an implicit map key');
      if (!(this.ch() === '?' && this.sep(this.pos + 1)) && !this.isKey()) this.fail('Implicit map keys need to be followed by map values');
    }
    return { kind: 'map', entries, line };
  }

  /** A single-line key: a quoted or plain scalar before `:`. */
  private implicitKey(): YScalar {
    const at = this.pos;
    const line = this.line();
    const ch = this.ch();
    if (ch === '"' || ch === "'") return { kind: 'scalar', value: ch === '"' ? this.doubleQuoted() : this.singleQuoted(), line };
    this.checkPlainStart();
    let p = this.pos;
    while (!(this.src[p] === ':' && this.sep(p + 1))) p++;
    this.pos = p;
    const text = this.src.slice(at, p).trim();
    if (text.length > 1024) this.fail('The : indicator must be at most 1024 chars after the start of an implicit block mapping key', at);
    return { kind: 'scalar', value: resolvePlain(text), line };
  }

  /** The value after a block map's `key:`. */
  private value(c: number): Y {
    this.spaces();
    this.comment();
    if (this.eol()) {
      const at = this.pos;
      const cc = this.next();
      if (cc > c) return this.node(c, 'block');
      if (cc === c && this.ch() === '-' && this.sep(this.pos + 1)) return this.blockSeq(c);
      return this.nullAt(at);
    }
    return this.node(c, 'inline');
  }

  private checkPlainStart(): void {
    const ch = this.ch();
    if (!ch) return;
    if ('@`'.includes(ch)) this.fail(`Plain value cannot start with reserved character ${ch}`);
    if ('|>%'.includes(ch) || FLOW.includes(ch)) this.fail(`Plain value cannot start with ${ch}`);
  }

  /** A plain scalar, folded over continuation lines indented past `indent`. */
  private plain(indent: number): string {
    this.checkPlainStart();
    const readLine = (first: boolean): string => {
      const s = this.pos;
      while (!this.eol()) {
        const ch = this.ch();
        if (ch === '#' && (this.src[this.pos - 1] === ' ' || this.src[this.pos - 1] === '\t')) break;
        if (ch === ':' && this.sep(this.pos + 1)) this.fail(first ? 'Nested mappings are not allowed in compact mappings' : 'Implicit keys need to be on a single line');
        this.pos++;
      }
      return this.src.slice(s, this.pos).trim();
    };
    let out = readLine(true);
    for (;;) {
      if (this.ch() === '#') break; // a comment ends the scalar
      const save = this.pos;
      let blank = 0;
      let p = this.pos;
      let lineStart = -1;
      while (p < this.src.length) {
        p++; // past '\n'
        let q = p;
        while (this.src[q] === ' ' || this.src[q] === '\t') q++;
        if (this.eol(q)) {
          if (q >= this.src.length) break;
          blank++;
          p = q;
          continue;
        }
        lineStart = p;
        p = q;
        break;
      }
      if (lineStart < 0 || this.marker(lineStart) || this.src[p] === '#' || p - lineStart <= indent) {
        this.pos = save;
        break;
      }
      this.pos = p;
      const more = readLine(false);
      out += (blank ? '\n'.repeat(blank) : ' ') + more;
    }
    return out;
  }

  /** Indent a block quoted scalar's continuation lines must exceed (-1: no limit, as in flow context). */
  private quoteMin = -1;

  /** Folds a line break inside a quoted scalar; the cursor is on `\n`. */
  private foldBreak(quote: string): string {
    let blank = 0;
    this.pos++;
    for (;;) {
      if (this.marker()) this.fail('Unexpected document marker in a quoted string');
      this.spaces();
      if (this.ch() !== '\n') break;
      blank++;
      this.pos++;
    }
    // As the yaml package: an under-indented line ends the scalar unclosed.
    if (this.pos < this.src.length && this.col() <= this.quoteMin) this.fail(`Missing closing ${quote}quote`);
    return blank ? '\n'.repeat(blank) : ' ';
  }

  private doubleQuoted(): string {
    const at = this.pos;
    this.pos++;
    let out = '';
    let kept = 0; // the length escapes fixed; folding trims only past it
    for (;;) {
      if (this.pos >= this.src.length) this.fail('Missing closing "quote', at);
      const c = this.ch();
      if (c === '"') {
        this.pos++;
        return out;
      }
      if (c === '\n') {
        out = out.slice(0, kept) + out.slice(kept).replace(/[ \t]+$/, '');
        out += this.foldBreak('"');
        kept = out.length;
        continue;
      }
      if (c !== '\\') {
        out += c;
        this.pos++;
        continue;
      }
      const e = this.ch(this.pos + 1);
      if (e === '\n') {
        this.pos += 2;
        this.spaces();
        kept = out.length;
        continue;
      }
      if (e in ESCAPES) {
        out += ESCAPES[e];
        this.pos += 2;
      } else if (e === 'x' || e === 'u' || e === 'U') {
        const len = e === 'x' ? 2 : e === 'u' ? 4 : 8;
        const hex = this.src.slice(this.pos + 2, this.pos + 2 + len);
        if (!new RegExp(`^[0-9a-fA-F]{${len}}$`).test(hex)) this.fail(`Invalid escape sequence \\${e}${hex}`);
        out += String.fromCodePoint(parseInt(hex, 16));
        this.pos += 2 + len;
      } else this.fail(`Invalid escape sequence \\${e}`);
      kept = out.length;
    }
  }

  private singleQuoted(): string {
    const at = this.pos;
    this.pos++;
    let out = '';
    for (;;) {
      if (this.pos >= this.src.length) this.fail("Missing closing 'quote", at);
      const c = this.ch();
      if (c === "'") {
        if (this.ch(this.pos + 1) === "'") {
          out += "'";
          this.pos += 2;
          continue;
        }
        this.pos++;
        return out;
      }
      if (c === '\n') {
        out = out.replace(/[ \t]+$/, '');
        out += this.foldBreak("'");
        continue;
      }
      out += c;
      this.pos++;
    }
  }

  /** `|` / `>` with chomping (`-`, `+`) and indentation indicators. */
  private blockScalar(indent: number): string {
    const folded = this.ch() === '>';
    this.pos++;
    let chomp: 'clip' | 'strip' | 'keep' = 'clip';
    let explicit = 0;
    for (let i = 0; i < 2; i++) {
      const c = this.ch();
      if (c === '-' || c === '+') chomp = c === '-' ? 'strip' : 'keep';
      else if (/[1-9]/.test(c)) explicit = Number(c);
      else break;
      this.pos++;
    }
    const header = this.pos;
    this.spaces();
    if (this.ch() === '#' && this.pos === header) this.fail('Comments must be separated from other tokens by white space characters');
    this.comment();
    if (!this.eol()) this.fail('Unexpected content in a block scalar header');
    if (this.pos < this.src.length) this.pos++;
    const base = Math.max(indent, 0);
    let contentIndent = explicit ? base + explicit : -1;
    const lines: (string | null)[] = []; // null: an empty line
    while (this.pos < this.src.length) {
      const end = this.src.indexOf('\n', this.pos);
      const lineEnd = end < 0 ? this.src.length : end;
      const raw = this.src.slice(this.pos, lineEnd);
      const lead = raw.length - raw.replace(/^ +/, '').length;
      if (!raw.trim()) {
        lines.push(contentIndent >= 0 && raw.length > contentIndent ? raw.slice(contentIndent) : null);
      } else {
        if (contentIndent < 0) {
          if (lead <= indent) break;
          contentIndent = lead;
        }
        if (lead < contentIndent || this.marker(this.pos)) break;
        lines.push(raw.slice(contentIndent));
      }
      this.pos = end < 0 ? this.src.length : end + 1;
    }
    let out = '';
    let empty = 0;
    let content = false;
    let moreIndented = false;
    for (const l of lines) {
      if (l === null) {
        empty++;
        continue;
      }
      if (folded) {
        if (/^[ \t]/.test(l)) {
          moreIndented = true;
          out += '\n'.repeat(content ? 1 + empty : empty);
        } else if (moreIndented) {
          moreIndented = false;
          out += '\n'.repeat(empty + 1);
        } else if (empty === 0) {
          if (content) out += ' ';
        } else out += '\n'.repeat(empty);
      } else out += '\n'.repeat(content ? 1 + empty : empty);
      content = true;
      empty = 0;
      out += l;
    }
    if (chomp === 'keep') out += '\n'.repeat(content ? 1 + empty : empty);
    else if (chomp === 'clip' && content) out += '\n';
    return out;
  }

  // ---- flow collections ----
  private flowSpace(): void {
    for (;;) {
      if (this.marker()) this.fail('Unexpected document marker in a flow collection');
      while (' \t\n'.includes(this.ch()) && this.pos < this.src.length) this.pos++;
      if (this.ch() === '#') {
        this.comment();
        continue;
      }
      return;
    }
  }

  private flow(): Y {
    const open = this.ch();
    const close = open === '[' ? ']' : '}';
    const line = this.line();
    const at = this.pos;
    this.pos++;
    const items: Y[] = [];
    const entries: { key: Y; value: Y }[] = [];
    for (;;) {
      this.flowSpace();
      if (this.pos >= this.src.length) this.fail(`Missing closing ${close}`, at);
      if (this.ch() === close) {
        this.pos++;
        break;
      }
      const explicit = this.ch() === '?' && this.sep(this.pos + 1);
      if (explicit) {
        this.pos++;
        this.flowSpace();
      }
      const keyLine = this.line();
      const key = this.ch() === ',' || this.ch() === close || this.ch() === ':' ? this.nullAt(this.pos) : this.flowNode();
      this.flowSpace();
      let pair = explicit || open === '{';
      let value: Y = this.nullAt(this.pos);
      if (this.ch() === ':') {
        this.pos++;
        pair = true;
        this.flowSpace();
        if (this.ch() !== ',' && this.ch() !== close) value = this.flowNode();
        this.flowSpace();
      }
      if (open === '{') entries.push({ key, value });
      else items.push(pair ? { kind: 'map', entries: [{ key, value }], line: keyLine } : key);
      if (this.ch() === ',') this.pos++;
      else if (this.ch() !== close) this.fail(`Missing , or ${close} in a flow collection`);
    }
    return open === '[' ? { kind: 'seq', items, line } : { kind: 'map', entries, line };
  }

  private flowNode(): Y {
    let tag: string | undefined;
    while (this.ch() === '&' || this.ch() === '!') {
      const s = this.pos;
      while (!this.sep(this.pos) && !FLOW.includes(this.ch())) this.pos++;
      if (this.src[s] === '!') tag = this.src.slice(s, this.pos);
      this.flowSpace();
    }
    const ch = this.ch();
    const line = this.line();
    if (ch === '[' || ch === '{') return this.flow();
    if (ch === '*') {
      while (!this.sep(this.pos) && !FLOW.includes(this.ch())) this.pos++;
      return { kind: 'alias', line };
    }
    if (ch === '"' || ch === "'") {
      const v = ch === '"' ? this.doubleQuoted() : this.singleQuoted();
      return { kind: 'scalar', value: tag && tag !== '!!str' ? resolvePlain(v, tag) : v, line };
    }
    this.checkPlainStart();
    let out = '';
    let lineText = '';
    let blank = 0;
    while (this.pos < this.src.length) {
      const c = this.ch();
      if (FLOW.includes(c)) break;
      if (c === ':' && (this.sep(this.pos + 1) || FLOW.includes(this.ch(this.pos + 1)))) break;
      if (c === '#' && (this.src[this.pos - 1] === ' ' || this.src[this.pos - 1] === '\t' || this.src[this.pos - 1] === '\n')) break;
      if (c === '\n') {
        const t = lineText.trim();
        if (t) {
          out += (out ? (blank ? '\n'.repeat(blank) : ' ') : '') + t;
          blank = 0;
        } else if (out) blank++;
        lineText = '';
        this.pos++;
        continue;
      }
      lineText += c;
      this.pos++;
    }
    const t = lineText.trim();
    if (t) out += (out ? (blank ? '\n'.repeat(blank) : ' ') : '') + t;
    return { kind: 'scalar', value: resolvePlain(out, tag), line };
  }
}

function parseYaml(text: string): Y | null {
  return new YamlParser(text).parse();
}

// ---- templates ----

const squash = (t: string) => t.replace(/[​­]/g, '').replace(/[\r\n\s\t]+/g, ' ').trim();
const textValue = (raw: string): TextTemplate => ({ raw, normalized: squash(raw) });

/** `role "name" [attr=value]` (Playwright's KeyParser). */
function parseKey(input: string, line: number): RoleTemplate {
  let i = 0;
  const fail = (m: string): never => {
    throw new YamlError(`line ${line}: ${m}:\n\n${input}\n${' '.repeat(i)}^`);
  };
  const ws = () => {
    while (i < input.length && /\s/.test(input[i])) i++;
  };
  const ident = (what: string) => {
    if (i >= input.length) fail(`Unexpected end of input when expecting ${what}`);
    const s = i;
    while (i < input.length && /[a-zA-Z]/.test(input[i])) i++;
    return input.slice(s, i);
  };
  ws();
  const role = ident('role');
  ws();
  let name: Pattern | undefined;
  if (input[i] === '"') {
    i++;
    let out = '';
    for (;;) {
      if (i >= input.length) fail('Unterminated string');
      const c = input[i++];
      if (c === '\\') out += input[i++] ?? '';
      else if (c === '"') break;
      else out += c;
    }
    name = squash(out);
  } else if (input[i] === '/') {
    i++;
    let out = '';
    let inClass = false;
    for (;;) {
      if (i >= input.length) fail('Unterminated regex');
      const c = input[i++];
      if (c === '\\') out += c + (input[i++] ?? '');
      else if (c === '/' && !inClass) break;
      else {
        if (c === '[') inClass = true;
        else if (c === ']') inClass = false;
        out += c;
      }
    }
    name = { pattern: out };
  }
  const t: RoleTemplate = { kind: 'role', role, name: name ?? '' };
  for (;;) {
    ws();
    if (input[i] !== '[') break;
    i++;
    ws();
    const at = i;
    const flag = ident('attribute');
    ws();
    let value = '';
    if (input[i] === '=') {
      i++;
      ws();
      while (i < input.length && input[i] !== ']' && !/\s/.test(input[i])) value += input[i++];
    }
    ws();
    if (input[i] !== ']') fail('Expected ]');
    i++;
    value ||= 'true';
    const bool = (m: string, mixed = false) => {
      if (value !== 'true' && value !== 'false' && !(mixed && value === 'mixed')) {
        i = at;
        fail(m);
      }
      return value === 'mixed' ? 'mixed' : value === 'true';
    };
    if (flag === 'checked') t.checked = bool('Value of "checked" attribute must be a boolean or "mixed"', true);
    else if (flag === 'disabled') t.disabled = bool('Value of "disabled" attribute must be a boolean') as boolean;
    else if (flag === 'expanded') t.expanded = bool('Value of "expanded" attribute must be a boolean') as boolean;
    else if (flag === 'selected') t.selected = bool('Value of "selected" attribute must be a boolean') as boolean;
    else if (flag === 'pressed') t.pressed = bool('Value of "pressed" attribute must be a boolean or "mixed"', true);
    else if (flag === 'active') bool('Value of "active" attribute must be a boolean');
    else if (flag === 'level') {
      if (isNaN(Number(value))) {
        i = at;
        fail('Value of "level" attribute must be a number');
      }
      t.level = Number(value);
    } else {
      i = at;
      fail(`Unsupported attribute [${flag}]`);
    }
  }
  ws();
  if (i < input.length) fail('Unexpected input');
  return t;
}

/** Playwright's template for `toMatchAriaSnapshot(expected)`; throws on a malformed one. */
export function parseAriaTemplate(text: string): RoleTemplate {
  const doc = parseYaml(text);
  const fragment: RoleTemplate = { kind: 'role', role: 'fragment' };
  if (!doc || doc.kind !== 'seq') throw new YamlError('Aria snapshot must be a YAML sequence, elements starting with " -"');
  const err = (line: number, m: string): never => {
    throw new YamlError(`line ${line}: ${m}`);
  };
  const isString = (v: Y | undefined): v is YScalar & { value: string } => v?.kind === 'scalar' && typeof v.value === 'string';
  const seq = (into: RoleTemplate, s: Y & { kind: 'seq' }) => {
    for (const item of s.items) {
      if (isString(item)) (into.children ??= []).push(parseKey(item.value, item.line));
      else if (item.kind === 'map') map(into, item);
      else err(item.line, 'Sequence items should be strings or maps');
    }
  };
  const map = (into: RoleTemplate, m: Y & { kind: 'map' }) => {
    for (const { key: k, value } of m.entries) {
      into.children ??= [];
      if (!isString(k)) err(k.line, 'Only string keys are supported');
      const key = (k as YScalar & { value: string }).value;
      if (key === 'text') {
        if (!isString(value)) err(value.line, 'Text value should be a string');
        into.children.push({ kind: 'text', text: textValue((value as YScalar & { value: string }).value) });
      } else if (key === '/children') {
        const str = isString(value) ? value.value : undefined;
        if (str !== 'contain' && str !== 'equal' && str !== 'deep-equal') err(value.line, 'Strict value should be "contain", "equal" or "deep-equal"');
        into.containerMode = str as 'contain' | 'equal' | 'deep-equal';
      } else if (key.startsWith('/')) {
        if (!isString(value)) err(value.line, 'Property value should be a string');
        (into.props ??= {})[key.slice(1)] = textValue((value as YScalar & { value: string }).value);
      } else {
        const node = parseKey(key, k.line);
        if (value.kind === 'scalar') {
          // Strings, numbers and booleans (as `String()`); null is not a value.
          if (value.value === null) err(value.line, 'Node value should be a string or a sequence');
          into.children.push({ ...node, children: [{ kind: 'text', text: textValue(String(value.value)) }] });
        } else if (value.kind === 'seq') {
          into.children.push(node);
          seq(node, value);
        } else err(value.line, 'Map values should be strings or sequences');
      }
    }
  };
  seq(fragment, doc);
  if (fragment.children?.length === 1 && (!fragment.containerMode || fragment.containerMode === 'contain')) return fragment.children[0] as RoleTemplate;
  return fragment;
}

// ---- matching (Playwright's semantics) ----

function matchesPattern(text: string, p: Pattern | undefined): boolean {
  if (!p) return true;
  if (!text) return false;
  return typeof p === 'string' ? text === p : new RegExp(p.pattern).test(text);
}
function matchesText(text: string | undefined, t: TextTemplate | undefined): boolean {
  if (!t?.normalized) return true;
  if (!text) return false;
  if (text === t.normalized || text === t.raw) return true;
  if (t.raw.length > 1 && t.raw.startsWith('/') && t.raw.endsWith('/')) {
    try {
      return new RegExp(t.raw.slice(1, -1)).test(text);
    } catch {
      return false;
    }
  }
  return false;
}
function matchesNode(node: AriaNode | string, t: AriaTemplate, deep: boolean): boolean {
  if (typeof node === 'string') return t.kind === 'text' && matchesText(node, t.text);
  if (t.kind !== 'role') return false;
  if (t.role !== 'fragment' && t.role !== node.role) return false;
  for (const k of ['checked', 'disabled', 'expanded', 'level', 'pressed', 'selected'] as const) if (t[k] !== undefined && t[k] !== node[k]) return false;
  if (!matchesPattern(node.name, t.name)) return false;
  if (!matchesText(node.props.url, t.props?.url)) return false;
  const kids = node.children;
  const want = t.children ?? [];
  if (t.containerMode === 'contain') return containsList(kids, want);
  if (t.containerMode === 'equal') return listEqual(kids, want, false);
  if (t.containerMode === 'deep-equal' || deep) return listEqual(kids, want, true);
  return containsList(kids, want);
}
function listEqual(kids: (AriaNode | string)[], want: AriaTemplate[], deep: boolean): boolean {
  return kids.length === want.length && want.every((t, i) => matchesNode(kids[i], t, deep));
}
function containsList(kids: (AriaNode | string)[], want: AriaTemplate[]): boolean {
  if (want.length > kids.length) return false;
  let i = 0;
  for (const t of want) {
    while (i < kids.length && !matchesNode(kids[i], t, false)) i++;
    if (i++ >= kids.length) return false;
  }
  return true;
}
/** Whether the template matches somewhere in the tree. */
export function matchesAria(root: AriaNode, t: AriaTemplate): boolean {
  const visit = (n: AriaNode | string): boolean => matchesNode(n, t, false) || (typeof n !== 'string' && n.children.some(visit));
  return visit(root);
}

/**
 * Playwright's `unshift`: blank lines dropped and the common leading
 * indentation (at most 100) removed.
 */
export function dedent(text: string): string {
  const lines = text.split('\n');
  let min = 100;
  for (const l of lines) {
    if (!l.trim()) continue;
    const lead = /^(\s*)/.exec(l)![1].length;
    if (lead < min) min = lead;
  }
  return lines
    .filter((l) => l.trim())
    .map((l) => l.substring(min))
    .join('\n');
}
