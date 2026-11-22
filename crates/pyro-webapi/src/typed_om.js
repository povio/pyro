// CSS Typed OM 1 (https://drafts.css-houdini.org/css-typed-om-1/), made on a realm's first use
// (typed_om.rs): CSSStyleValue and its subclasses (CSSUnparsedValue, CSSVariableReferenceValue,
// CSSKeywordValue, CSSNumericValue with CSSUnitValue and the CSSMathValue family), the CSS.*
// numeric factories, and StylePropertyMap(ReadOnly) for `attributeStyleMap` / `computedStyleMap()`.
// Values are parsed and normalized by Stylo (`native.parse`); reification reads the normalized
// text. Not yet: CSSTransformValue, CSSImageValue, CSSColorValue (reified as plain CSSStyleValue).
(native => {
  'use strict';
  const { parse, kind, namedColor } = native;
  const apply = Reflect.apply;
  const DE = DOMException;
  const freeze = Object.freeze;
  const ILLEGAL = Symbol('illegal');
  const illegal = () => new TypeError('Illegal constructor');

  // Units and their base types (CSSNumericType), with the factor to the canonical unit.
  const UNITS = {};
  const def = (type, canonical, entries) => { for (const [u, f] of entries) UNITS[u] = { type, f: f ?? null, canonical }; };
  def('length', 'px', [['px', 1], ['cm', 96 / 2.54], ['mm', 96 / 25.4], ['q', 96 / 101.6], ['in', 96], ['pt', 4 / 3], ['pc', 16],
    ['em'], ['ex'], ['ch'], ['ic'], ['rem'], ['lh'], ['rlh'], ['vw'], ['vh'], ['vi'], ['vb'], ['vmin'], ['vmax'],
    ['svw'], ['svh'], ['svi'], ['svb'], ['svmin'], ['svmax'], ['lvw'], ['lvh'], ['lvi'], ['lvb'], ['lvmin'], ['lvmax'],
    ['dvw'], ['dvh'], ['dvi'], ['dvb'], ['dvmin'], ['dvmax'], ['cqw'], ['cqh'], ['cqi'], ['cqb'], ['cqmin'], ['cqmax'],
    ['cap'], ['rcap'], ['rch'], ['rex'], ['ric']]);
  def('angle', 'deg', [['deg', 1], ['rad', 180 / Math.PI], ['grad', 0.9], ['turn', 360]]);
  def('time', 's', [['s', 1], ['ms', 0.001]]);
  def('frequency', 'hz', [['hz', 1], ['khz', 1000]]);
  def('resolution', 'dppx', [['dppx', 1], ['dpi', 1 / 96], ['dpcm', 2.54 / 96], ['x', 1]]);
  def('flex', 'fr', [['fr', 1]]);
  const TYPES = ['length', 'angle', 'time', 'frequency', 'resolution', 'flex', 'percent'];
  // The factory names that are not the unit's (`CSS.Q()` makes `q`).
  const CASED = { q: 'Q', hz: 'Hz', khz: 'kHz' };
  const unitName = (u) => {
    const l = `${u}`.toLowerCase();
    if (l === 'number' || l === 'percent') return l;
    if (!(l in UNITS)) return null;
    return l;
  };
  const unitInfo = (u) => UNITS[u.toLowerCase()];
  const typeOf = (unit) => {
    const t = {};
    if (unit === 'percent') t.percent = 1;
    else if (unit !== 'number') t[unitInfo(unit).type] = 1;
    return t;
  };

  const fmt = (n) => {
    if (Object.is(n, -0)) return '0';
    if (!Number.isFinite(n)) return n === Infinity ? 'infinity' : n === -Infinity ? '-infinity' : 'NaN';
    let s = String(Number(n.toPrecision(6)));
    if (/e/.test(s)) s = n.toFixed(20).replace(/\.?0+$/, '');
    return s;
  };

  class CSSStyleValue {
    #text;
    constructor(token, text) {
      if (token !== ILLEGAL) throw illegal();
      this.#text = text;
    }
    toString() {
      if (!(this instanceof CSSStyleValue)) throw new TypeError('Illegal invocation');
      return this.#text ?? '';
    }
    static parse(property, cssText) {
      if (arguments.length < 2) throw new TypeError(`Failed to execute 'parse' on 'CSSStyleValue': 2 arguments required, but only ${arguments.length} present.`);
      return parseValues(`${property}`, `${cssText}`, false, 'parse')[0];
    }
    static parseAll(property, cssText) {
      if (arguments.length < 2) throw new TypeError(`Failed to execute 'parseAll' on 'CSSStyleValue': 2 arguments required, but only ${arguments.length} present.`);
      return parseValues(`${property}`, `${cssText}`, true, 'parseAll');
    }
  }
  const associatedProperties = new WeakMap();
  const plain = (property, text) => {
    const value = new CSSStyleValue(ILLEGAL, text);
    associatedProperties.set(value, property);
    return value;
  };

  // ---------- CSSUnparsedValue / CSSVariableReferenceValue ----------
  const isUnparsedMember = (v) => typeof v === 'string' || v instanceof CSSVariableReferenceValue;
  class CSSVariableReferenceValue {
    #variable;
    #fallback;
    constructor(variable, fallback = null) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSVariableReferenceValue': 1 argument required, but only 0 present.");
      variable = `${variable}`;
      if (!variable.startsWith('--')) throw new TypeError(`Failed to construct 'CSSVariableReferenceValue': Invalid custom property name '${variable}'.`);
      if (fallback !== null && fallback !== undefined && !(fallback instanceof CSSUnparsedValue)) {
        throw new TypeError("Failed to construct 'CSSVariableReferenceValue': parameter 2 is not of type 'CSSUnparsedValue'.");
      }
      this.#variable = variable;
      this.#fallback = fallback ?? null;
    }
    get variable() { return this.#variable; }
    set variable(v) {
      v = `${v}`;
      if (!v.startsWith('--')) throw new TypeError(`Failed to set the 'variable' property on 'CSSVariableReferenceValue': Invalid custom property name '${v}'.`);
      this.#variable = v;
    }
    get fallback() { return this.#fallback; }
    static _text(r) { return `var(${r.#variable}${r.#fallback ? ',' + r.#fallback.toString() : ''})`; }
  }
  const varText = CSSVariableReferenceValue._text;
  delete CSSVariableReferenceValue._text;

  class CSSUnparsedValue extends CSSStyleValue {
    #members;
    constructor(members) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSUnparsedValue': 1 argument required, but only 0 present.");
      super(ILLEGAL, null);
      if (members === null || typeof members !== 'object' || typeof members[Symbol.iterator] !== 'function') {
        throw new TypeError("Failed to construct 'CSSUnparsedValue': The provided value cannot be converted to a sequence.");
      }
      const list = [...members].map((m) => (m instanceof CSSVariableReferenceValue ? m : `${m}`));
      this.#members = list;
      const proxy = new Proxy(this, unparsedHandler);
      proxied.set(proxy, this);
      return proxy;
    }
    get length() { return CSSUnparsedValue.#get(this).length; }
    toString() { return CSSUnparsedValue.#get(this).map((m) => (typeof m === 'string' ? m : varText(m))).join(''); }
    entries() { return CSSUnparsedValue.#get(this).map((m, i) => [i, m])[Symbol.iterator](); }
    keys() { return CSSUnparsedValue.#get(this).map((_, i) => i)[Symbol.iterator](); }
    values() { return CSSUnparsedValue.#get(this).slice()[Symbol.iterator](); }
    forEach(cb, thisArg = undefined) { CSSUnparsedValue.#get(this).forEach((m, i) => apply(cb, thisArg, [m, i, this])); }
    static #get(o) {
      const t = proxied.get(o) || o;
      if (!(#members in t)) throw new TypeError('Illegal invocation');
      return t.#members;
    }
    static _members(o) { return CSSUnparsedValue.#get(o); }
  }
  const unparsedMembers = CSSUnparsedValue._members;
  delete CSSUnparsedValue._members;
  Object.defineProperty(CSSUnparsedValue.prototype, Symbol.iterator, { value: CSSUnparsedValue.prototype.values, writable: true, configurable: true });
  // Indexed properties (the value's members), on a proxy of the value.
  const proxied = new WeakMap();
  const index = (p) => (typeof p === 'string' && /^(0|[1-9]\d*)$/.test(p) ? Number(p) : -1);
  let transformItems;
  const unparsedHandler = {
    get(t, p, r) { const i = index(p); if (i >= 0) return unparsedMembers(t)[i]; return Reflect.get(t, p, t); },
    set(t, p, v, r) {
      const i = index(p);
      if (i < 0) return Reflect.set(t, p, v, t);
      const m = unparsedMembers(t);
      if (i > m.length) throw new RangeError(`Failed to set an indexed property on 'CSSUnparsedValue': Index ${i} exceeds index range for unparsed segments.`);
      m[i] = v instanceof CSSVariableReferenceValue ? v : `${v}`;
      return true;
    },
    has(t, p) { const i = index(p); if (i >= 0) return i < unparsedMembers(t).length; return Reflect.has(t, p); },
    getOwnPropertyDescriptor(t, p) {
      const i = index(p);
      if (i >= 0) { const m = unparsedMembers(t); return i < m.length ? { value: m[i], writable: true, enumerable: true, configurable: true } : undefined; }
      return Reflect.getOwnPropertyDescriptor(t, p);
    },
    ownKeys(t) { return [...unparsedMembers(t).map((_, i) => String(i)), ...Reflect.ownKeys(t)]; },
  };
  const makeUnparsed = (list) => { const v = new CSSUnparsedValue(list); return v; };
  // Text with var() references as CSSUnparsedValue members.
  const unparsedFrom = (text) => {
    const out = [];
    let i = 0, buf = '';
    while (i < text.length) {
      const m = /^var\(\s*/i.exec(text.slice(i));
      if (m) {
        // Find the matching close paren.
        let depth = 1, j = i + m[0].length;
        for (; j < text.length && depth; j++) { if (text[j] === '(') depth++; else if (text[j] === ')') depth--; }
        const inner = text.slice(i + m[0].length, j - 1);
        const comma = topComma(inner);
        const name = (comma < 0 ? inner : inner.slice(0, comma)).trim();
        if (buf) { out.push(buf); buf = ''; }
        const fallback = comma < 0 ? null : unparsedFrom(inner.slice(comma + 1));
        out.push(new CSSVariableReferenceValue(name, fallback));
        i = j;
        continue;
      }
      buf += text[i++];
    }
    if (buf || !out.length) out.push(buf);
    return makeUnparsed(out);
  };
  const topComma = (s) => { let d = 0; for (let i = 0; i < s.length; i++) { if (s[i] === '(') d++; else if (s[i] === ')') d--; else if (s[i] === ',' && !d) return i; } return -1; };

  // ---------- CSSKeywordValue ----------
  class CSSKeywordValue extends CSSStyleValue {
    #value;
    constructor(value) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSKeywordValue': 1 argument required, but only 0 present.");
      super(ILLEGAL, null);
      value = `${value}`;
      if (value === '') throw new TypeError("Failed to construct 'CSSKeywordValue': CSSKeywordValue does not support empty strings");
      this.#value = value;
    }
    get value() { return this.#value; }
    set value(v) {
      v = `${v}`;
      if (v === '') throw new TypeError("Failed to set the 'value' property on 'CSSKeywordValue': CSSKeywordValue does not support empty strings");
      this.#value = v;
    }
    toString() {
      if (!(#value in this)) throw new TypeError('Illegal invocation');
      let out = '';
      // Serialized as an identifier.
      for (const [i, ch] of [...this.#value].entries()) {
        const c = ch.codePointAt(0);
        if (c === 0) out += '�';
        else if ((c >= 1 && c <= 0x1f) || c === 0x7f || (i === 0 && c >= 0x30 && c <= 0x39)) out += '\\' + c.toString(16) + ' ';
        else if (c >= 0x80 || ch === '-' || ch === '_' || /[0-9A-Za-z]/.test(ch)) out += ch;
        else out += '\\' + ch;
      }
      return out;
    }
  }

  // ---------- CSSNumericValue ----------
  const typeEquals = (a, b) => TYPES.every((t) => (a[t] || 0) === (b[t] || 0));
  const mathTypes = new WeakMap();
  const applyPercentHint = (type, hint) => {
    if (type.percentHint === hint) return type;
    const result = { ...type, percentHint: hint };
    if (result.percent) {
      result[hint] = (result[hint] || 0) + result.percent;
      delete result.percent;
    }
    return result;
  };
  const combineTypes = (left, right, multiply) => {
    if (left.percentHint && right.percentHint && left.percentHint !== right.percentHint) return null;
    const hint = left.percentHint || right.percentHint;
    if (hint) {
      left = applyPercentHint(left, hint);
      right = applyPercentHint(right, hint);
    }
    if (multiply) {
      const result = hint ? { percentHint: hint } : {};
      for (const k of TYPES) {
        const power = (left[k] || 0) + (right[k] || 0);
        if (power) result[k] = power;
      }
      return result;
    }
    if (typeEquals(left, right)) return left;
    if (hint || (!left.percent && !right.percent)) return null;
    for (const k of TYPES) {
      if (k === 'percent' || (!left[k] && !right[k])) continue;
      const candidate = applyPercentHint(left, k);
      if (typeEquals(candidate, applyPercentHint(right, k))) return candidate;
    }
    return null;
  };
  const combineValueTypes = (values, multiply) => {
    let result = numericType(values[0]);
    for (let i = 1; result && i < values.length; i++) {
      const next = numericType(values[i]);
      result = next && combineTypes(result, next, multiply);
    }
    return result;
  };
  const rectify = (v, what) => {
    if (v instanceof CSSNumericValue) return v;
    if (typeof v === 'number') return new CSSUnitValue(v, 'number');
    if (typeof v === 'string') {
      const parsed = CSSNumericValue.parse(v);
      return parsed;
    }
    throw new TypeError(`Failed to execute '${what}' on 'CSSNumericValue': The value provided is not a CSSNumericValue or number.`);
  };

  class CSSNumericValue extends CSSStyleValue {
    constructor(token) {
      if (token !== ILLEGAL) throw illegal();
      super(ILLEGAL, null);
    }
    add(...values) { return sumOf([this, ...values.map((v) => rectify(v, 'add'))]); }
    sub(...values) { return sumOf([this, ...values.map((v) => negate(rectify(v, 'sub')))]); }
    mul(...values) { return productOf([this, ...values.map((v) => rectify(v, 'mul'))]); }
    div(...values) { return productOf([this, ...values.map((v) => invert(rectify(v, 'div')))]); }
    min(...values) { return extremum([this, ...values.map((v) => rectify(v, 'min'))], 'min'); }
    max(...values) { return extremum([this, ...values.map((v) => rectify(v, 'max'))], 'max'); }
    equals(...values) { return values.map((v) => rectify(v, 'equals')).every((v) => equal(this, v)); }
    to(unit) {
      const u = unitName(unit);
      if (u === null) throw new DE("Failed to execute 'to' on 'CSSNumericValue': Invalid unit for conversion", 'SyntaxError');
      if (isUnit(this)) {
        const result = convert(this, u);
        if (result) return result;
        throw new TypeError("Failed to execute 'to' on 'CSSNumericValue': Cannot convert to the given unit");
      }
      const sum = sumValues(this);
      if (!sum || sum.length !== 1) throw new TypeError("Failed to execute 'to' on 'CSSNumericValue': Cannot convert to the given unit");
      const r = convert(sum[0], u);
      if (!r) throw new TypeError("Failed to execute 'to' on 'CSSNumericValue': Cannot convert to the given unit");
      return r;
    }
    toSum(...units) {
      const names = units.map((u) => { const n = unitName(u); if (n === null) throw new DE("Failed to execute 'toSum' on 'CSSNumericValue': Invalid unit", 'SyntaxError'); return n; });
      const sum = sumValues(this);
      if (!sum) throw new TypeError("Failed to execute 'toSum' on 'CSSNumericValue': Cannot convert to a sum");
      if (!names.length) {
        const sorted = sum.slice().sort((a, b) => (a.unit < b.unit ? -1 : a.unit > b.unit ? 1 : 0));
        return new CSSMathSum(...sorted);
      }
      const out = names.map((n) => new CSSUnitValue(0, n));
      for (const v of sum) {
        let done = false;
        for (let i = 0; i < names.length; i++) {
          const c = convert(v, names[i]);
          if (c) { out[i] = new CSSUnitValue(out[i].value + c.value, names[i]); done = true; break; }
        }
        if (!done) throw new TypeError("Failed to execute 'toSum' on 'CSSNumericValue': Cannot convert to the given units");
      }
      return new CSSMathSum(...out);
    }
    type() {
      const t = numericType(this);
      if (!t) throw new TypeError('Invalid type');
      const out = {};
      for (const k of TYPES) if (t[k]) out[k] = t[k];
      if (t.percentHint) out.percentHint = t.percentHint;
      return out;
    }
    static parse(cssText) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'parse' on 'CSSNumericValue': 1 argument required, but only 0 present.");
      const v = parseNumeric(`${cssText}`.trim(), true);
      if (!v) throw new DE("Failed to execute 'parse' on 'CSSNumericValue': Invalid math expression", 'SyntaxError');
      return v;
    }
  }

  class CSSUnitValue extends CSSNumericValue {
    #value;
    #unit;
    constructor(value, unit) {
      if (arguments.length < 2) throw new TypeError(`Failed to construct 'CSSUnitValue': 2 arguments required, but only ${arguments.length} present.`);
      super(ILLEGAL);
      value = Number(value);
      if (!Number.isFinite(value)) throw new TypeError("Failed to construct 'CSSUnitValue': The provided double value is non-finite.");
      const u = unitName(unit);
      if (u === null) throw new TypeError(`Failed to construct 'CSSUnitValue': Invalid unit: ${unit}`);
      this.#value = value;
      this.#unit = u;
    }
    get value() { return this.#value; }
    set value(v) {
      v = Number(v);
      if (!Number.isFinite(v)) throw new TypeError("Failed to set the 'value' property on 'CSSUnitValue': The provided double value is non-finite.");
      this.#value = v;
    }
    get unit() { return this.#unit; }
    toString() {
      if (!(#unit in this)) throw new TypeError('Illegal invocation');
      return unitText(this.#value, this.#unit);
    }
  }
  const unitText = (v, u) => fmt(v) + (u === 'number' ? '' : u === 'percent' ? '%' : u);
  const isUnit = (v) => v instanceof CSSUnitValue;

  class CSSNumericArray {
    #values;
    constructor(token, values) {
      if (token !== ILLEGAL) throw illegal();
      this.#values = freeze(values.slice());
      for (let i = 0; i < values.length; i++) Object.defineProperty(this, i, { value: values[i], enumerable: true });
    }
    get length() { return this.#values.length; }
    values() { return this.#values[Symbol.iterator](); }
    keys() { return this.#values.keys(); }
    entries() { return this.#values.entries(); }
    forEach(cb, thisArg = undefined) { this.#values.forEach((v, i) => apply(cb, thisArg, [v, i, this])); }
  }
  Object.defineProperty(CSSNumericArray.prototype, Symbol.iterator, { value: CSSNumericArray.prototype.values, writable: true, configurable: true });

  class CSSMathValue extends CSSNumericValue {
    constructor(token) {
      if (token !== ILLEGAL) throw illegal();
      super(ILLEGAL);
    }
    toString() { return mathText(this, false); }
  }
  const variadic = (name, minArgs) => class extends CSSMathValue {
    #values;
    constructor(...args) {
      super(ILLEGAL);
      if (args.length < minArgs) throw new DE(`Failed to construct '${name}': Must specify at least one argument.`, 'SyntaxError');
      const values = args.map((v) => rectify(v, name));
      this.#values = new CSSNumericArray(ILLEGAL, values);
      const type = combineValueTypes(values, name === 'CSSMathProduct');
      if (!type) throw new TypeError(`Failed to construct '${name}': Incompatible types`);
      mathTypes.set(this, type);
    }
    get values() { return this.#values; }
    get operator() { return OPERATORS[name]; }
  };
  const OPERATORS = { CSSMathSum: 'sum', CSSMathProduct: 'product', CSSMathMin: 'min', CSSMathMax: 'max', CSSMathNegate: 'negate', CSSMathInvert: 'invert', CSSMathClamp: 'clamp' };
  const CSSMathSum = variadic('CSSMathSum', 1);
  const CSSMathProduct = variadic('CSSMathProduct', 1);
  const CSSMathMin = variadic('CSSMathMin', 1);
  const CSSMathMax = variadic('CSSMathMax', 1);
  for (const [cls, name] of [[CSSMathSum, 'CSSMathSum'], [CSSMathProduct, 'CSSMathProduct'], [CSSMathMin, 'CSSMathMin'], [CSSMathMax, 'CSSMathMax']]) {
    Object.defineProperty(cls, 'name', { value: name });
  }
  class CSSMathNegate extends CSSMathValue {
    #value;
    constructor(arg) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSMathNegate': 1 argument required, but only 0 present.");
      super(ILLEGAL);
      this.#value = rectify(arg, 'CSSMathNegate');
    }
    get value() { return this.#value; }
    get operator() { return 'negate'; }
  }
  class CSSMathInvert extends CSSMathValue {
    #value;
    constructor(arg) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSMathInvert': 1 argument required, but only 0 present.");
      super(ILLEGAL);
      this.#value = rectify(arg, 'CSSMathInvert');
    }
    get value() { return this.#value; }
    get operator() { return 'invert'; }
  }
  class CSSMathClamp extends CSSMathValue {
    #lower; #value; #upper;
    constructor(lower, value, upper) {
      if (arguments.length < 3) throw new TypeError(`Failed to construct 'CSSMathClamp': 3 arguments required, but only ${arguments.length} present.`);
      super(ILLEGAL);
      this.#lower = rectify(lower, 'CSSMathClamp');
      this.#value = rectify(value, 'CSSMathClamp');
      this.#upper = rectify(upper, 'CSSMathClamp');
      const type = combineValueTypes([this.#lower, this.#value, this.#upper], false);
      if (!type) throw new TypeError("Failed to construct 'CSSMathClamp': Incompatible types");
      mathTypes.set(this, type);
    }
    get lower() { return this.#lower; }
    get value() { return this.#value; }
    get upper() { return this.#upper; }
    get operator() { return 'clamp'; }
  }

  const negate = (v) => (isUnit(v) ? new CSSUnitValue(-v.value, v.unit) : v instanceof CSSMathNegate ? v.value : new CSSMathNegate(v));
  const invert = (v) => {
    if (isUnit(v) && v.unit === 'number') {
      if (v.value === 0) throw new RangeError('Cannot divide by zero');
      return new CSSUnitValue(1 / v.value, 'number');
    }
    return v instanceof CSSMathInvert ? v.value : new CSSMathInvert(v);
  };
  const sumOf = (values) => {
    const flat = values.flatMap((v) => (v instanceof CSSMathSum ? [...v.values] : [v]));
    if (flat.every(isUnit) && flat.every((v) => v.unit === flat[0].unit)) {
      return new CSSUnitValue(flat.reduce((a, v) => a + v.value, 0), flat[0].unit);
    }
    return new CSSMathSum(...flat);
  };
  const productOf = (values) => {
    const flat = values.flatMap((v) => (v instanceof CSSMathProduct ? [...v.values] : [v]));
    if (flat.every(isUnit)) {
      const nonNumbers = flat.filter((v) => v.unit !== 'number');
      if (nonNumbers.length <= 1) {
        return new CSSUnitValue(flat.reduce((a, v) => a * v.value, 1), nonNumbers.length ? nonNumbers[0].unit : 'number');
      }
    }
    return new CSSMathProduct(...flat);
  };
  const extremum = (values, which) => {
    const Cls = which === 'min' ? CSSMathMin : CSSMathMax;
    const flat = values.flatMap((v) => (v instanceof Cls ? [...v.values] : [v]));
    if (flat.every(isUnit) && flat.every((v) => v.unit === flat[0].unit)) {
      return new CSSUnitValue(Math[which](...flat.map((v) => v.value)), flat[0].unit);
    }
    return new Cls(...flat);
  };

  const numericType = (v) => {
    if (isUnit(v)) return typeOf(v.unit);
    if (v instanceof CSSMathNegate) return numericType(v.value);
    if (v instanceof CSSMathInvert) {
      const t = numericType(v.value);
      if (!t) return null;
      const o = t.percentHint ? { percentHint: t.percentHint } : {};
      for (const k of TYPES) if (t[k]) o[k] = -t[k];
      return o;
    }
    return mathTypes.get(v) || null;
  };

  // Sum-value terms keep powers of canonical units until the entire expression is
  // reduced: a reciprocal or squared unit need not itself fit in CSSUnitValue.
  const unitKey = units => Object.keys(units).sort().map(unit => unit + ':' + units[unit]).join(',');
  function accumulate(terms, value, units) {
    const key = unitKey(units), existing = terms.get(key);
    if (existing) existing.value += value;
    else terms.set(key, { value, units });
  }
  function sumTerms(v) {
    if (isUnit(v)) {
      const info = unitInfo(v.unit);
      const absolute = info && info.f !== null;
      const unit = absolute ? info.canonical : v.unit;
      return [{ value: absolute ? v.value * info.f : v.value, units: unit === 'number' ? {} : { [unit]: 1 } }];
    }
    if (v instanceof CSSMathInvert || v instanceof CSSMathNegate) {
      const terms = sumTerms(v.value);
      if (!terms) return null;
      if (v instanceof CSSMathNegate) return terms.map(t => ({value: -t.value, units: t.units}));
      if (terms.length !== 1 || terms[0].value === 0) return null;
      const term = terms[0], units = {};
      for (const [unit, power] of Object.entries(term.units)) units[unit] = -power;
      return [{ value: 1 / term.value, units }];
    }
    if (v instanceof CSSMathSum) {
      const sum = new Map();
      for (const item of v.values) {
        const terms = sumTerms(item);
        if (!terms) return null;
        for (const term of terms) accumulate(sum, term.value, term.units);
      }
      return [...sum.values()];
    }
    if (v instanceof CSSMathProduct) {
      let product = [{value: 1, units: {}}];
      for (const item of v.values) {
        const terms = sumTerms(item);
        if (!terms) return null;
        const next = new Map();
        for (const a of product) for (const b of terms) {
          const units = {...a.units};
          for (const [unit, power] of Object.entries(b.units)) {
            units[unit] = (units[unit] || 0) + power;
            if (!units[unit]) delete units[unit];
          }
          accumulate(next, a.value * b.value, units);
        }
        product = [...next.values()];
      }
      return product;
    }
    if (v instanceof CSSMathMin || v instanceof CSSMathMax || v instanceof CSSMathClamp) {
      const items = v instanceof CSSMathClamp ? [v.lower, v.value, v.upper] : [...v.values];
      const terms = items.map(sumTerms);
      if (terms.some(list => !list || list.length !== 1)) return null;
      const units = terms[0][0].units, key = unitKey(units);
      if (terms.some(list => unitKey(list[0].units) !== key)) return null;
      const numbers = terms.map(list => list[0].value);
      const value = v instanceof CSSMathClamp ? Math.max(numbers[0], Math.min(numbers[1], numbers[2])) :
        (v instanceof CSSMathMin ? Math.min(...numbers) : Math.max(...numbers));
      return [{value, units}];
    }
    return null;
  }
  const sumValues = v => {
    if (v instanceof CSSMathSum) {
      // Most sums contain only scalar units convertible to one canonical unit.
      // Read their current values directly; unit values remain mutable after construction.
      let canonical, total = 0;
      for (const item of v.values) {
        if (!isUnit(item)) { canonical = undefined; break; }
        const info = unitInfo(item.unit), absolute = info && info.f !== null;
        const unit = absolute ? info.canonical : item.unit;
        if (canonical !== undefined && canonical !== unit) { canonical = undefined; break; }
        canonical = unit;
        total += absolute ? item.value * info.f : item.value;
      }
      if (canonical !== undefined && Number.isFinite(total)) return [new CSSUnitValue(total, canonical)];
    }
    const terms = sumTerms(v);
    if (!terms) return null;
    const values = [];
    for (const {value, units} of terms) {
      const keys = Object.keys(units);
      if (keys.length > 1 || (keys.length === 1 && units[keys[0]] !== 1) || !Number.isFinite(value)) return null;
      values.push(new CSSUnitValue(value, keys[0] ?? 'number'));
    }
    return values;
  };
  const convert = (v, unit) => {
    if (v.unit === unit) return new CSSUnitValue(v.value, unit);
    if (unit === 'number' || unit === 'percent' || v.unit === 'number' || v.unit === 'percent') return null;
    const from = unitInfo(v.unit), to = unitInfo(unit);
    if (!from || !to || from.type !== to.type || from.f === null || to.f === null) return null;
    // Rounded to 12 significant digits: the factors are inexact (1cm is 10mm, not 9.999999999999998).
    return new CSSUnitValue(Number((v.value * from.f / to.f).toPrecision(12)), unit);
  };
  const equal = (a, b) => {
    if (a.constructor !== b.constructor) return false;
    if (isUnit(a)) return a.value === b.value && a.unit === b.unit;
    if (a instanceof CSSMathNegate || a instanceof CSSMathInvert) return equal(a.value, b.value);
    if (a instanceof CSSMathClamp) return equal(a.lower, b.lower) && equal(a.value, b.value) && equal(a.upper, b.upper);
    const x = [...a.values], y = [...b.values];
    return x.length === y.length && x.every((v, i) => equal(v, y[i]));
  };

  // Serialization of math values (CSS Typed OM §6.7.3.1).
  const mathText = (v, nested, paren) => {
    if (isUnit(v)) return v.toString();
    const wrap = (s) => (nested ? `(${s})` : `calc(${s})`);
    // A function's arguments are calculations themselves: no calc() and no parentheses.
    const arg = (x) => (x instanceof CSSMathSum || x instanceof CSSMathProduct ? mathText(x, true).slice(1, -1) : mathText(x, false));
    if (v instanceof CSSMathMin || v instanceof CSSMathMax) {
      return `${v instanceof CSSMathMin ? 'min' : 'max'}(${[...v.values].map(arg).join(', ')})`;
    }
    if (v instanceof CSSMathClamp) return `clamp(${arg(v.lower)}, ${arg(v.value)}, ${arg(v.upper)})`;
    if (v instanceof CSSMathNegate) return nested ? `(-${mathText(v.value, true)})` : `calc(-${mathText(v.value, true)})`;
    if (v instanceof CSSMathInvert) return nested ? `(1 / ${mathText(v.value, true)})` : `calc(1 / ${mathText(v.value, true)})`;
    if (v instanceof CSSMathSum) {
      const vals = [...v.values];
      let s = mathText(vals[0], true);
      for (const x of vals.slice(1)) {
        if (x instanceof CSSMathNegate) s += ' - ' + mathText(x.value, true);
        else s += ' + ' + mathText(x, true);
      }
      return wrap(s);
    }
    if (v instanceof CSSMathProduct) {
      const vals = [...v.values];
      let s = mathText(vals[0], true);
      for (const x of vals.slice(1)) {
        if (x instanceof CSSMathInvert) s += ' / ' + mathText(x.value, true);
        else s += ' * ' + mathText(x, true);
      }
      return wrap(s);
    }
    return '';
  };

  // ---------- CSSTransformValue and its components ----------
  const typeIs = (v, t, allowPercent) => {
    const nt = numericType(v);
    if (!nt) return false;
    const keys = TYPES.filter((k) => nt[k]);
    if (t === 'number') return keys.length === 0;
    if (allowPercent && keys.length === 1 && keys[0] === 'percent') return true;
    return keys.length === 1 && keys[0] === t && nt[t] === 1;
  };
  const numberish = (v, what) => {
    const r = typeof v === 'number' ? new CSSUnitValue(v, 'number') : v;
    if (!(r instanceof CSSNumericValue) || !typeIs(r, 'number')) throw new TypeError(`Failed to construct '${what}': Must specify a number unit`);
    return r;
  };
  const need = (v, t, what, pct) => {
    if (!(v instanceof CSSNumericValue) || !typeIs(v, t, pct)) throw new TypeError(`Failed to construct '${what}': Must pass ${t === 'length' && pct ? 'length or percent' : t} to ${what}`);
    return v;
  };
  // A value as px/deg/number for toMatrix(), or a TypeError.
  const absolute = (v, unit) => {
    try { return v.to(unit).value; } catch (e) { throw new TypeError(`Failed to execute 'toMatrix' on 'CSSTransformComponent': Cannot resolve to an absolute value`); }
  };
  class CSSTransformComponent {
    #is2D = true;
    constructor(token) { if (token !== ILLEGAL) throw illegal(); }
    get is2D() { return this.#is2D; }
    set is2D(v) { this.#is2D = !!v; }
    toString() { return ''; }
    toMatrix() { return new DOMMatrix(); }
  }
  class CSSTranslate extends CSSTransformComponent {
    #x; #y; #z;
    constructor(x, y, z = undefined) {
      if (arguments.length < 2) throw new TypeError(`Failed to construct 'CSSTranslate': 2 arguments required, but only ${arguments.length} present.`);
      super(ILLEGAL);
      this.#x = need(x, 'length', 'CSSTranslate', true);
      this.#y = need(y, 'length', 'CSSTranslate', true);
      this.#z = z === undefined ? new CSSUnitValue(0, 'px') : need(z, 'length', 'CSSTranslate', false);
      this.is2D = z === undefined;
    }
    get x() { return this.#x; } set x(v) { this.#x = need(v, 'length', 'CSSTranslate', true); }
    get y() { return this.#y; } set y(v) { this.#y = need(v, 'length', 'CSSTranslate', true); }
    get z() { return this.#z; } set z(v) { this.#z = need(v, 'length', 'CSSTranslate', false); }
    toString() { return this.is2D ? `translate(${this.#x}, ${this.#y})` : `translate3d(${this.#x}, ${this.#y}, ${this.#z})`; }
    toMatrix() {
      const m = new DOMMatrix();
      return this.is2D ? m.translate(absolute(this.#x, 'px'), absolute(this.#y, 'px')) : m.translate(absolute(this.#x, 'px'), absolute(this.#y, 'px'), absolute(this.#z, 'px'));
    }
  }
  class CSSRotate extends CSSTransformComponent {
    #x; #y; #z; #angle;
    constructor(...args) {
      if (args.length !== 1 && args.length !== 4) throw new TypeError(`Failed to construct 'CSSRotate': ${args.length} arguments provided.`);
      super(ILLEGAL);
      if (args.length === 1) {
        this.#x = new CSSUnitValue(0, 'number'); this.#y = new CSSUnitValue(0, 'number'); this.#z = new CSSUnitValue(1, 'number');
        this.#angle = need(args[0], 'angle', 'CSSRotate');
        this.is2D = true;
      } else {
        this.#x = numberish(args[0], 'CSSRotate'); this.#y = numberish(args[1], 'CSSRotate'); this.#z = numberish(args[2], 'CSSRotate');
        this.#angle = need(args[3], 'angle', 'CSSRotate');
        this.is2D = false;
      }
    }
    get x() { return this.#x; } set x(v) { this.#x = numberish(v, 'CSSRotate'); }
    get y() { return this.#y; } set y(v) { this.#y = numberish(v, 'CSSRotate'); }
    get z() { return this.#z; } set z(v) { this.#z = numberish(v, 'CSSRotate'); }
    get angle() { return this.#angle; } set angle(v) { this.#angle = need(v, 'angle', 'CSSRotate'); }
    toString() { return this.is2D ? `rotate(${this.#angle})` : `rotate3d(${this.#x}, ${this.#y}, ${this.#z}, ${this.#angle})`; }
    toMatrix() {
      const a = absolute(this.#angle, 'deg');
      if (this.is2D) return new DOMMatrix().rotateAxisAngle(0, 0, 1, a);
      return new DOMMatrix().rotateAxisAngle(absolute(this.#x, 'number'), absolute(this.#y, 'number'), absolute(this.#z, 'number'), a);
    }
  }
  class CSSScale extends CSSTransformComponent {
    #x; #y; #z;
    constructor(x, y, z = undefined) {
      if (arguments.length < 2) throw new TypeError(`Failed to construct 'CSSScale': 2 arguments required, but only ${arguments.length} present.`);
      super(ILLEGAL);
      this.#x = numberish(x, 'CSSScale'); this.#y = numberish(y, 'CSSScale');
      this.#z = z === undefined ? new CSSUnitValue(1, 'number') : numberish(z, 'CSSScale');
      this.is2D = z === undefined;
    }
    get x() { return this.#x; } set x(v) { this.#x = numberish(v, 'CSSScale'); }
    get y() { return this.#y; } set y(v) { this.#y = numberish(v, 'CSSScale'); }
    get z() { return this.#z; } set z(v) { this.#z = numberish(v, 'CSSScale'); }
    toString() { return this.is2D ? `scale(${this.#x}, ${this.#y})` : `scale3d(${this.#x}, ${this.#y}, ${this.#z})`; }
    toMatrix() {
      const [x, y, z] = [this.#x, this.#y, this.#z].map((v) => absolute(v, 'number'));
      return this.is2D ? new DOMMatrix().scale(x, y) : new DOMMatrix().scale(x, y, z);
    }
  }
  const skewMatrix = (ax, ay) => new DOMMatrix([1, Math.tan(ay * Math.PI / 180), Math.tan(ax * Math.PI / 180), 1, 0, 0]);
  class CSSSkew extends CSSTransformComponent {
    #ax; #ay;
    constructor(ax, ay) {
      if (arguments.length < 2) throw new TypeError(`Failed to construct 'CSSSkew': 2 arguments required, but only ${arguments.length} present.`);
      super(ILLEGAL);
      this.#ax = need(ax, 'angle', 'CSSSkew'); this.#ay = need(ay, 'angle', 'CSSSkew');
    }
    get ax() { return this.#ax; } set ax(v) { this.#ax = need(v, 'angle', 'CSSSkew'); }
    get ay() { return this.#ay; } set ay(v) { this.#ay = need(v, 'angle', 'CSSSkew'); }
    get is2D() { return true; } set is2D(v) {}
    toString() { return `skew(${this.#ax}${this.#ay instanceof CSSUnitValue && this.#ay.value === 0 ? '' : ', ' + this.#ay})`; }
    toMatrix() { return skewMatrix(absolute(this.#ax, 'deg'), absolute(this.#ay, 'deg')); }
  }
  class CSSSkewX extends CSSTransformComponent {
    #ax;
    constructor(ax) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSSkewX': 1 argument required, but only 0 present.");
      super(ILLEGAL);
      this.#ax = need(ax, 'angle', 'CSSSkewX');
    }
    get ax() { return this.#ax; } set ax(v) { this.#ax = need(v, 'angle', 'CSSSkewX'); }
    get is2D() { return true; } set is2D(v) {}
    toString() { return `skewX(${this.#ax})`; }
    toMatrix() { return skewMatrix(absolute(this.#ax, 'deg'), 0); }
  }
  class CSSSkewY extends CSSTransformComponent {
    #ay;
    constructor(ay) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSSkewY': 1 argument required, but only 0 present.");
      super(ILLEGAL);
      this.#ay = need(ay, 'angle', 'CSSSkewY');
    }
    get ay() { return this.#ay; } set ay(v) { this.#ay = need(v, 'angle', 'CSSSkewY'); }
    get is2D() { return true; } set is2D(v) {}
    toString() { return `skewY(${this.#ay})`; }
    toMatrix() { return skewMatrix(0, absolute(this.#ay, 'deg')); }
  }
  const perspectiveLength = (v) => {
    if (typeof v === 'string') v = new CSSKeywordValue(v);
    if (v instanceof CSSKeywordValue) {
      if (v.value !== 'none') throw new TypeError("Failed to construct 'CSSPerspective': Must pass length or none to CSSPerspective");
      return v;
    }
    return need(v, 'length', 'CSSPerspective');
  };
  class CSSPerspective extends CSSTransformComponent {
    #length;
    constructor(length) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSPerspective': 1 argument required, but only 0 present.");
      super(ILLEGAL);
      this.#length = perspectiveLength(length);
    }
    get length() { return this.#length; } set length(v) { this.#length = perspectiveLength(v); }
    get is2D() { return false; } set is2D(v) {}
    toString() {
      const l = this.#length;
      // A negative length is clamped at parse time: kept as written inside calc().
      const text = l instanceof CSSUnitValue && l.value < 0 ? `calc(${l})` : `${l}`;
      return `perspective(${text})`;
    }
    toMatrix() {
      if (this.#length instanceof CSSKeywordValue) return new DOMMatrix();
      const d = Math.max(absolute(this.#length, 'px'), 1);
      const m = new DOMMatrix();
      m.m34 = -1 / d;
      return m;
    }
  }
  class CSSMatrixComponent extends CSSTransformComponent {
    #matrix;
    constructor(matrix, options = undefined) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSMatrixComponent': 1 argument required, but only 0 present.");
      if (!(matrix instanceof DOMMatrixReadOnly)) throw new TypeError("Failed to construct 'CSSMatrixComponent': parameter 1 is not of type 'DOMMatrixReadOnly'.");
      super(ILLEGAL);
      this.#matrix = DOMMatrix.fromMatrix(matrix);
      this.is2D = options != null && options.is2D !== undefined ? !!options.is2D : matrix.is2D;
    }
    get matrix() { return this.#matrix; }
    set matrix(v) {
      if (!(v instanceof DOMMatrixReadOnly)) throw new TypeError("Failed to set the 'matrix' property on 'CSSMatrixComponent': The provided value is not of type 'DOMMatrixReadOnly'.");
      this.#matrix = DOMMatrix.fromMatrix(v);
    }
    toString() {
      const m = this.#matrix;
      if (this.is2D) return `matrix(${[m.a, m.b, m.c, m.d, m.e, m.f].map(fmt).join(', ')})`;
      return `matrix3d(${[m.m11, m.m12, m.m13, m.m14, m.m21, m.m22, m.m23, m.m24, m.m31, m.m32, m.m33, m.m34, m.m41, m.m42, m.m43, m.m44].map(fmt).join(', ')})`;
    }
    toMatrix() {
      const m = this.#matrix;
      return this.is2D ? new DOMMatrix([m.a, m.b, m.c, m.d, m.e, m.f]) : DOMMatrix.fromMatrix(m);
    }
  }
  const isComponent = (v) => v instanceof CSSTransformComponent;
  class CSSTransformValue extends CSSStyleValue {
    #items;
    constructor(transforms) {
      if (arguments.length < 1) throw new TypeError("Failed to construct 'CSSTransformValue': 1 argument required, but only 0 present.");
      super(ILLEGAL, null);
      if (transforms === null || typeof transforms !== 'object' || typeof transforms[Symbol.iterator] !== 'function') {
        throw new TypeError("Failed to construct 'CSSTransformValue': The provided value cannot be converted to a sequence.");
      }
      const list = [...transforms];
      if (!list.length) throw new TypeError("Failed to construct 'CSSTransformValue': CSSTransformValue must have at least one component");
      if (!list.every(isComponent)) throw new TypeError("Failed to construct 'CSSTransformValue': Failed to convert value to 'CSSTransformComponent'.");
      this.#items = list;
      const proxy = new Proxy(this, listHandler(transformItems, (v) => {
        if (!isComponent(v)) throw new TypeError("Failed to set an indexed property on 'CSSTransformValue': Failed to convert value to 'CSSTransformComponent'.");
        return v;
      }, 'CSSTransformValue'));
      proxied.set(proxy, this);
      return proxy;
    }
    get length() { return transformItems(this).length; }
    get is2D() { return transformItems(this).every((c) => c.is2D); }
    toMatrix() { return transformItems(this).reduce((m, c) => m.multiply(c.toMatrix()), new DOMMatrix()); }
    toString() { return transformItems(this).map(String).join(' '); }
    entries() { return transformItems(this).map((m, i) => [i, m])[Symbol.iterator](); }
    keys() { return transformItems(this).map((_, i) => i)[Symbol.iterator](); }
    values() { return transformItems(this).slice()[Symbol.iterator](); }
    forEach(cb, thisArg = undefined) { transformItems(this).forEach((m, i) => apply(cb, thisArg, [m, i, this])); }
    static {
      transformItems = (o) => {
        const t = proxied.get(o) || o;
        if (!(t !== null && typeof t === 'object' && #items in t)) throw new TypeError('Illegal invocation');
        return t.#items;
      };
    }
  }
  Object.defineProperty(CSSTransformValue.prototype, Symbol.iterator, { value: CSSTransformValue.prototype.values, writable: true, configurable: true });
  // Indexed access to a list-like value's items, through a proxy of the value.
  function listHandler(items, convert, iface) {
    return {
      get(t, p) { const i = index(p); if (i >= 0) return items(t)[i]; return Reflect.get(t, p, t); },
      set(t, p, v) {
        const i = index(p);
        if (i < 0) return Reflect.set(t, p, v, t);
        const m = items(t);
        if (i > m.length) throw new RangeError(`Failed to set an indexed property on '${iface}': Index ${i} exceeds index range.`);
        m[i] = convert(v);
        return true;
      },
      has(t, p) { const i = index(p); if (i >= 0) return i < items(t).length; return Reflect.has(t, p); },
      getOwnPropertyDescriptor(t, p) {
        const i = index(p);
        if (i >= 0) { const m = items(t); return i < m.length ? { value: m[i], writable: true, enumerable: true, configurable: true } : undefined; }
        return Reflect.getOwnPropertyDescriptor(t, p);
      },
      ownKeys(t) { return [...items(t).map((_, i) => String(i)), ...Reflect.ownKeys(t)]; },
    };
  }

  // Transform functions as Stylo serializes them, as components (null when one is not reified).
  const transformFrom = (text) => {
    const out = [];
    const re = /([a-zA-Z0-9]+)\(/g;
    let i = 0;
    while (i < text.length) {
      while (text[i] === ' ') i++;
      if (i >= text.length) break;
      re.lastIndex = i;
      const m = re.exec(text);
      if (!m || m.index !== i) return null;
      let depth = 1, j = i + m[0].length;
      for (; j < text.length && depth; j++) { if (text[j] === '(') depth++; else if (text[j] === ')') depth--; }
      const args = splitTop(text.slice(i + m[0].length, j - 1)).map((a) => a.trim());
      const num = (a) => parseNumeric(a);
      const all = args.map(num);
      if (all.some((v) => !v)) return null;
      const name = m[1].toLowerCase();
      const zero = (u) => new CSSUnitValue(0, u);
      try {
        switch (name) {
          case 'translate': out.push(new CSSTranslate(all[0], all[1] ?? zero('px'))); break;
          case 'translatex': out.push(new CSSTranslate(all[0], zero('px'))); break;
          case 'translatey': out.push(new CSSTranslate(zero('px'), all[0])); break;
          case 'translatez': out.push(new CSSTranslate(zero('px'), zero('px'), all[0])); break;
          case 'translate3d': out.push(new CSSTranslate(all[0], all[1], all[2])); break;
          case 'rotate': case 'rotatez': out.push(name === 'rotate' ? new CSSRotate(all[0]) : new CSSRotate(0, 0, 1, all[0])); break;
          case 'rotatex': out.push(new CSSRotate(1, 0, 0, all[0])); break;
          case 'rotatey': out.push(new CSSRotate(0, 1, 0, all[0])); break;
          case 'rotate3d': out.push(new CSSRotate(all[0], all[1], all[2], all[3])); break;
          case 'scale': out.push(new CSSScale(all[0], all[1] ?? all[0])); break;
          case 'scalex': out.push(new CSSScale(all[0], 1)); break;
          case 'scaley': out.push(new CSSScale(1, all[0])); break;
          case 'scalez': out.push(new CSSScale(1, 1, all[0])); break;
          case 'scale3d': out.push(new CSSScale(all[0], all[1], all[2])); break;
          case 'skew': out.push(new CSSSkew(all[0], all[1] ?? zero('deg'))); break;
          case 'skewx': out.push(new CSSSkewX(all[0])); break;
          case 'skewy': out.push(new CSSSkewY(all[0])); break;
          case 'perspective': out.push(new CSSPerspective(all[0])); break;
          case 'matrix': out.push(new CSSMatrixComponent(new DOMMatrixReadOnly(all.map((v) => v.value)))); break;
          case 'matrix3d': out.push(new CSSMatrixComponent(new DOMMatrixReadOnly(all.map((v) => v.value)))); break;
          default: return null;
        }
      } catch (e) { return null; }
      i = j;
    }
    return out.length ? new CSSTransformValue(out) : null;
  };

  // ---------- CSSImageValue ----------
  // An opaque image (`url()`), as Typed OM level 1 reifies <image> values; not constructible.
  class CSSImageValue extends CSSStyleValue {
    constructor(token, text) { if (token !== ILLEGAL) throw illegal(); super(ILLEGAL, text); }
  }

  // ---------- CSSColorValue and its subclasses ----------
  const colorError = (what) => new DE(`Failed to ${what}: Invalid color channel value`, 'SyntaxError');
  const isNumericOf = (v, t) => v instanceof CSSNumericValue && typeIs(v, t);
  const noneKeyword = (v) => {
    const k = typeof v === 'string' ? (v === 'none' ? new CSSKeywordValue('none') : null) : v instanceof CSSKeywordValue && v.value === 'none' ? v : null;
    return k;
  };
  // Channel conversions (CSS Typed OM §4.7.1): `pct` CSSColorPercent, `num` CSSColorNumber,
  // `rgb` CSSColorRGBComp (number or percent), `hue` CSSColorAngle.
  const CHANNEL = {
    pct(v, what) {
      if (typeof v === 'number') return new CSSUnitValue(v * 100, 'percent');
      if (isNumericOf(v, 'percent') || (v instanceof CSSMathValue && typeIs(v, 'percent', true))) return v;
      const k = noneKeyword(v); if (k) return k;
      throw colorError(what);
    },
    num(v, what) {
      if (typeof v === 'number') return new CSSUnitValue(v, 'number');
      if (isNumericOf(v, 'number')) return v;
      const k = noneKeyword(v); if (k) return k;
      throw colorError(what);
    },
    rgb(v, what) {
      if (typeof v === 'number') return new CSSUnitValue(v * 100, 'percent');
      if (v instanceof CSSNumericValue && (typeIs(v, 'number') || typeIs(v, 'percent', true))) return v;
      const k = noneKeyword(v); if (k) return k;
      throw colorError(what);
    },
    // CSSHWB's hue: a CSSNumericValue angle only (a TypeError for anything else).
    angle(v, what) {
      if (!(v instanceof CSSNumericValue)) throw new TypeError(`Failed to ${what}: The provided value is not of type 'CSSNumericValue'.`);
      if (isNumericOf(v, 'angle')) return v;
      throw colorError(what);
    },
    hue(v, what) {
      if (typeof v === 'number') return new CSSUnitValue(v, 'deg');
      if (v === undefined) return new CSSKeywordValue('undefined');
      if (isNumericOf(v, 'angle')) return v;
      const k = noneKeyword(v); if (k) return k;
      throw colorError(what);
    },
  };
  class CSSColorValue extends CSSStyleValue {
    constructor(token) { if (token !== ILLEGAL) throw illegal(); super(ILLEGAL, null); }
    static parse(cssText) {
      if (arguments.length < 1) throw new TypeError("Failed to execute 'parse' on 'CSSColorValue': 1 argument required, but only 0 present.");
      const v = parseColor(`${cssText}`.trim());
      if (!v) throw new DE(`Failed to execute 'parse' on 'CSSColorValue': Invalid color string '${cssText}'`, 'SyntaxError');
      return v;
    }
  }
  // A color class of named channels, each converted as `kinds` says; the last is alpha (100% when absent).
  const colorClass = (name, fn, channels, kinds) => {
    const cls = class extends CSSColorValue {
      #values = {};
      constructor(...args) {
        if (args.length < channels.length - 1) throw new TypeError(`Failed to construct '${name}': ${channels.length - 1} arguments required, but only ${args.length} present.`);
        super(ILLEGAL);
        channels.forEach((c, i) => {
          const v = args[i];
          if (i === channels.length - 1 && v === undefined) { this.#values[c] = new CSSUnitValue(100, 'percent'); return; }
          this.#values[c] = CHANNEL[kinds[i]](v, `construct '${name}'`);
        });
      }
      static {
        channels.forEach((c, i) => {
          const accessors = {
            get [c]() { return this.#values[c]; },
            set [c](v) { this.#values[c] = CHANNEL[kinds[i]](v, `set the '${c}' property on '${name}'`); },
          };
          const desc = Object.getOwnPropertyDescriptor(accessors, c);
          desc.enumerable = true;
          Object.defineProperty(this.prototype, c, desc);
        });
      }
      toString() {
        const v = channels.map((c) => String(this.#values[c]));
        const alpha = v.pop();
        return `${fn}(${v.join(' ')}${alpha === '100%' ? '' : ' / ' + alpha})`;
      }
    };
    Object.defineProperty(cls, 'name', { value: name });
    return cls;
  };
  const CSSRGB = colorClass('CSSRGB', 'rgb', ['r', 'g', 'b', 'alpha'], ['rgb', 'rgb', 'rgb', 'pct']);
  const CSSHSL = colorClass('CSSHSL', 'hsl', ['h', 's', 'l', 'alpha'], ['hue', 'pct', 'pct', 'pct']);
  const CSSHWB = colorClass('CSSHWB', 'hwb', ['h', 'w', 'b', 'alpha'], ['angle', 'pct', 'pct', 'pct']);
  const CSSLab = colorClass('CSSLab', 'lab', ['l', 'a', 'b', 'alpha'], ['pct', 'num', 'num', 'pct']);
  const CSSLCH = colorClass('CSSLCH', 'lch', ['l', 'c', 'h', 'alpha'], ['pct', 'pct', 'hue', 'pct']);
  const CSSOKLab = colorClass('CSSOKLab', 'oklab', ['l', 'a', 'b', 'alpha'], ['pct', 'num', 'num', 'pct']);
  const CSSOKLCH = colorClass('CSSOKLCH', 'oklch', ['l', 'c', 'h', 'alpha'], ['pct', 'pct', 'hue', 'pct']);
  // `rgb()`/`rgba()`, `#hex`, named colors, `hsl()`/`hsla()`, `hwb()` and system colors (keywords).
  const SYSTEM_COLORS = new Set(['accentcolor', 'accentcolortext', 'activetext', 'buttonborder', 'buttonface', 'buttontext', 'canvas',
    'canvastext', 'field', 'fieldtext', 'graytext', 'highlight', 'highlighttext', 'linktext', 'mark', 'marktext', 'selecteditem',
    'selecteditemtext', 'visitedtext', 'currentcolor']);
  const parseColor = (text) => {
    const lower = text.toLowerCase();
    if (SYSTEM_COLORS.has(lower)) return new CSSKeywordValue(lower);
    const named = lower === 'transparent' ? [0, 0, 0] : /^[a-z]+$/.test(lower) ? namedColor(lower) : null;
    if (named) return new CSSRGB(CSS_N(named[0]), CSS_N(named[1]), CSS_N(named[2]), lower === 'transparent' ? new CSSUnitValue(0, 'percent') : undefined);
    const hex = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(text);
    if (hex) {
      let h = hex[1];
      if (h.length <= 4) h = [...h].map((c) => c + c).join('');
      const n = (i) => parseInt(h.slice(i, i + 2), 16);
      return new CSSRGB(CSS_N(n(0)), CSS_N(n(2)), CSS_N(n(4)), h.length === 8 ? new CSSUnitValue(n(6) / 255 * 100, 'percent') : undefined);
    }
    const m = /^(rgba?|hsla?|hwb)\((.*)\)$/i.exec(text);
    if (!m) return null;
    const parts = m[2].replace(/\s*\/\s*/, ' / ').split(/\s*,\s*|\s+/).filter((p) => p !== '');
    const slash = parts.indexOf('/');
    let alphaText = null;
    if (slash >= 0) { alphaText = parts[slash + 1]; parts.splice(slash); }
    else if (parts.length === 4) alphaText = parts.pop();
    if (parts.length !== 3) return null;
    const vals = parts.map((p) => parseNumeric(p));
    if (vals.some((v) => !v)) return null;
    const alpha = alphaText === null ? undefined : (() => { const a = parseNumeric(alphaText); return a && isUnit(a) && a.unit === 'number' ? new CSSUnitValue(a.value * 100, 'percent') : a; })();
    try {
      const f = m[1].toLowerCase();
      if (f.startsWith('rgb')) return new CSSRGB(vals[0], vals[1], vals[2], alpha);
      const hue = isUnit(vals[0]) && vals[0].unit === 'number' ? new CSSUnitValue(vals[0].value, 'deg') : vals[0];
      return new (f === 'hwb' ? CSSHWB : CSSHSL)(hue, vals[1], vals[2], alpha);
    } catch (e) { return null; }
  };
  const CSS_N = (n) => new CSSUnitValue(n, 'number');

  // ---------- Parsing numeric text (dimensions and math functions) ----------
  const NUM = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)(%|[a-zA-Z]+)?/;
  // These leaves belong to the parser and have not escaped to script. Fold
  // their magnitudes without allocating CSSUnitValue copies for conversion.
  function foldParsedUnits(left, right) {
    const a = left instanceof CSSMathNegate ? left.value : left;
    const b = right instanceof CSSMathNegate ? right.value : right;
    if (!isUnit(a) || !isUnit(b)) return null;
    let unit = a.unit, av = a.value, bv = b.value;
    if (unit !== b.unit) {
      const from = unitInfo(unit), other = unitInfo(b.unit);
      if (!from || !other || from.f === null || other.f === null || from.canonical !== other.canonical) return null;
      unit = from.canonical;
      av *= from.f;
      bv *= other.f;
    }
    const value = (left === a ? av : -av) + (right === b ? bv : -bv);
    if (unit === a.unit) { a.value = value; return a; }
    return new CSSUnitValue(value, unit);
  }
  const parseNumeric = (text, simplifyParsedSums = false) => {
    const p = { s: text, i: 0 };
    const ws = () => { while (p.i < p.s.length && /\s/.test(p.s[p.i])) p.i++; };
    const term = () => {
      ws();
      const rest = p.s.slice(p.i);
      const fn = /^(calc|min|max|clamp)\(/i.exec(rest);
      if (fn) {
        p.i += fn[0].length;
        const name = fn[1].toLowerCase();
        const args = [sum()];
        ws();
        while (p.s[p.i] === ',') { p.i++; args.push(sum()); ws(); }
        if (p.s[p.i] !== ')') throw 0;
        p.i++;
        if (name === 'calc') { if (args.length !== 1) throw 0; return args[0]; }
        if (name === 'clamp') { if (args.length !== 3) throw 0; return new CSSMathClamp(...args); }
        return name === 'min' ? new CSSMathMin(...args) : new CSSMathMax(...args);
      }
      if (rest[0] === '(') { p.i++; const v = sum(); ws(); if (p.s[p.i] !== ')') throw 0; p.i++; return v; }
      const m = NUM.exec(rest);
      if (!m) throw 0;
      p.i += m[0].length;
      const unit = m[2] === undefined ? 'number' : m[2] === '%' ? 'percent' : unitName(m[2]);
      if (unit === null) throw 0;
      return new CSSUnitValue(Number(m[1]), unit);
    };
    const product = () => {
      const items = [term()];
      for (;;) {
        ws();
        const op = p.s[p.i];
        if (op !== '*' && op !== '/') break;
        p.i++;
        const t = term();
        items.push(op === '*' ? t : new CSSMathInvert(t));
      }
      return items.length === 1 ? items[0] : new CSSMathProduct(...items);
    };
    const sum = () => {
      const items = [product()];
      for (;;) {
        ws();
        const op = p.s[p.i];
        if ((op !== '+' && op !== '-') || !/\s/.test(p.s[p.i + 1] || '')) break;
        p.i++;
        const t = product();
        items.push(op === '+' ? t : new CSSMathNegate(t));
      }
      if (simplifyParsedSums && items.length === 2) {
        return foldParsedUnits(items[0], items[1]) ?? new CSSMathSum(...items);
      }
      if (simplifyParsedSums && items.length > 2) {
        // Combine compatible numeric leaves while retaining the order and
        // structure of unresolved terms (and their authored math operators).
        const combined = [], indices = new Map();
        for (const item of items) {
          const leaf = item instanceof CSSMathNegate ? item.value : item;
          if (!isUnit(leaf)) { combined.push(item); continue; }
          const info = unitInfo(leaf.unit);
          const canonical = info && info.f !== null ? info.canonical : leaf.unit;
          const index = indices.get(canonical);
          if (index === undefined) {
            indices.set(canonical, combined.length);
            combined.push(item);
          } else {
            combined[index] = foldParsedUnits(combined[index], item);
          }
        }
        return combined.length === 1 ? combined[0] : new CSSMathSum(...combined);
      }
      return items.length === 1 ? items[0] : new CSSMathSum(...items);
    };
    try {
      const top = /^(calc|min|max|clamp)\(/i.test(text);
      const v = sum();
      ws();
      if (p.i !== p.s.length) return null;
      // A bare sum or product is no CSS value: only inside a math function.
      if (!top && !isUnit(v)) return null;
      // calc() of a single value is still a math value (a CSSMathSum of it).
      if (/^calc\(/i.test(text) && !(v instanceof CSSMathValue)) return new CSSMathSum(v);
      return v;
    } catch (e) {
      return null;
    }
  };

  // ---------- Reification (normalized text to CSSStyleValue) ----------
  // Named colors (and `transparent`): <color> values, which Typed OM level 1 does not reify.
  const NAMED_COLORS = new Set(('aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood ' +
    'cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey ' +
    'darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey ' +
    'darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite ' +
    'gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon ' +
    'lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue ' +
    'lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid ' +
    'mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin ' +
    'navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff ' +
    'peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver ' +
    'skyblue slateblue slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow ' +
    'yellowgreen transparent').split(' '));
  // Properties whose percentages are only another way to write a number (see valueText).
  const NUMBER_ONLY = new Set(['shape-image-threshold']);
  // Longhands whose values are several components ({1,2}, {1,4}) or whose grammar level 1 does not
  // reify (as in Chromium): always a plain CSSStyleValue.
  const UNREIFIED = new Set(['border-top-left-radius', 'border-top-right-radius', 'border-bottom-left-radius',
    'border-bottom-right-radius',
    'border-spacing', 'vertical-align']);
  const CSS_WIDE = /^(inherit|initial|unset|revert|revert-layer|default)$/i;
  const OPAQUE_PROPERTIES = new Set(['margin', 'padding', 'border-radius', 'border-width', 'border-color', 'transition', 'flex-flow', 'font', 'font-family', 'font-variant', 'gap', 'grid-gap',
    'grid', 'grid-area', 'grid-template', 'marker', 'mask', 'offset', 'scroll-margin', 'scroll-padding',
    'text-decoration', 'text-shadow', 'will-change']);
  const LIST_VALUED = new Set(['transition', 'transition-property', 'transition-duration', 'transition-timing-function',
    'transition-delay', 'transition-behavior', 'animation', 'animation-name', 'animation-duration', 'animation-timing-function',
    'animation-delay', 'animation-iteration-count', 'animation-direction', 'animation-fill-mode', 'animation-play-state',
    'animation-composition', 'animation-timeline', 'background', 'background-image', 'background-position', 'background-position-x',
    'background-position-y', 'background-size', 'background-repeat', 'background-attachment', 'background-origin', 'background-clip',
    'mask', 'mask-image', 'mask-position', 'mask-size', 'mask-repeat', 'mask-origin', 'mask-clip', 'mask-composite', 'mask-mode',
    'box-shadow', 'text-shadow', 'will-change', 'font-family']);
  const splitSpaces = (text) => {
    const out = [];
    let d = 0, cur = '';
    for (const c of text.trim()) {
      if (c === '(') d++; else if (c === ')') d--;
      if (c === ' ' && !d) { if (cur) out.push(cur); cur = ''; } else cur += c;
    }
    if (cur) out.push(cur);
    return out;
  };
  const splitTop = (text) => {
    const out = [];
    let d = 0, start = 0, q = null;
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      if (q) { if (c === '\\') i++; else if (c === q) q = null; continue; }
      if (c === '"' || c === "'") q = c;
      else if (c === '(') d++;
      else if (c === ')') d--;
      else if (c === ',' && !d) { out.push(text.slice(start, i).trim()); start = i + 1; }
    }
    out.push(text.slice(start).trim());
    return out;
  };
  const COLUMN_RULE_VALUES = new Set(['column-rule-style', 'column-rule-width']);
  const reifyOne = (property, text, custom, shorthand = false) => {
    if (property === 'background-size') {
      // A single size is stored and serialized with an automatic second dimension.
      const pair = splitSpaces(text);
      if (pair.length === 2 && pair[1] === 'auto') text = pair[0];
    }
    if (custom || /var\(/i.test(text)) return unparsedFrom(text);
    if (CSS_WIDE.test(text)) return new CSSKeywordValue(text.toLowerCase());
    if (UNREIFIED.has(property)) return plain(property, text);
    // A grid line's number (with or without a name) is no single numeric value.
    if (/^grid-(row|column)-(start|end)$/.test(property) && !/^[A-Za-z_-][A-Za-z0-9_-]*$/.test(text)) return plain(property, text);
    if (/^-?[A-Za-z_][A-Za-z0-9_-]*$/.test(text)) return NAMED_COLORS.has(text.toLowerCase()) ? plain(property, text) : new CSSKeywordValue(text);
    if (property === 'transform') { const t = transformFrom(text); if (t) return t; }
    if (!shorthand && /^url\((?:"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^()'"\s]*)\)$/i.test(text)) return new CSSImageValue(ILLEGAL, text);
    const n = parseNumeric(text);
    if (n) return n;
    return plain(property, text);
  };
  const BORDER_IMAGE_TUPLES = new Set(['border-image-width', 'border-image-outset', 'border-image-slice', 'border-image-repeat']);
  const reify = (property, text, all, computed = false) => {
    const k = kind(property);
    if (!k) return null;
    const [name, which] = k;
    if (which === 2) return [unparsedFrom(text)];
    if (CSS_WIDE.test(text) || /var\(/i.test(text)) return [reifyOne(name, text, false)];
    if (COLUMN_RULE_VALUES.has(name) || (computed && BORDER_IMAGE_TUPLES.has(name))) return [plain(name, text)];
    // A declared transition-property list is an opaque value unless it is 'none'.
    // A single computed property name reifies as a keyword.
    if (name === 'transition-property' && text !== 'none' && (!computed || splitTop(text).length > 1)) {
      return [plain(name, text)];
    }
    if ((name === 'text-shadow' && text === 'none') || (name === 'will-change' && text === 'auto')) return [new CSSKeywordValue(text)];
    if (OPAQUE_PROPERTIES.has(name)) return [plain(name, text)];
    // A shorthand's value of several components is not reified (a plain CSSStyleValue); one
    // component is, as a longhand's.
    if (which === 1) return [splitSpaces(text).length > 1 ? plain(name, text) : reifyOne(name, text, false, true)];
    const parts = LIST_VALUED.has(name) ? splitTop(text) : [text];
    const values = parts.map((t) => reifyOne(name, t, false));
    if (name === 'background-size' && values.some(value => value.constructor === CSSStyleValue)) {
      return [plain(name, text)];
    }
    return all ? values : values.slice(0, 1);
  };
  const reifyMapValue = (src, name, all) => {
    if (src.computed && name.startsWith('--')) {
      const components = src.components(name);
      if (components) return (all ? components : components.slice(0, 1)).map(([kind, text]) => {
        switch (kind) {
          case 'keyword': return new CSSKeywordValue(text);
          case 'numeric': return parseNumeric(text) ?? plain(name, text);
          case 'transform-function': return transformFrom(text)?.[0] ?? plain(name, text);
          case 'transform': return transformFrom(text) ?? plain(name, text);
          case 'image': if (/^url\(/i.test(text)) return new CSSImageValue(ILLEGAL, text);
        }
        return plain(name, text);
      });
    }
    // CSS column rules store parsed CSS text as a repeatable list. A scalar
    // supplied through Typed OM keeps its numeric or keyword specified value.
    if (COLUMN_RULE_VALUES.has(name)) {
      const typed = src.typedText?.(name);
      if (typed != null) return [reifyOne(name, typed, false)];
    }
    const text = src.get(name);
    return text === '' ? [] : reify(name, text, all, src.computed) || [];
  };
  function parseValues(property, cssText, all, what) {
    const k = kind(property);
    if (!k) throw new TypeError(`Failed to execute '${what}' on 'CSSStyleValue': Invalid property name`);
    const text = parse(k[0], cssText);
    if (text === null) throw new TypeError(`Failed to execute '${what}' on 'CSSStyleValue': The value provided ('${cssText}') could not be parsed as a '${property}'.`);
    if (k[1] === 2) {
      if (cssText.trim() === '') throw new TypeError(`Failed to execute '${what}' on 'CSSStyleValue': The value provided ('${cssText}') could not be parsed as a '${property}'.`);
      return [unparsedFrom(cssText.trim())];
    }
    if (text === '') return [plain(k[0], cssText.trim())];
    return reify(k[0], text, all);
  }

  // ---------- StylePropertyMap ----------
  const nameOf = (property, what, iface) => {
    property = `${property}`;
    const k = kind(property);
    if (!k) throw new TypeError(`Failed to execute '${what}' on '${iface}': Invalid propertyName: ${property}`);
    return k;
  };
  let mapSource;
  class StylePropertyMapReadOnly {
    #source;  // { get(name) -> text, names() -> [names] }
    constructor(token, source) {
      if (token !== ILLEGAL) throw illegal();
      this.#source = source;
    }
    static {
      mapSource = (m) => {
        if (!(m !== null && typeof m === 'object' && #source in m)) throw new TypeError('Illegal invocation');
        return m.#source;
      };
    }
    get(property) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'get' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present.");
      const [name] = nameOf(property, 'get', 'StylePropertyMapReadOnly');
      return reifyMapValue(src, name, false)[0];
    }
    getAll(property) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'getAll' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present.");
      const [name] = nameOf(property, 'getAll', 'StylePropertyMapReadOnly');
      return reifyMapValue(src, name, true);
    }
    has(property) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'has' on 'StylePropertyMapReadOnly': 1 argument required, but only 0 present.");
      const [name] = nameOf(property, 'has', 'StylePropertyMapReadOnly');
      return src.get(name) !== '';
    }
    get size() { return mapSource(this).names().length; }
    entries() {
      const src = mapSource(this);
      return src.names().map((n) => [n, reifyMapValue(src, n, true)])[Symbol.iterator]();
    }
    keys() { const src = mapSource(this); return src.names()[Symbol.iterator](); }
    values() { const src = mapSource(this); return src.names().map((n) => reifyMapValue(src, n, true))[Symbol.iterator](); }
    forEach(cb, thisArg = undefined) {
      for (const [k, v] of this.entries()) apply(cb, thisArg, [v, k, this]);
    }
  }
  Object.defineProperty(StylePropertyMapReadOnly.prototype, Symbol.iterator, { value: StylePropertyMapReadOnly.prototype.entries, writable: true, configurable: true });

  // A value given to set()/append() as text for `name`: a CSSStyleValue's serialization (checked
  // against the property), or a string parsed as the property.
  const integerLonghands = new Set(['order', 'orphans', 'widows', 'z-index', 'column-count', 'math-depth']);
  const opacityLonghands = new Set(['opacity', 'fill-opacity', 'flood-opacity', 'stop-opacity', 'stroke-opacity']);
  // Step keywords that Stylo stores as steps(1, start|end); their specified Typed OM value stays the keyword.
  const STEP_KEYWORDS = new Set(['step-start', 'step-end']);
  const timingFunctionLonghands = new Set(['animation-timing-function', 'transition-timing-function']);
  function retainsTypedOrigin(name, value, text) {
    if (timingFunctionLonghands.has(name)) return value instanceof CSSKeywordValue && STEP_KEYWORDS.has(text);
    if (name === 'column-rule-width' || name === 'column-rule-style')
      return value instanceof CSSNumericValue || value instanceof CSSKeywordValue && !CSS_WIDE.test(value.value);
    if (opacityLonghands.has(name)) return value instanceof CSSMathValue || isUnit(value) && value.unit === 'percent';
    return integerLonghands.has(name) && value instanceof CSSNumericValue && text.startsWith('calc(');
  }
  const valueText = (name, which, v, what) => {
    if (v instanceof CSSStyleValue) {
      if (associatedProperties.has(v) && associatedProperties.get(v) !== name) {
        throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Value belongs to another property`);
      }
      if (which === 2) {
        if (!(v instanceof CSSUnparsedValue)) throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
        return v.toString();
      }
      // A property whose CSS grammar takes a percentage that it only stands for a number
      // (`shape-image-threshold: 50%` is 0.5): its Typed OM type is <number>, as in Chromium.
      if (NUMBER_ONLY.has(name) && v instanceof CSSNumericValue && typeIs(v, 'percent', true)) {
        throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
      }
      const text = v.toString();
      const typed = !(v instanceof CSSUnparsedValue) && v.constructor !== CSSStyleValue && !(v instanceof CSSKeywordValue && CSS_WIDE.test(v.value));
      // A numeric object cannot stand for an opaque shorthand or a CSS grid line.
      // A CSSStyleValue parsed for the property can still carry its complete grammar.
      if (v instanceof CSSNumericValue && ((which === 1 && OPAQUE_PROPERTIES.has(name)) ||
          /^grid-(row|column)-(start|end)$/.test(name))) {
        throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
      }
      // A number is no length: unitless 0 parses as one, so a property that takes numbers must take 1.
      if (isUnit(v) && v.unit === 'number' && v.value === 0 && parse(name, '1') === null) {
        throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
      }
      // A shorthand takes a typed value only where it stands for the whole shorthand (one component).
      if (typed && which === 1) {
        if (v instanceof CSSNumericValue && /^border(?:-(?:top|right|bottom|left|(?:block|inline)(?:-(?:start|end))?))?$/.test(name)) {
          throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
        }
        const normalized = parse(name, text);
        if (normalized === null || splitSpaces(normalized).length > 1) throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
        return text;
      }
      if (parse(name, text) !== null) return text;
      // Typed OM represents a fractional <number> assigned to an <integer> as
      // a one-item sum. CSS calc() rounds it when the property computes.
      if (isUnit(v) && v.unit === 'number' && !Number.isInteger(v.value) && integerLonghands.has(name)) {
        const wrapped = `calc(${text})`;
        if (parse(name, wrapped) !== null) return wrapped;
      }
      // A value of the right type outside the property's range is kept, in calc() (which clamps).
      // (Only below the range: its absolute value must be valid, so a non-integer stays invalid for an integer.)
      const below = v.value < 0 ? parse(name, unitText(-v.value, v.unit)) !== null : v.unit === 'number' && parse(name, '1') !== null && (Number.isInteger(v.value) || parse(name, '1.5') !== null)
        // A grid line's 0 is no range violation: it is not a line at all.
        && !/^grid-(row|column)-(start|end)$/.test(name);
      if (isUnit(v) && below && parse(name, `calc(${text})`) !== null) {
        return `calc(${text})`;
      }
      throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
    }
    const text = `${v}`;
    if (parse(name, text) === null) throw new TypeError(`Failed to execute '${what}' on 'StylePropertyMap': Invalid type for property`);
    return text;
  };

  class StylePropertyMap extends StylePropertyMapReadOnly {
    constructor(token, source) {
      if (token !== ILLEGAL) throw illegal();
      super(ILLEGAL, source);
    }
    set(property, ...values) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': 1 argument required, but only 0 present.");
      const [name, which] = nameOf(property, 'set', 'StylePropertyMap');
      if (values.length !== 1 && (which !== 0 || !LIST_VALUED.has(name))) {
        if (values.length === 0 || which === 2 || !LIST_VALUED.has(name)) throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': Invalid type for property");
      }
      if (values.length > 1 && values.some((v) => v instanceof CSSUnparsedValue || (typeof v === 'string' && /var\(/i.test(v)))) {
        throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': Invalid type for property");
      }
      const texts = values.map((v) => valueText(name, which, v, 'set'));
      const text = texts.join(', ');
      if (values.length > 1 && parse(name, text) === null) throw new TypeError("Failed to execute 'set' on 'StylePropertyMap': Invalid type for property");
      if (which === 0 && values.length === 1 && retainsTypedOrigin(name, values[0], text)) {
        src.setTyped(name, text);
      } else {
        src.set(name, text);
      }
    }
    append(property, ...values) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': 1 argument required, but only 0 present.");
      const [name, which] = nameOf(property, 'append', 'StylePropertyMap');
      if (which !== 0 || !LIST_VALUED.has(name)) throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Property does not support multiple values");
      if (!values.length) return;
      const current = src.get(name);
      if (CSS_WIDE.test(current) || /var\(/i.test(current)) throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Invalid type for property");
      if (values.some((v) => v instanceof CSSUnparsedValue || (typeof v === 'string' && /var\(/i.test(v)))) {
        throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Invalid type for property");
      }
      const texts = values.map((v) => valueText(name, which, v, 'append'));
      const combined = [current, ...texts].filter((t) => t !== '').join(', ');
      if (texts.some((t) => CSS_WIDE.test(t)) || parse(name, combined) === null) {
        throw new TypeError("Failed to execute 'append' on 'StylePropertyMap': Invalid type for property");
      }
      src.set(name, combined);
    }
    delete(property) {
      const src = mapSource(this);
      if (arguments.length < 1) throw new TypeError("Failed to execute 'delete' on 'StylePropertyMap': 1 argument required, but only 0 present.");
      const [name] = nameOf(property, 'delete', 'StylePropertyMap');
      src.set(name, '');
    }
    clear() { mapSource(this).clear(); }
  }

  // `element.attributeStyleMap`: the element's inline declarations (`element.style`).
  const styleMaps = new WeakMap();
  const declOf = (el) => {
    for (let p = Object.getPrototypeOf(el); p; p = Object.getPrototypeOf(p)) {
      const d = Object.getOwnPropertyDescriptor(p, 'style');
      if (d && d.get) return apply(d.get, el, []);
    }
    return null;
  };
  // A declaration block's map (`element.attributeStyleMap`, `CSSStyleRule.styleMap`): one per block.
  const declarationMap = (d) => {
    let m = styleMaps.get(d);
    if (m) return m;
    m = new StylePropertyMap(ILLEGAL, {
      get: (name) => d.getPropertyValue(name),
      typedText: (name) => native.typedNumericText(d, name),
      setTyped: (name, text) => native.setTypedNumeric(d, name, text),
      set: (name, text) => { if (text === '') d.removeProperty(name); else d.setProperty(name, text); },
      clear: () => { d.cssText = ''; },
      names: () => {
        const seen = new Set();
        for (let i = 0; i < d.length; i++) seen.add(d.item(i));
        return [...seen];
      },
    });
    styleMaps.set(d, m);
    return m;
  };
  const attributeStyleMap = (el) => declarationMap(declOf(el));
  // `element.computedStyleMap()`: the element's computed values (`getComputedStyle`).
  const RESOLVED = /^((min-|max-)?(width|height|inline-size|block-size)|(margin|padding|inset)(-(top|right|bottom|left|block|inline|block-start|block-end|inline-start|inline-end))?|top|right|bottom|left|transform|line-height|border-(top|right|bottom|left|(block|inline)-(start|end))-width)$/;
  // Standard names precede vendor-prefixed names, then custom names. Chrome orders custom
  // names by UTF-16 code unit (so `--💩`, a surrogate pair, sorts before `--豈` U+F900),
  // which is what css-typed-om's computed iterable test expects.
  const propertyGroup = (name) => name.startsWith('--') ? 2 : name.startsWith('-') ? 1 : 0;
  const compareComputedPropertyNames = (a, b) => {
    const group = propertyGroup(a) - propertyGroup(b);
    if (group || a === b) return group;
    return a < b ? -1 : 1;
  };
  const computedStyleMap = (el, computedValue) => {
    const cs = () => getComputedStyle(el);
    return new StylePropertyMapReadOnly(ILLEGAL, {
      computed: true,
      components: computedValue,
      // Computed values: for the properties whose getComputedStyle value is a resolved (used) one,
      // the style engine's computed value; getComputedStyle's for the rest.
      get: (name) => (RESOLVED.test(name) ? computedValue(name) : null) ?? cs().getPropertyValue(name),
      names: () => {
        const s = cs();
        const names = [];
        for (let i = 0; i < s.length; i++) names.push(s.item(i));
        return names.sort(compareComputedPropertyNames);
      },
    });
  };

  // The CSS.* numeric factories.
  const factories = {};
  for (const unit of ['number', 'percent', ...Object.keys(UNITS)]) {
    if (unit === 'x') continue;
    const name = CASED[unit] || unit;
    factories[name] = { [name](value) { return new CSSUnitValue(value, unit); } }[name];
  }

  const enumerate = (proto, names) => {
    for (const name of names) {
      const desc = Object.getOwnPropertyDescriptor(proto, name);
      if (!desc) continue;
      desc.enumerable = true;
      Object.defineProperty(proto, name, desc);
    }
  };
  const classes = { CSSStyleValue, CSSUnparsedValue, CSSVariableReferenceValue, CSSKeywordValue, CSSNumericValue,
    CSSUnitValue, CSSMathValue, CSSMathSum, CSSMathProduct, CSSMathNegate, CSSMathInvert, CSSMathMin, CSSMathMax,
    CSSMathClamp, CSSNumericArray, StylePropertyMapReadOnly, StylePropertyMap, CSSTransformValue, CSSTransformComponent,
    CSSTranslate, CSSRotate, CSSScale, CSSSkew, CSSSkewX, CSSSkewY, CSSPerspective, CSSMatrixComponent,
    CSSImageValue, CSSColorValue, CSSRGB, CSSHSL, CSSHWB, CSSLab, CSSLCH, CSSOKLab, CSSOKLCH };
  for (const [name, cls] of Object.entries(classes)) {
    const names = Object.getOwnPropertyNames(cls.prototype).filter((n) => n !== 'constructor');
    enumerate(cls.prototype, names);
    Object.defineProperty(cls.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }
  return { ...classes, factories, attributeStyleMap, computedStyleMap, declarationMap };
});
