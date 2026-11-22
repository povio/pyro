// IndexedDB API objects and state machine for one realm, made on its first
// IndexedDB use (see indexeddb.rs). `N` holds the natives: storage of records
// in key order, the cross-realm registry of connections, locks and upgrades,
// and `post()`, which queues one task that calls `step()`. A step returns
// {t: target, e: event, a: after(notCanceled, threw)} to have the event
// dispatched natively; listeners then run as outermost calls, so microtasks
// run after each one while the transaction is still active.
//
// Keys travel as ASCII strings whose code unit order is IndexedDB key order:
//   number '1' + 16 hex (order-preserving float64 bits), date '2' + 16 hex,
//   string '3' + 4 hex per code unit + '!', binary '4' + 2 hex per byte + '!',
//   array '5' + elements + '!'.
// Shapes, messages and exception order follow the spec and Chrome.
(function (N) {
  'use strict';
  const O = Object, A = Array, TE = TypeError, DE = DOMException, ET = EventTarget, EV = Event;
  const defineProperty = O.defineProperty, getPrototypeOf = O.getPrototypeOf, descriptor = O.getOwnPropertyDescriptor;
  const hasOwn = (o, k) => O.prototype.hasOwnProperty.call(o, k);
  const isArray = A.isArray, apply = Reflect.apply, stringify = JSON.stringify, parse = JSON.parse;
  const fromCharCode = String.fromCharCode;
  const AB = ArrayBuffer, U8 = Uint8Array, DV = DataView, isView = AB.isView;
  const getter = (proto, key) => descriptor(proto, key).get;
  const abLength = getter(AB.prototype, 'byteLength');
  const typedProto = getPrototypeOf(U8.prototype);
  const typedBuffer = getter(typedProto, 'buffer'), typedOffset = getter(typedProto, 'byteOffset');
  const typedBytes = getter(typedProto, 'byteLength');
  const dvBuffer = getter(DV.prototype, 'buffer'), dvOffset = getter(DV.prototype, 'byteOffset');
  const dvBytes = getter(DV.prototype, 'byteLength');
  const u8Slice = U8.prototype.slice;
  const dateTime = Date.prototype.getTime;
  const iteratorKey = Symbol.iterator, tagKey = Symbol.toStringTag;
  const token = Symbol('IndexedDB');
  const illegal = () => new TE('Illegal constructor');
  const fail = (name, message) => new DE(message, name);
  // Class syntax supplies the IDL getters and operations. Web IDL makes
  // string-named members enumerable; the constructor and symbols stay hidden.
  function enumerableMembers(holder, names) {
    for (let i = 0; i < names.length; i++) defineProperty(holder, names[i], { enumerable: true });
  }
  function tag(C, name, ...members) {
    enumerableMembers(C.prototype, members);
    defineProperty(C.prototype, tagKey, { value: name, configurable: true });
  }
  const domString = (v) => `${v}`;

  // ---- Keys ----
  const HEX = '0123456789abcdef';
  const f64 = new Float64Array(1), u32 = new Uint32Array(f64.buffer);
  const hex8 = (n) => {
    let out = '';
    for (let i = 28; i >= 0; i -= 4) out += HEX[(n >>> i) & 15];
    return out;
  };
  function encNumber(n) {
    f64[0] = n === 0 ? 0 : n;
    let hi = u32[1], lo = u32[0];
    if (hi >>> 31) {
      hi = ~hi >>> 0;
      lo = ~lo >>> 0;
    } else {
      hi = (hi | 0x80000000) >>> 0;
    }
    return hex8(hi) + hex8(lo);
  }
  function decNumber(s, i) {
    let hi = parseInt(s.slice(i, i + 8), 16), lo = parseInt(s.slice(i + 8, i + 16), 16);
    if (hi >>> 31) {
      hi = (hi & 0x7fffffff) >>> 0;
    } else {
      hi = ~hi >>> 0;
      lo = ~lo >>> 0;
    }
    u32[1] = hi;
    u32[0] = lo;
    return f64[0];
  }
  const numberKey = (n) => '1' + encNumber(n);
  function encString(s) {
    let out = '3';
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      out += HEX[c >>> 12] + HEX[(c >>> 8) & 15] + HEX[(c >>> 4) & 15] + HEX[c & 15];
    }
    return out + '!';
  }
  // The bytes of a buffer source, null for other values, undefined when detached.
  function bufferBytes(v) {
    let buffer, offset, length;
    try {
      length = apply(abLength, v, []);
      buffer = v;
      offset = 0;
    } catch (_) {
      if (!isView(v)) return null;
      try {
        buffer = apply(typedBuffer, v, []);
        offset = apply(typedOffset, v, []);
        length = apply(typedBytes, v, []);
      } catch (_) {
        buffer = apply(dvBuffer, v, []);
        offset = apply(dvOffset, v, []);
        length = apply(dvBytes, v, []);
      }
    }
    try {
      return apply(u8Slice, new U8(buffer, offset, length), []);
    } catch (_) {
      return undefined;
    }
  }
  // "Convert a value to a key": the encoded key, or undefined when invalid.
  // Exceptions (getters on arrays) propagate.
  function toKey(input, seen) {
    const type = typeof input;
    if (type === 'number') return input !== input ? undefined : numberKey(input);
    if (type === 'string') return encString(input);
    if (input === null || type !== 'object') return undefined;
    let time, date = true;
    try {
      time = apply(dateTime, input, []);
    } catch (_) {
      date = false;
    }
    if (date) return time !== time ? undefined : '2' + encNumber(time);
    const bytes = bufferBytes(input);
    if (bytes !== null) {
      if (bytes === undefined) return undefined;
      let out = '4';
      for (let i = 0; i < bytes.length; i++) out += HEX[bytes[i] >>> 4] + HEX[bytes[i] & 15];
      return out + '!';
    }
    if (N.isArrayKeyObject(input)) {
      if (!seen) seen = new Set();
      if (seen.has(input)) return undefined;
      seen.add(input);
      const length = input.length;
      let out = '5';
      for (let i = 0; i < length; i++) {
        if (!hasOwn(input, i)) return undefined;
        const key = toKey(input[i], seen);
        if (key === undefined) return undefined;
        out += key;
      }
      return out + '!';
    }
    return undefined;
  }
  function mustKey(v, what) {
    const key = toKey(v);
    if (key === undefined) throw fail('DataError', `The ${what || 'parameter'} is not a valid key.`);
    return key;
  }
  function decodeAt(s, st) {
    const t = s[st.i++];
    if (t === '1' || t === '2') {
      const v = decNumber(s, st.i);
      st.i += 16;
      return t === '1' ? v : new Date(v);
    }
    if (t === '3') {
      let out = '', units = [];
      while (s[st.i] !== '!') {
        own(units, units.length, parseInt(s.slice(st.i, st.i + 4), 16));
        st.i += 4;
        if (units.length === 4096) {
          out += apply(fromCharCode, null, units);
          units = [];
        }
      }
      st.i++;
      return out + apply(fromCharCode, null, units);
    }
    if (t === '4') {
      const bytes = [];
      while (s[st.i] !== '!') {
        own(bytes, bytes.length, parseInt(s.slice(st.i, st.i + 2), 16));
        st.i += 2;
      }
      st.i++;
      return new U8(bytes).buffer;
    }
    const out = [];
    while (s[st.i] !== '!') own(out, out.length, decodeAt(s, st));
    st.i++;
    return out;
  }
  const decodeKey = (s) => (s === undefined || s === null ? undefined : decodeAt(s, { i: 0 }));

  // ---- Web IDL conversions ----
  function enforceRange(v, max, what) {
    const n = +v;
    if (n !== n || n === Infinity || n === -Infinity) throw new TE(`${what} is not a finite number.`);
    const t = Math.trunc(n);
    if (t < 0 || t > max) throw new TE(`${what} is outside the valid range.`);
    return t;
  }
  const unsignedLong = (v, what) => (v === undefined ? 0 : enforceRange(v, 4294967295, what));
  const DIRECTIONS = ['next', 'nextunique', 'prev', 'prevunique'];
  function direction(v) {
    if (v === undefined) return 0;
    const i = DIRECTIONS.indexOf(domString(v));
    if (i < 0) throw new TE(`The provided value '${domString(v)}' is not a valid enum value of type IDBCursorDirection.`);
    return i;
  }
  const dictionary = (v) => {
    if (v === undefined || v === null) return {};
    if (typeof v !== 'object' && typeof v !== 'function') throw new TE('The provided value is not of type dictionary.');
    return v;
  };
  const iterableObject = (v) =>
    v !== null && (typeof v === 'object' || typeof v === 'function') && typeof v[iteratorKey] === 'function';
  const toStrings = (v) => A.from(v, domString);

  // ---- Key ranges ----
  let isRange, rangeParts;
  class IDBKeyRange {
    #lower;
    #upper;
    #lowerValue;
    #upperValue;
    #lowerOpen;
    #upperOpen;
    constructor(t = undefined, lower, upper, lowerOpen, upperOpen) {
      if (t !== token) throw illegal();
      this.#lower = lower;
      this.#upper = upper;
      this.#lowerValue = decodeKey(lower);
      this.#upperValue = lower === upper ? this.#lowerValue : decodeKey(upper);
      this.#lowerOpen = lowerOpen;
      this.#upperOpen = upperOpen;
    }
    static {
      isRange = (r) => r !== null && typeof r === 'object' && #lower in r;
      rangeParts = (r) => [r.#lower ?? null, r.#upper ?? null, r.#lowerOpen, r.#upperOpen];
    }
    get lower() {
      return this.#lowerValue;
    }
    get upper() {
      return this.#upperValue;
    }
    get lowerOpen() {
      return this.#lowerOpen;
    }
    get upperOpen() {
      return this.#upperOpen;
    }
    includes(key) {
      if (!arguments.length) throw new TE("Failed to execute 'includes' on 'IDBKeyRange': 1 argument required.");
      return inRange(rangeParts(this), mustKey(key));
    }
    static only(value) {
      if (!arguments.length) throw new TE("Failed to execute 'only' on 'IDBKeyRange': 1 argument required.");
      const key = mustKey(value);
      return new IDBKeyRange(token, key, key, false, false);
    }
    static lowerBound(lower, open = false) {
      if (!arguments.length) throw new TE("Failed to execute 'lowerBound' on 'IDBKeyRange': 1 argument required.");
      return new IDBKeyRange(token, mustKey(lower), undefined, !!open, true);
    }
    static upperBound(upper, open = false) {
      if (!arguments.length) throw new TE("Failed to execute 'upperBound' on 'IDBKeyRange': 1 argument required.");
      return new IDBKeyRange(token, undefined, mustKey(upper), true, !!open);
    }
    static bound(lower, upper, lowerOpen = false, upperOpen = false) {
      if (arguments.length < 2) throw new TE("Failed to execute 'bound' on 'IDBKeyRange': 2 arguments required.");
      const l = mustKey(lower), u = mustKey(upper);
      if (l > u) throw fail('DataError', 'The lower key is greater than the upper key.');
      if (l === u && (lowerOpen || upperOpen)) throw fail('DataError', 'The lower key and upper key are equal and one of the bounds is open.');
      return new IDBKeyRange(token, l, u, !!lowerOpen, !!upperOpen);
    }
  }
  enumerableMembers(IDBKeyRange, ['only', 'lowerBound', 'upperBound', 'bound']);
  tag(IDBKeyRange, 'IDBKeyRange', 'lower', 'upper', 'lowerOpen', 'upperOpen', 'includes');
  function inRange([lower, upper, lowerOpen, upperOpen], key) {
    if (lower !== null && (key < lower || (lowerOpen && key === lower))) return false;
    if (upper !== null && (key > upper || (upperOpen && key === upper))) return false;
    return true;
  }
  const ALL = [null, null, false, false];
  // "Convert a value to a key range".
  function toRange(value, nullDisallowed) {
    if (isRange(value)) return rangeParts(value);
    if (value === undefined || value === null) {
      if (nullDisallowed) throw fail('DataError', 'No key or key range specified.');
      return ALL;
    }
    const key = mustKey(value);
    return [key, key, false, false];
  }
  // A "potentially valid key range": anything but a dictionary of options.
  function potentiallyRange(v) {
    if (isRange(v) || v === null || typeof v !== 'object') return true;
    if (isArray(v) || bufferBytes(v) !== null) return true;
    try {
      apply(dateTime, v, []);
      return true;
    } catch (_) {
      return false;
    }
  }

  // ---- Key paths ----
  const IDENT = /^[$_\p{ID_Start}][$‌‍\p{ID_Continue}]*$/u;
  function validPath(p) {
    if (typeof p === 'string') return p === '' || p.split('.').every((id) => IDENT.test(id));
    return isArray(p) && p.length > 0 && p.every((x) => typeof x === 'string' && validPath(x));
  }
  function toKeyPath(v) {
    if (v === undefined || v === null) return null;
    return iterableObject(v) ? toStrings(v) : domString(v);
  }
  const FAIL = Symbol('failure');
  // "Evaluate a key path on a value" (values are clones: own data only).
  function evalPath(value, path) {
    if (isArray(path)) {
      const out = [];
      for (const p of path) {
        const v = evalPath(value, p);
        if (v === FAIL) return FAIL;
        out.push(v);
      }
      return out;
    }
    if (path === '') return value;
    for (const id of path.split('.')) {
      if (typeof value === 'string' && id === 'length') value = value.length;
      else if (isArray(value) && id === 'length') value = value.length;
      else if (value === null || typeof value !== 'object') return FAIL;
      else if (hasOwn(value, id)) value = value[id];
      else if (id === 'size' || id === 'type' || id === 'name' || id === 'lastModified') {
        value = N.blobKeyPath(value, id);
        if (value === undefined) return FAIL;
      } else return FAIL;
    }
    return value;
  }
  // The keys a record has in an index (none when the path yields no valid key).
  function indexKeys(im, value) {
    const v = evalPath(value, im.keyPath);
    if (v === FAIL) return [];
    if (im.multiEntry && isArray(v)) {
      const seen = new Set([v]), keys = [];
      for (let i = 0; i < v.length; i++) {
        if (!hasOwn(v, i)) continue;
        const k = toKey(v[i], seen);
        if (k !== undefined && !keys.includes(k)) keys.push(k);
      }
      return keys;
    }
    const k = toKey(v);
    return k === undefined ? [] : [k];
  }
  function canInject(value, path) {
    const ids = path.split('.');
    ids.pop();
    for (const id of ids) {
      if (value === null || typeof value !== 'object') return false;
      if (!hasOwn(value, id)) return true;
      value = value[id];
    }
    return value !== null && typeof value === 'object';
  }
  const own = (o, k, value) => defineProperty(o, k, { value, writable: true, enumerable: true, configurable: true });
  function inject(value, path, key) {
    const ids = path.split('.');
    const last = ids.pop();
    for (const id of ids) {
      if (!hasOwn(value, id)) own(value, id, {});
      value = value[id];
    }
    own(value, last, key);
  }
  const pathValue = (p) => (isArray(p) ? p.slice() : p);

  // ---- DOMStringList, IDBRecord ----
  let listOf;
  class DOMStringList {
    #list;
    constructor(t = undefined, list) {
      if (t !== token) throw illegal();
      this.#list = list;
      for (let i = 0; i < list.length; i++) defineProperty(this, i, { value: list[i], enumerable: true });
    }
    static {
      listOf = (l) => l.#list;
    }
    get length() {
      return this.#list.length;
    }
    item(index) {
      if (!arguments.length) throw new TE("Failed to execute 'item' on 'DOMStringList': 1 argument required.");
      return this.#list[+index >>> 0] ?? null;
    }
    contains(string) {
      return this.#list.includes(domString(string));
    }
    [iteratorKey]() {
      return this.#list.values();
    }
  }
  tag(DOMStringList, 'DOMStringList', 'length', 'item', 'contains');
  const sortedList = (names) => new DOMStringList(token, [...names].sort());

  class IDBRecord {
    #key;
    #primaryKey;
    #value;
    constructor(t = undefined, key, primaryKey, value) {
      if (t !== token) throw illegal();
      this.#key = key;
      this.#primaryKey = primaryKey;
      this.#value = value;
    }
    get key() {
      return this.#key;
    }
    get primaryKey() {
      return this.#primaryKey;
    }
    get value() {
      return this.#value;
    }
  }
  tag(IDBRecord, 'IDBRecord', 'key', 'primaryKey', 'value');

  // ---- Event handler attributes ----
  // The host EventTarget owns this state: same-origin realms borrowing an
  // accessor see the same handler, and a forged prototype carries no brand.
  function initHandlerState(target, state) {
    state.assign = assignHandler;
    N.handlerState(target, state);
  }
  function assignHandler(target, h, type, value) {
    const previous = h[type] ?? null;
    const next = typeof value === 'function' || (typeof value === 'object' && value !== null) ? value : null;
    h[type] = next;
    if (previous === null && next !== null) {
      // Install from the target's realm even when another realm's setter is
      // borrowed. A replacement keeps the registration in its current position.
      N.setHandler(target, type, event => {
        if (apply(h[type], target, [event]) === false) N.cancelEvent(event);
      });
    } else if (previous !== null && next === null) {
      N.setHandler(target, type, null);
    }
  }
  function handlers(C, brand, ...types) {
    function state(receiver) {
      const h = N.handlerState(receiver);
      if (!h || !h[brand]) throw new TE('Illegal invocation');
      return h;
    }
    for (let i = 0; i < types.length; i++) {
      const type = types[i], name = 'on' + type;
      const accessors = {
        get [name]() { return state(this)[type] ?? null; },
        set [name](v) {
          const h = state(this);
          h.assign(this, h, type, v);
        },
      };
      defineProperty(C.prototype, name, descriptor(accessors, name));
    }
  }

  class IDBVersionChangeEvent extends EV {
    #oldVersion;
    #newVersion;
    constructor(type, init = undefined) {
      if (!arguments.length) throw new TE("Failed to construct 'IDBVersionChangeEvent': 1 argument required.");
      const d = dictionary(init);
      super(type, d);
      const nv = d.newVersion, ov = d.oldVersion;
      this.#newVersion = nv === undefined || nv === null ? null : Math.trunc(+nv) || 0;
      this.#oldVersion = ov === undefined ? 0 : Math.trunc(+ov) || 0;
    }
    get oldVersion() {
      return this.#oldVersion;
    }
    get newVersion() {
      return this.#newVersion;
    }
  }
  tag(IDBVersionChangeEvent, 'IDBVersionChangeEvent', 'oldVersion', 'newVersion');
  const versionEvent = (type, oldVersion, newVersion) =>
    new IDBVersionChangeEvent(type, { oldVersion, newVersion });
  const plainEvent = (type, bubbles, cancelable) => new EV(type, { bubbles, cancelable });

  // ---- Tasks ----
  const jobs = [];
  function post(job) {
    jobs.push(job);
    N.post();
  }
  function step() {
    const job = jobs.shift();
    return job ? job() : undefined;
  }

  // Waiting for other connections, locks or another realm's upgrade.
  const waiters = new Map();
  function waitFor(name, fn) {
    let list = waiters.get(name);
    if (!list) {
      waiters.set(name, (list = []));
      N.watch(name, true);
    }
    list.push(fn);
  }
  function runWaiters(name) {
    const list = waiters.get(name);
    if (!list) return;
    waiters.delete(name);
    N.watch(name, false);
    for (const fn of list) fn();
  }
  const wakeLocal = (name) => post(() => runWaiters(name));

  // ---- Requests ----
  let requestState;
  class IDBRequest extends ET {
    #s;
    constructor(t = undefined, open = false) {
      if (t !== token) throw illegal();
      super();
      initHandlerState(this, { __proto__: null, request: true, open });
      this.#s = { done: false, result: undefined, error: null, source: null, tx: null };
    }
    static {
      requestState = (r) => r.#s;
    }
    get result() {
      const s = this.#s;
      if (!s.done) throw fail('InvalidStateError', "Failed to read the 'result' property from 'IDBRequest': The request has not finished.");
      return s.result;
    }
    get error() {
      const s = this.#s;
      if (!s.done) throw fail('InvalidStateError', "Failed to read the 'error' property from 'IDBRequest': The request has not finished.");
      return s.error;
    }
    get source() {
      return this.#s.source;
    }
    get transaction() {
      return this.#s.tx ? this.#s.tx.obj : null;
    }
    get readyState() {
      return this.#s.done ? 'done' : 'pending';
    }
  }
  tag(IDBRequest, 'IDBRequest', 'result', 'error', 'source', 'transaction', 'readyState');
  handlers(IDBRequest, 'request', 'success', 'error');
  class IDBOpenDBRequest extends IDBRequest {
    constructor(t = undefined) {
      super(t, true);
    }
  }
  tag(IDBOpenDBRequest, 'IDBOpenDBRequest');
  handlers(IDBOpenDBRequest, 'open', 'blocked', 'upgradeneeded');

  const succeed = (s, result) => {
    s.done = true;
    s.result = result;
    s.error = null;
  };
  const failed = (s, error) => {
    s.done = true;
    s.result = undefined;
    s.error = error;
  };

  // ---- Connections ----
  const conns = [];
  function loadMeta(name) {
    const meta = new Map();
    for (const [store, kp, ai, indexes] of N.schema(name)) {
      const m = { name: store, backingName: store, backingIndexes: new Set(), keyPath: parse(kp), ai, indexes: new Map(), deleted: false };
      for (const [index, ikp, unique, multiEntry] of indexes) {
        m.indexes.set(index, { name: index, backingName: index, keyPath: parse(ikp), unique, multiEntry, deleted: false });
      }
      for (const im of m.indexes.values()) m.backingIndexes.add(im);
      meta.set(store, m);
    }
    return meta;
  }
  function connect(name, version) {
    const c = {
      name,
      id: N.connect(name),
      version,
      meta: loadMeta(name),
      closePending: false,
      closed: false,
      txs: new Set(),
      upgrade: null,
      db: null,
    };
    c.db = new IDBDatabase(token, c);
    conns.push(c);
    return c;
  }
  function maybeClose(c) {
    if (!c.closePending || c.closed || c.txs.size) return;
    c.closed = true;
    const i = conns.indexOf(c);
    if (i >= 0) conns.splice(i, 1);
    N.disconnect(c.name, c.id);
    wakeLocal(c.name);
  }
  const othersOpen = (name, self) => conns.some((o) => o.name === name && o !== self) || N.others(name) > 0;

  let connection;
  class IDBDatabase extends ET {
    #c;
    constructor(t = undefined, c) {
      if (t !== token) throw illegal();
      super();
      initHandlerState(this, { __proto__: null, database: true });
      this.#c = c;
    }
    static {
      connection = (d) => d.#c;
    }
    get name() {
      return this.#c.name;
    }
    get version() {
      return this.#c.version;
    }
    get objectStoreNames() {
      return sortedList(this.#c.meta.keys());
    }
    createObjectStore(name, options = undefined) {
      if (!arguments.length) throw new TE("Failed to execute 'createObjectStore' on 'IDBDatabase': 1 argument required.");
      name = domString(name);
      const o = dictionary(options);
      const ai = !!o.autoIncrement;
      const keyPath = toKeyPath(o.keyPath);
      const c = this.#c, x = c.upgrade;
      if (!x) throw fail('InvalidStateError', 'The database is not running a version change transaction.');
      if (x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
      if (keyPath !== null && !validPath(keyPath)) throw fail('SyntaxError', 'The keyPath option is not a valid key path.');
      if (c.meta.has(name)) throw fail('ConstraintError', 'An object store with the specified name already exists.');
      if (ai && (keyPath === '' || isArray(keyPath))) {
        throw fail('InvalidAccessError', 'The autoIncrement option was set but the keyPath option was empty or an array.');
      }
      enqueueSchema(x, () => N.createStore(c.name, name, stringify(keyPath), ai));
      const m = { name, backingName: name, backingIndexes: new Set(), keyPath, ai, indexes: new Map(), deleted: false };
      c.meta.set(name, m);
      x.created.push(m);
      return storeHandle(x, m);
    }
    deleteObjectStore(name) {
      if (!arguments.length) throw new TE("Failed to execute 'deleteObjectStore' on 'IDBDatabase': 1 argument required.");
      name = domString(name);
      const c = this.#c, x = c.upgrade;
      if (!x) throw fail('InvalidStateError', 'The database is not running a version change transaction.');
      if (x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
      const m = c.meta.get(name);
      if (!m) throw fail('NotFoundError', 'The specified object store was not found.');
      enqueueSchema(x, () => N.deleteStore(c.name, name));
      c.meta.delete(name);
      markStoreDeleted(m);
    }
    transaction(storeNames, mode = undefined, options = undefined) {
      if (!arguments.length) throw new TE("Failed to execute 'transaction' on 'IDBDatabase': 1 argument required.");
      const names = iterableObject(storeNames) ? toStrings(storeNames) : [domString(storeNames)];
      mode = mode === undefined ? 'readonly' : domString(mode);
      if (mode !== 'readonly' && mode !== 'readwrite' && mode !== 'versionchange') {
        throw new TE(`The provided value '${mode}' is not a valid enum value of type IDBTransactionMode.`);
      }
      const o = dictionary(options);
      const durability = o.durability === undefined ? 'default' : domString(o.durability);
      if (durability !== 'default' && durability !== 'strict' && durability !== 'relaxed') {
        throw new TE(`The provided value '${durability}' is not a valid enum value of type IDBTransactionDurability.`);
      }
      const c = this.#c;
      if (c.upgrade) throw fail('InvalidStateError', 'A version change transaction is running.');
      if (c.closePending) throw fail('InvalidStateError', 'The database connection is closing.');
      const scope = [...new Set(names)].sort();
      for (const n of scope) {
        if (!c.meta.has(n)) throw fail('NotFoundError', 'One of the specified object stores was not found.');
      }
      if (!scope.length) throw fail('InvalidAccessError', 'The storeNames parameter was empty.');
      if (mode === 'versionchange') {
        throw new TE("The mode provided ('versionchange') is not one of 'readonly' or 'readwrite'.");
      }
      return newTransaction(c, scope, mode, durability, false).obj;
    }
    close() {
      const c = this.#c;
      if (!c.closePending) {
        c.closePending = true;
        maybeClose(c);
      }
    }
  }
  tag(IDBDatabase, 'IDBDatabase', 'name', 'version', 'objectStoreNames', 'createObjectStore', 'deleteObjectStore',
    'transaction', 'close');
  handlers(IDBDatabase, 'database', 'abort', 'close', 'error', 'versionchange');

  // ---- Transactions ----
  const txs = [];
  let txOf;
  class IDBTransaction extends ET {
    #x;
    constructor(t = undefined, x) {
      if (t !== token) throw illegal();
      super();
      initHandlerState(this, { __proto__: null, transaction: true });
      this.#x = x;
    }
    static {
      txOf = (t) => t.#x;
    }
    get objectStoreNames() {
      const x = this.#x;
      return x.upgrade ? sortedList(x.conn.meta.keys()) : new DOMStringList(token, x.scope.slice());
    }
    get mode() {
      return this.#x.mode;
    }
    get durability() {
      return this.#x.durability;
    }
    get db() {
      return this.#x.conn.db;
    }
    get error() {
      return this.#x.error;
    }
    objectStore(name) {
      if (!arguments.length) throw new TE("Failed to execute 'objectStore' on 'IDBTransaction': 1 argument required.");
      name = domString(name);
      const x = this.#x;
      if (x.state === 'finished') throw fail('InvalidStateError', 'The transaction has finished.');
      const m = x.conn.meta.get(name);
      if (!m || (!x.upgrade && !x.scope.includes(name))) {
        throw fail('NotFoundError', 'The specified object store was not found.');
      }
      return storeHandle(x, m);
    }
    commit() {
      const x = this.#x;
      if (x.state !== 'active') throw fail('InvalidStateError', 'The transaction is not active.');
      x.state = 'committing';
      pump(x);
    }
    abort() {
      const x = this.#x;
      if (x.state === 'committing' || x.state === 'finished') {
        throw fail('InvalidStateError', 'The transaction has already been committed or aborted.');
      }
      abortTx(x, null);
    }
  }
  tag(IDBTransaction, 'IDBTransaction', 'objectStoreNames', 'mode', 'durability', 'db', 'error', 'objectStore',
    'commit', 'abort');
  handlers(IDBTransaction, 'transaction', 'abort', 'complete', 'error');

  function newTransaction(c, scope, mode, durability, upgrade) {
    const x = {
      id: N.nextId(),
      conn: c,
      scope,
      mode,
      durability,
      upgrade,
      state: 'active',
      started: false,
      inflight: false,
      queue: [],
      error: null,
      handles: new Map(),
      created: [],
      snapshot: null,
      openItem: null,
      obj: null,
    };
    x.obj = new IDBTransaction(token, x);
    N.setParent(x.obj, c.db);
    c.txs.add(x);
    txs.push(x);
    if (!upgrade) {
      // Inactive once the creating task is over.
      post(() => {
        if (x.state === 'active') x.state = 'inactive';
        pump(x);
      });
      tryStart();
    }
    return x;
  }
  const overlaps = (a, b) => a.scope.some((n) => b.scope.includes(n));
  function tryStart() {
    for (let i = 0; i < txs.length; i++) {
      const x = txs[i];
      if (x.started || x.state === 'finished') continue;
      let blocked = false;
      for (let j = 0; j < i && !blocked; j++) {
        const y = txs[j];
        blocked =
          y.conn.name === x.conn.name &&
          (y.upgrade || ((y.mode !== 'readonly' || x.mode !== 'readonly') && overlaps(x, y)));
      }
      if (blocked) continue;
      if (!N.lock(x.conn.name, x.id, x.scope, x.mode !== 'readonly')) {
        waitFor(x.conn.name, tryStart);
        continue;
      }
      x.started = true;
      pump(x);
    }
  }
  function finish(x, committed) {
    x.state = 'finished';
    const i = txs.indexOf(x);
    if (i >= 0) txs.splice(i, 1);
    x.conn.txs.delete(x);
    post(() => finishedEvent(x, committed ? 'complete' : 'abort', committed));
    tryStart();
    if (!x.upgrade) maybeClose(x.conn);
  }
  function pump(x) {
    if (!x.started || x.inflight || x.state === 'finished') return;
    if (x.queue.length) {
      x.inflight = true;
      post(() => execute(x));
    } else if (x.state === 'inactive' || x.state === 'committing') {
      commitTx(x);
    }
  }
  function enqueueSchema(x, op) {
    x.queue.push({ request: null, op });
    pump(x);
  }
  function execute(x) {
    if (x.state === 'finished') {
      x.inflight = false;
      return;
    }
    while (x.queue.length && !x.queue[0].request) {
      try {
        x.queue.shift().op();
      } catch (error) {
        x.inflight = false;
        abortTx(x, error);
        return;
      }
    }
    if (!x.queue.length) {
      x.inflight = false;
      pump(x);
      return;
    }
    const r = x.queue.shift();
    const s = requestState(r.request);
    let error = null;
    try {
      succeed(s, r.op());
    } catch (e) {
      error = e;
      failed(s, e);
    }
    if (x.state === 'inactive') x.state = 'active';
    return {
      t: r.request,
      e: error ? plainEvent('error', true, true) : plainEvent('success', false, false),
      a: (notCanceled, threw) => {
        x.inflight = false;
        if (x.state === 'active') x.state = 'inactive';
        if (x.state === 'finished') return;
        if (threw) abortTx(x, fail('AbortError', 'An exception was thrown in an event handler.'));
        else if (error && notCanceled) abortTx(x, error);
        else pump(x);
      },
    };
  }
  function commitTx(x) {
    x.state = 'committing';
    N.commit(x.conn.name, x.id);
    finish(x, true);
  }
  function abortTx(x, error) {
    if (x.state === 'finished') return;
    if (x.started) N.abort(x.conn.name, x.id);
    x.error = error;
    if (x.upgrade) revertUpgrade(x);
    for (const r of x.queue.splice(0)) {
      if (!r.request) continue;
      failed(requestState(r.request), fail('AbortError', 'The transaction was aborted, so the request cannot be fulfilled.'));
      post(() => ({ t: r.request, e: plainEvent('error', true, true) }));
    }
    finish(x, false);
  }
  // `complete` or `abort`. An upgrade no longer runs while its listeners do
  // (the open request keeps its transaction until after them).
  function finishedEvent(x, type, committed) {
    if (x.upgrade) x.conn.upgrade = null;
    return {
      t: x.obj,
      e: plainEvent(type, !committed, false),
      a: () => {
        if (x.upgrade) upgradeFinished(x, committed);
      },
    };
  }
  // "Abort an upgrade transaction": the connection's metadata as it was.
  function revertUpgrade(x) {
    const c = x.conn;
    c.version = x.snapshot.version;
    c.meta = new Map();
    for (const [m, name, indexes] of x.snapshot.stores) {
      m.name = name;
      m.deleted = false;
      m.indexes = new Map();
      for (const [im, iname] of indexes) {
        im.name = iname;
        im.deleted = false;
        m.indexes.set(iname, im);
      }
      c.meta.set(name, m);
    }
    for (const store of x.created) markStoreDeleted(store);
    for (const im of x.createdIndexes || []) im.deleted = true;
    c.closePending = true;
  }

  // ---- Object stores ----
  function markStoreDeleted(store) {
    store.deleted = true;
    for (const index of store.indexes.values()) index.deleted = true;
    store.indexes.clear();
  }
  let storeOf;
  function storeHandle(x, m) {
    let h = x.handles.get(m);
    if (!h) {
      h = new IDBObjectStore(token, { x, m, indexes: new Map(), keyPath: undefined });
      x.handles.set(m, h);
    }
    return h;
  }
  function check(h, write) {
    if (h.m.deleted) throw fail('InvalidStateError', 'The object store has been deleted.');
    if (h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
    if (write && h.x.mode === 'readonly') throw fail('ReadOnlyError', 'The transaction is read-only.');
  }
  function upgradeCheck(h, deleted) {
    if (!h.x.upgrade) throw fail('InvalidStateError', 'The database is not running a version change transaction.');
    if (deleted) throw fail('InvalidStateError', 'The object store or index has been deleted.');
    if (h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
  }
  function request(x, source, op) {
    const r = new IDBRequest(token);
    const s = requestState(r);
    s.source = source;
    s.tx = x;
    N.setParent(r, x.obj);
    x.queue.push({ request: r, op });
    pump(x);
    return r;
  }
  // The clone, with the transaction inactive while getters run.
  function cloneValue(x, value) {
    x.state = 'inactive';
    try {
      return N.clone(value);
    } finally {
      x.state = 'active';
    }
  }
  // "Store a record into an object store".
  function storeRecord(h, value, key, noOverwrite) {
    const m = h.m, x = h.x, name = x.conn.name;
    if (key === undefined) {
      const n = N.nextKey(name, m.backingName);
      if (n < 0) throw fail('ConstraintError', 'The key generator has reached its maximum value.');
      key = numberKey(n);
      if (m.keyPath !== null) inject(value, m.keyPath, n);
    }
    const idx = [];
    for (const im of m.backingIndexes) idx.push([im.backingName, indexKeys(im, value)]);
    const code = N.put(name, m.backingName, x.id, key, value, idx, noOverwrite);
    if (code === 1) throw fail('ConstraintError', 'Key already exists in the object store.');
    if (code === 2) throw fail('ConstraintError', 'Unable to add key to index: at least one key does not satisfy the uniqueness requirements.');
    const primaryKey = decodeKey(key);
    if (m.ai && key[0] === '1') N.bumpKey(name, m.backingName, x.id, primaryKey);
    return primaryKey;
  }
  function addOrPut(store, args, noOverwrite, what) {
    if (!args.length) throw new TE(`Failed to execute '${what}' on 'IDBObjectStore': 1 argument required.`);
    const h = storeOf(store);
    const value = args[0], key = args[1];
    check(h, true);
    const m = h.m, kp = m.keyPath;
    if (kp !== null && key !== undefined) {
      throw fail('DataError', 'The object store uses in-line keys and the key parameter was provided.');
    }
    if (kp === null && !m.ai && key === undefined) {
      throw fail('DataError', 'The object store uses out-of-line keys and has no key generator and the key parameter was not provided.');
    }
    let k = key === undefined ? undefined : mustKey(key);
    const clone = cloneValue(h.x, value);
    if (kp !== null) {
      const v = evalPath(clone, kp);
      if (v !== FAIL) {
        k = toKey(v);
        if (k === undefined) throw fail('DataError', 'Evaluating the object store\'s key path yielded a value that is not a valid key.');
      } else if (!m.ai) {
        throw fail('DataError', 'Evaluating the object store\'s key path did not yield a value.');
      } else if (!canInject(clone, kp)) {
        throw fail('DataError', 'A generated key could not be inserted into the value.');
      }
    }
    return request(h.x, store, () => storeRecord(h, clone, k, noOverwrite));
  }
  // getAll(queryOrOptions, count) and getAllRecords(options): the Web IDL
  // conversions, before the state checks; the query is converted after them.
  function getAllArgs(args, records) {
    const first = args[0];
    if (!records && potentiallyRange(first)) {
      return { query: first, count: unsignedLong(args[1], 'count'), dir: 0 };
    }
    const o = dictionary(first);
    const count = unsignedLong(o.count, 'count');
    const dir = direction(o.direction);
    return { query: o.query, count, dir };
  }
  // kind: 0 values, 1 primary keys, 2 records.
  function getAllRequest(source, h, index, args, records, kind) {
    const a = getAllArgs(args, records);
    if (index) indexCheck(index);
    else check(h, false);
    const range = toRange(a.query, false);
    return request(h.x, source, () => {
      const name = index ? index.im.backingName : null;
      const flat = N.getAll(h.x.conn.name, h.m.backingName, name, ...range, a.count, a.dir, kind !== 1);
      const out = [];
      for (let i = 0; i < flat.length; i += 3) {
        if (kind === 0) out.push(flat[i + 2]);
        else if (kind === 1) out.push(decodeKey(flat[i + 1]));
        else out.push(new IDBRecord(token, decodeKey(flat[i]), decodeKey(flat[i + 1]), flat[i + 2]));
      }
      return out;
    });
  }
  class IDBObjectStore {
    #h;
    constructor(t = undefined, h) {
      if (t !== token) throw illegal();
      this.#h = h;
    }
    static {
      storeOf = (s) => s.#h;
    }
    get name() {
      return this.#h.m.name;
    }
    set name(value) {
      const h = this.#h, name = domString(value), c = h.x.conn;
      upgradeCheck(h, h.m.deleted);
      if (h.m.name === name) return;
      if (c.meta.has(name)) throw fail('ConstraintError', 'An object store with the specified name already exists.');
      enqueueSchema(h.x, () => {
        N.renameStore(c.name, h.m.backingName, name);
        h.m.backingName = name;
      });
      c.meta.delete(h.m.name);
      h.m.name = name;
      c.meta.set(name, h.m);
    }
    get keyPath() {
      const h = this.#h;
      if (h.keyPath === undefined) h.keyPath = pathValue(h.m.keyPath);
      return h.keyPath;
    }
    get indexNames() {
      return sortedList(this.#h.m.indexes.keys());
    }
    get transaction() {
      return this.#h.x.obj;
    }
    get autoIncrement() {
      return this.#h.m.ai;
    }
    put(value, key = undefined) {
      return addOrPut(this, arguments, false, 'put');
    }
    add(value, key = undefined) {
      return addOrPut(this, arguments, true, 'add');
    }
    delete(query) {
      if (!arguments.length) throw new TE("Failed to execute 'delete' on 'IDBObjectStore': 1 argument required.");
      const h = this.#h;
      check(h, true);
      const range = toRange(query, true);
      return request(h.x, this, () => {
        N.deleteRange(h.x.conn.name, h.m.backingName, h.x.id, ...range);
        return undefined;
      });
    }
    clear() {
      const h = this.#h;
      check(h, true);
      return request(h.x, this, () => {
        N.clear(h.x.conn.name, h.m.backingName, h.x.id);
        return undefined;
      });
    }
    get(query) {
      if (!arguments.length) throw new TE("Failed to execute 'get' on 'IDBObjectStore': 1 argument required.");
      const h = this.#h;
      check(h, false);
      const range = toRange(query, true);
      return request(h.x, this, () => {
        const r = N.getRecord(h.x.conn.name, h.m.backingName, null, ...range, true);
        return r ? r[2] : undefined;
      });
    }
    getKey(query) {
      if (!arguments.length) throw new TE("Failed to execute 'getKey' on 'IDBObjectStore': 1 argument required.");
      const h = this.#h;
      check(h, false);
      const range = toRange(query, true);
      return request(h.x, this, () => {
        const r = N.getRecord(h.x.conn.name, h.m.backingName, null, ...range, false);
        return r ? decodeKey(r[1]) : undefined;
      });
    }
    getAll(query = undefined, count = undefined) {
      return getAllRequest(this, this.#h, null, arguments, false, 0);
    }
    getAllKeys(query = undefined, count = undefined) {
      return getAllRequest(this, this.#h, null, arguments, false, 1);
    }
    getAllRecords(options = undefined) {
      return getAllRequest(this, this.#h, null, [options], true, 2);
    }
    count(query = undefined) {
      const h = this.#h;
      check(h, false);
      const range = toRange(query, false);
      return request(h.x, this, () => N.count(h.x.conn.name, h.m.backingName, null, ...range));
    }
    openCursor(query = undefined, dir = undefined) {
      return openCursor(this, this.#h, null, query, dir, false);
    }
    openKeyCursor(query = undefined, dir = undefined) {
      return openCursor(this, this.#h, null, query, dir, true);
    }
    index(name) {
      if (!arguments.length) throw new TE("Failed to execute 'index' on 'IDBObjectStore': 1 argument required.");
      name = domString(name);
      const h = this.#h;
      if (h.m.deleted) throw fail('InvalidStateError', 'The object store has been deleted.');
      if (h.x.state === 'finished') throw fail('InvalidStateError', 'The transaction has finished.');
      const im = h.m.indexes.get(name);
      if (!im) throw fail('NotFoundError', 'The specified index was not found.');
      return indexHandle(this, h, im);
    }
    createIndex(name, keyPath, options = undefined) {
      if (arguments.length < 2) throw new TE("Failed to execute 'createIndex' on 'IDBObjectStore': 2 arguments required.");
      name = domString(name);
      keyPath = iterableObject(keyPath) ? toStrings(keyPath) : domString(keyPath);
      const o = dictionary(options);
      const multiEntry = !!o.multiEntry, unique = !!o.unique;
      const h = this.#h, x = h.x, c = x.conn;
      upgradeCheck(h, h.m.deleted);
      if (h.m.indexes.has(name)) throw fail('ConstraintError', 'An index with the specified name already exists.');
      if (!validPath(keyPath)) throw fail('SyntaxError', 'The keyPath argument contains an invalid key path.');
      if (isArray(keyPath) && multiEntry) {
        throw fail('InvalidAccessError', 'The keyPath argument was an array and the multiEntry option is true.');
      }
      const im = { name, backingName: name, keyPath, unique, multiEntry, deleted: false };
      h.m.indexes.set(name, im);
      (x.createdIndexes || (x.createdIndexes = [])).push(im);
      enqueueSchema(x, () => {
        N.createIndex(c.name, h.m.backingName, name, stringify(keyPath), unique, multiEntry);
        const rows = N.records(c.name, h.m.backingName);
        for (let i = 0; i < rows.length; i += 2) {
          if (!N.indexAdd(c.name, h.m.backingName, name, rows[i], indexKeys(im, rows[i + 1]))) {
            throw fail('ConstraintError', 'Unable to add key to index: at least one key does not satisfy the uniqueness requirements.');
          }
        }
        h.m.backingIndexes.add(im);
      });
      return indexHandle(this, h, im);
    }
    deleteIndex(name) {
      if (!arguments.length) throw new TE("Failed to execute 'deleteIndex' on 'IDBObjectStore': 1 argument required.");
      name = domString(name);
      const h = this.#h;
      upgradeCheck(h, h.m.deleted);
      const im = h.m.indexes.get(name);
      if (!im) throw fail('NotFoundError', 'The specified index was not found.');
      enqueueSchema(h.x, () => {
        N.deleteIndex(h.x.conn.name, h.m.backingName, name);
        h.m.backingIndexes.delete(im);
      });
      h.m.indexes.delete(name);
      im.deleted = true;
    }
  }
  tag(IDBObjectStore, 'IDBObjectStore', 'name', 'keyPath', 'indexNames', 'transaction', 'autoIncrement', 'put',
    'add', 'delete', 'clear', 'get', 'getKey', 'getAll', 'getAllKeys', 'getAllRecords', 'count', 'openCursor',
    'openKeyCursor', 'index', 'createIndex', 'deleteIndex');

  // ---- Indexes ----
  let indexOf;
  function indexHandle(store, h, im) {
    let i = h.indexes.get(im);
    if (!i) {
      i = new IDBIndex(token, { store, h, im, keyPath: undefined });
      h.indexes.set(im, i);
    }
    return i;
  }
  function indexCheck(d) {
    if (d.im.deleted || d.h.m.deleted) throw fail('InvalidStateError', 'The index or its object store has been deleted.');
    if (d.h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
  }
  class IDBIndex {
    #d;
    constructor(t = undefined, d) {
      if (t !== token) throw illegal();
      this.#d = d;
    }
    static {
      indexOf = (i) => i.#d;
    }
    get name() {
      return this.#d.im.name;
    }
    set name(value) {
      const d = this.#d, name = domString(value), h = d.h;
      upgradeCheck(h, d.im.deleted || h.m.deleted);
      if (d.im.name === name) return;
      if (h.m.indexes.has(name)) throw fail('ConstraintError', 'An index with the specified name already exists.');
      enqueueSchema(h.x, () => {
        N.renameIndex(h.x.conn.name, h.m.backingName, d.im.backingName, name);
        d.im.backingName = name;
      });
      h.m.indexes.delete(d.im.name);
      d.im.name = name;
      h.m.indexes.set(name, d.im);
    }
    get objectStore() {
      return this.#d.store;
    }
    get keyPath() {
      const d = this.#d;
      if (d.keyPath === undefined) d.keyPath = pathValue(d.im.keyPath);
      return d.keyPath;
    }
    get multiEntry() {
      return this.#d.im.multiEntry;
    }
    get unique() {
      return this.#d.im.unique;
    }
    get(query) {
      if (!arguments.length) throw new TE("Failed to execute 'get' on 'IDBIndex': 1 argument required.");
      const d = this.#d;
      indexCheck(d);
      const range = toRange(query, true);
      return request(d.h.x, this, () => {
        const r = N.getRecord(d.h.x.conn.name, d.h.m.backingName, d.im.backingName, ...range, true);
        return r ? r[2] : undefined;
      });
    }
    getKey(query) {
      if (!arguments.length) throw new TE("Failed to execute 'getKey' on 'IDBIndex': 1 argument required.");
      const d = this.#d;
      indexCheck(d);
      const range = toRange(query, true);
      return request(d.h.x, this, () => {
        const r = N.getRecord(d.h.x.conn.name, d.h.m.backingName, d.im.backingName, ...range, false);
        return r ? decodeKey(r[1]) : undefined;
      });
    }
    getAll(query = undefined, count = undefined) {
      return getAllRequest(this, this.#d.h, this.#d, arguments, false, 0);
    }
    getAllKeys(query = undefined, count = undefined) {
      return getAllRequest(this, this.#d.h, this.#d, arguments, false, 1);
    }
    getAllRecords(options = undefined) {
      return getAllRequest(this, this.#d.h, this.#d, [options], true, 2);
    }
    count(query = undefined) {
      const d = this.#d;
      indexCheck(d);
      const range = toRange(query, false);
      return request(d.h.x, this, () => N.count(d.h.x.conn.name, d.h.m.backingName, d.im.backingName, ...range));
    }
    openCursor(query = undefined, dir = undefined) {
      return openCursor(this, this.#d.h, this.#d, query, dir, false);
    }
    openKeyCursor(query = undefined, dir = undefined) {
      return openCursor(this, this.#d.h, this.#d, query, dir, true);
    }
  }
  tag(IDBIndex, 'IDBIndex', 'name', 'objectStore', 'keyPath', 'multiEntry', 'unique', 'get', 'getKey', 'getAll',
    'getAllKeys', 'getAllRecords', 'count', 'openCursor', 'openKeyCursor');

  // ---- Cursors ----
  function openCursor(source, h, index, query, dir, keyOnly) {
    const d = direction(dir);
    if (index) indexCheck(index);
    else check(h, false);
    const range = toRange(query, false);
    const c = {
      source,
      h,
      im: index ? index.im : null,
      dir: d,
      range,
      key: undefined,
      pk: undefined,
      keyValue: undefined,
      pkValue: undefined,
      value: undefined,
      gotValue: false,
      keyOnly,
      request: null,
      obj: null,
    };
    c.obj = keyOnly ? new IDBCursor(token, c) : new IDBCursorWithValue(token, c);
    c.request = request(h.x, source, () => iterate(c, undefined, undefined, 1));
    return c.request;
  }
  // "Iterate a cursor".
  function iterate(c, key, primaryKey, count) {
    const r = N.seek(
      c.h.x.conn.name,
      c.h.m.backingName,
      c.im ? c.im.backingName : null,
      ...c.range,
      c.dir,
      c.key ?? null,
      c.im ? c.pk ?? null : null,
      key ?? null,
      primaryKey ?? null,
      count,
      !c.keyOnly
    );
    if (!r) {
      c.key = c.pk = c.keyValue = c.pkValue = c.value = undefined;
      return null;
    }
    c.key = r[0];
    c.pk = r[1];
    c.keyValue = decodeKey(r[0]);
    c.pkValue = c.im ? decodeKey(r[1]) : c.keyValue;
    c.value = r[2];
    c.gotValue = true;
    return c.obj;
  }
  const sourceDeleted = (c) => c.h.m.deleted || (c.im !== null && c.im.deleted);
  function again(c, op) {
    const x = c.h.x;
    const s = requestState(c.request);
    s.done = false;
    s.result = undefined;
    s.error = null;
    x.queue.push({ request: c.request, op });
    pump(x);
  }
  function cursorWrite(c) {
    const x = c.h.x;
    if (x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
    if (x.mode === 'readonly') throw fail('ReadOnlyError', 'The record may not be modified inside a read-only transaction.');
    if (sourceDeleted(c)) throw fail('InvalidStateError', 'The cursor\'s source or effective object store has been deleted.');
    if (!c.gotValue || c.keyOnly) throw fail('InvalidStateError', 'The cursor is being iterated or has iterated past its end.');
  }
  class IDBCursor {
    #c;
    constructor(t = undefined, c) {
      if (t !== token) throw illegal();
      this.#c = c;
    }
    get source() {
      return this.#c.source;
    }
    get direction() {
      return DIRECTIONS[this.#c.dir];
    }
    get key() {
      return this.#c.keyValue;
    }
    get primaryKey() {
      return this.#c.pkValue;
    }
    get request() {
      return this.#c.request;
    }
    advance(count) {
      if (!arguments.length) throw new TE("Failed to execute 'advance' on 'IDBCursor': 1 argument required.");
      const n = enforceRange(count, 4294967295, 'count');
      if (n === 0) throw new TE("Failed to execute 'advance' on 'IDBCursor': A count argument with value 0 (zero) was supplied, must be greater than 0.");
      const c = this.#c;
      if (c.h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
      if (sourceDeleted(c)) throw fail('InvalidStateError', 'The cursor\'s source or effective object store has been deleted.');
      if (!c.gotValue) throw fail('InvalidStateError', 'The cursor is being iterated or has iterated past its end.');
      c.gotValue = false;
      again(c, () => iterate(c, undefined, undefined, n));
    }
    continue(key = undefined) {
      const c = this.#c;
      if (c.h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
      if (sourceDeleted(c)) throw fail('InvalidStateError', 'The cursor\'s source or effective object store has been deleted.');
      if (!c.gotValue) throw fail('InvalidStateError', 'The cursor is being iterated or has iterated past its end.');
      let k;
      if (key !== undefined) {
        k = mustKey(key);
        const forward = c.dir < 2;
        if (forward ? k <= c.key : k >= c.key) {
          throw fail('DataError', 'The parameter is less than or equal to this cursor\'s position.');
        }
      }
      c.gotValue = false;
      again(c, () => iterate(c, k, undefined, 1));
    }
    continuePrimaryKey(key, primaryKey) {
      if (arguments.length < 2) throw new TE("Failed to execute 'continuePrimaryKey' on 'IDBCursor': 2 arguments required.");
      const c = this.#c;
      if (c.h.x.state !== 'active') throw fail('TransactionInactiveError', 'The transaction is not active.');
      if (sourceDeleted(c)) throw fail('InvalidStateError', 'The cursor\'s source or effective object store has been deleted.');
      if (c.im === null) throw fail('InvalidAccessError', 'The cursor\'s source is not an index.');
      if (c.dir !== 0 && c.dir !== 2) throw fail('InvalidAccessError', 'The cursor\'s direction is not \'next\' or \'prev\'.');
      if (!c.gotValue) throw fail('InvalidStateError', 'The cursor is being iterated or has iterated past its end.');
      const k = mustKey(key), p = mustKey(primaryKey, 'primary key');
      if (c.dir === 0 ? k < c.key || (k === c.key && p <= c.pk) : k > c.key || (k === c.key && p >= c.pk)) {
        throw fail('DataError', 'The key is not past this cursor\'s position.');
      }
      c.gotValue = false;
      again(c, () => iterate(c, k, p, 1));
    }
    update(value) {
      if (!arguments.length) throw new TE("Failed to execute 'update' on 'IDBCursor': 1 argument required.");
      const c = this.#c;
      cursorWrite(c);
      const clone = cloneValue(c.h.x, value);
      const kp = c.h.m.keyPath;
      if (kp !== null) {
        const v = evalPath(clone, kp);
        if (v === FAIL || toKey(v) !== c.pk) {
          throw fail('DataError', 'The effective object store of this cursor uses in-line keys and evaluating the key path of the value parameter results in a different value than the cursor\'s effective key.');
        }
      }
      const pk = c.pk;
      return request(c.h.x, this, () => storeRecord(c.h, clone, pk, false));
    }
    delete() {
      const c = this.#c;
      cursorWrite(c);
      const pk = c.pk;
      return request(c.h.x, this, () => {
        N.deleteRange(c.h.x.conn.name, c.h.m.backingName, c.h.x.id, pk, pk, false, false);
        return undefined;
      });
    }
  }
  tag(IDBCursor, 'IDBCursor', 'source', 'direction', 'key', 'primaryKey', 'request', 'advance', 'continue',
    'continuePrimaryKey', 'update', 'delete');
  class IDBCursorWithValue extends IDBCursor {
    #v;
    constructor(t = undefined, c) {
      super(t, c);
      this.#v = c;
    }
    get value() {
      return this.#v.value;
    }
  }
  tag(IDBCursorWithValue, 'IDBCursorWithValue', 'value');

  // ---- Opening and deleting databases ----
  const queues = new Map();
  function enqueue(item) {
    let q = queues.get(item.name);
    if (!q) queues.set(item.name, (q = []));
    q.push(item);
    if (q.length === 1) post(() => run(item));
  }
  function next(name) {
    const q = queues.get(name);
    q.shift();
    if (q.length) post(() => run(q[0]));
    else queues.delete(name);
  }
  const run = (item) => (item.kind === 'open' ? runOpen(item) : runDelete(item));
  const retry = (item) => waitFor(item.name, () => post(() => run(item)));
  function settle(item, event) {
    return {
      t: item.request,
      e: event,
      a: () => next(item.name),
    };
  }
  function runOpen(item) {
    const name = item.name;
    if (N.busy(name)) return retry(item);
    const current = N.version(name);
    const version = item.version === undefined ? (current > 0 ? current : 1) : item.version;
    const s = requestState(item.request);
    if (current > version) {
      failed(s, fail('VersionError', `The requested version (${version}) is less than the existing version (${current}).`));
      return settle(item, plainEvent('error', true, true));
    }
    if (current === version) {
      const c = connect(name, current);
      succeed(s, c.db);
      return settle(item, plainEvent('success', false, false));
    }
    if (!N.reserve(name)) return retry(item);
    const old = current < 0 ? 0 : current;
    item.conn = connect(name, old);
    item.old = old;
    item.version = version;
    versionChange(item, old, version);
  }
  // Fires `versionchange` at the other connections, then `blocked` at the
  // request if some stay open, then continues once all are closed.
  function versionChange(item, old, version) {
    for (const o of conns) {
      if (o.name === item.name && o !== item.conn && !o.closePending) {
        post(() => (o.closePending ? undefined : { t: o.db, e: versionEvent('versionchange', old, version) }));
      }
    }
    N.notify(item.name, old, version);
    post(() => {
      if (!othersOpen(item.name, item.conn)) return proceed(item);
      waitClosed(item);
      return { t: item.request, e: versionEvent('blocked', old, version) };
    });
  }
  function waitClosed(item) {
    waitFor(item.name, () => {
      if (othersOpen(item.name, item.conn)) waitClosed(item);
      else post(() => proceed(item));
    });
  }
  const proceed = (item) => (item.kind === 'open' ? upgrade(item) : deleteNow(item));
  // "Run an upgrade transaction".
  function upgrade(item) {
    const c = item.conn, name = item.name;
    const x = newTransaction(c, [], 'versionchange', 'default', true);
    x.openItem = item;
    x.snapshot = {
      version: item.old,
      stores: [...c.meta.values()].map((m) => [m, m.name, [...m.indexes.values()].map((im) => [im, im.name])]),
    };
    c.upgrade = x;
    N.beginUpgrade(name, x.id, item.version);
    c.version = item.version;
    x.started = true;
    const s = requestState(item.request);
    succeed(s, c.db);
    s.tx = x;
    x.state = 'active';
    return {
      t: item.request,
      e: versionEvent('upgradeneeded', item.old, item.version),
      a: (notCanceled, threw) => {
        if (x.state === 'active') x.state = 'inactive';
        if (x.state === 'finished') return;
        if (threw) abortTx(x, fail('AbortError', 'An exception was thrown in the upgradeneeded handler.'));
        else pump(x);
      },
    };
  }
  // After the upgrade transaction's complete or abort event.
  function upgradeFinished(x, committed) {
    const item = x.openItem, c = x.conn, s = requestState(item.request);
    s.tx = null;
    maybeClose(c);
    post(() => {
      N.release(item.name);
      if (committed && !c.closePending) {
        succeed(s, c.db);
        return settle(item, plainEvent('success', false, false));
      }
      failed(s, fail('AbortError', committed ? 'The connection was closed.' : 'The version change transaction was aborted.'));
      return settle(item, plainEvent('error', true, true));
    });
  }
  function runDelete(item) {
    const name = item.name;
    if (N.busy(name)) return retry(item);
    const current = N.version(name);
    const s = requestState(item.request);
    if (current < 0) {
      succeed(s, undefined);
      return settle(item, versionEvent('success', 0, null));
    }
    if (!N.reserve(name)) return retry(item);
    item.old = current;
    versionChange(item, current, null);
  }
  function deleteNow(item) {
    N.deleteDb(item.name);
    N.release(item.name);
    succeed(requestState(item.request), undefined);
    return settle(item, versionEvent('success', item.old, null));
  }
  function databaseName(name) {
    return domString(name);
  }
  function open(name, version, argc) {
    name = databaseName(name);
    let v;
    if (argc > 1 && version !== undefined) {
      v = enforceRange(version, 9007199254740991, 'The version');
      if (v === 0) throw new TE("Failed to execute 'open' on 'IDBFactory': The version provided must not be 0.");
    }
    const r = new IDBOpenDBRequest(token);
    enqueue({ kind: 'open', name, version: v, request: r });
    return r;
  }
  function deleteDatabase(name) {
    const r = new IDBOpenDBRequest(token);
    enqueue({ kind: 'delete', name: databaseName(name), request: r });
    return r;
  }
  function cmp(a, b) {
    const x = mustKey(a, 'first argument'), y = mustKey(b, 'second argument');
    return x < y ? -1 : x > y ? 1 : 0;
  }
  // Native messages: 0 re-check waiters of `name`; 1 versionchange at `conn`.
  function wake(kind, name, conn, oldVersion, newVersion) {
    if (kind === 0) return runWaiters(name);
    const c = conns.find((o) => o.id === conn);
    if (c && !c.closePending) post(() => (c.closePending ? undefined : { t: c.db, e: versionEvent('versionchange', oldVersion, newVersion) }));
  }

  return {
    step,
    wake,
    open,
    deleteDatabase,
    cmp,
    globals: {
      IDBRequest,
      IDBOpenDBRequest,
      IDBDatabase,
      IDBTransaction,
      IDBObjectStore,
      IDBIndex,
      IDBCursor,
      IDBCursorWithValue,
      IDBKeyRange,
      IDBVersionChangeEvent,
      IDBRecord,
      DOMStringList,
    },
  };
});
