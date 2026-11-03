// Internal per-realm helpers. Evaluated once per realm before any page script;
// the returned object is held natively and is not reachable from page globals.
// Captures intrinsics up front so later page monkey-patching cannot affect them.
(function () {
  'use strict';
  const O = Object;
  const defineProperty = O.defineProperty;
  const getPrototypeOf = O.getPrototypeOf;
  const objectCreate = O.create;
  const setPrototypeOf = O.setPrototypeOf;
  const getOwnPropertyNames = O.getOwnPropertyNames;
  const getOwnPropertySymbols = O.getOwnPropertySymbols;
  const getOwnPropertyDescriptor = O.getOwnPropertyDescriptor;
  const ArrayProto = Array.prototype;
  // Intrinsics are invoked only through the captured Reflect.apply: a page
  // replacing Function.prototype.call/apply/bind cannot intercept them.
  const apply = Reflect.apply;
  const reflectGet = Reflect.get;
  const reflectSet = Reflect.set;
  const reflectHas = Reflect.has;
  const reflectOwnKeys = Reflect.ownKeys;
  const reflectGOPD = Reflect.getOwnPropertyDescriptor;
  const reflectDefine = Reflect.defineProperty;
  const reflectDelete = Reflect.deleteProperty;
  const ProxyC = Proxy;
  const reflectPreventExtensions = Reflect.preventExtensions;
  // Interface state for member installation after disposal (see realm.rs
  // populate_disposed); `populated` is kept in sync by the native side.
  // Held here, so it lives exactly as long as some interface proxy (whose
  // closure references this object) is reachable.
  const classProtos = [];
  const classTargets = [];
  const populated = [];
  const P = Promise;
  const promiseThen = P.prototype.then;
  const promiseResolveFn = P.resolve;
  const promiseResolve = (v) => apply(promiseResolveFn, P, [v]);
  // Native microtask scheduling must not consult page-controlled Promise
  // constructors/species. A polyfill may itself use queueMicrotask there,
  // causing unbounded recursive scheduling through the host bridge.
  // This private settled seed selects the intrinsic default constructor and
  // never escapes the helper closure; ordinary value assimilation is unchanged.
  const microtaskSeed = promiseResolve(undefined);
  defineProperty(microtaskSeed, 'constructor', { value: undefined });
  const hasInstance = Function.prototype[Symbol.hasInstance];
  const WM = WeakMap;
  const wmGet = WM.prototype.get;
  const wmSet = WM.prototype.set;
  const wmHas = WM.prototype.has;
  const setAdd = Set.prototype.add;
  const setHas = Set.prototype.has;
  const arraySlice = Array.prototype.slice;
  const stringSlice = String.prototype.slice;
  const stringToLowerCase = String.prototype.toLowerCase;
  const arrayPush = Array.prototype.push;
  const arrayConcat = Array.prototype.concat;
  const isArray = Array.isArray;
  const toStringTag = Symbol.toStringTag;
  const objToString = O.prototype.toString;
  const fnToString = Function.prototype.toString;
  const ErrorCtor = Error;
  const RegExpC = RegExp, DateC = Date, MapC = Map, SetC = Set, WeakMapC = WeakMap, WeakSetC = WeakSet;
  const ArrayBufferC = ArrayBuffer, DataViewC = DataView;
  const isView = ArrayBuffer.isView;
  // Remote-object previews and internal properties (CDP Runtime).
  const jsonStringify = JSON.stringify;
  const objectIs = O.is;
  const symbolToString = Symbol.prototype.toString;
  const mapEntries = MapC.prototype.entries, setValues = SetC.prototype.values;
  const mapSize = getOwnPropertyDescriptor(MapC.prototype, 'size').get;
  const setSize = getOwnPropertyDescriptor(SetC.prototype, 'size').get;
  const mapIterNext = getPrototypeOf(new MapC().entries()).next;
  const setIterNext = getPrototypeOf(new SetC().values()).next;
  const PRIMITIVE_VALUE_OF = { __proto__: null,
    '[object Number]': Number.prototype.valueOf, '[object String]': String.prototype.valueOf,
    '[object Boolean]': Boolean.prototype.valueOf, '[object Symbol]': Symbol.prototype.valueOf,
    '[object BigInt]': BigInt.prototype.valueOf,
  };
  const StringC = String;

  const DOM_EXCEPTION_CODES = { __proto__: null,
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
    NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
    InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20,
    URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25,
  };
  const DOM_EXCEPTION_CONSTANTS = [
    'INDEX_SIZE_ERR', 'DOMSTRING_SIZE_ERR', 'HIERARCHY_REQUEST_ERR', 'WRONG_DOCUMENT_ERR', 'INVALID_CHARACTER_ERR',
    'NO_DATA_ALLOWED_ERR', 'NO_MODIFICATION_ALLOWED_ERR', 'NOT_FOUND_ERR', 'NOT_SUPPORTED_ERR', 'INUSE_ATTRIBUTE_ERR',
    'INVALID_STATE_ERR', 'SYNTAX_ERR', 'INVALID_MODIFICATION_ERR', 'NAMESPACE_ERR', 'INVALID_ACCESS_ERR',
    'VALIDATION_ERR', 'TYPE_MISMATCH_ERR', 'SECURITY_ERR', 'NETWORK_ERR', 'ABORT_ERR', 'URL_MISMATCH_ERR',
    'QUOTA_EXCEEDED_ERR', 'TIMEOUT_ERR', 'INVALID_NODE_TYPE_ERR', 'DATA_CLONE_ERR',
  ];

  // DOMException as a JS class (prototype chains to Error.prototype per WebIDL).
  const names = new WM();
  class DOMException extends ErrorCtor {
    constructor(message = '', name = 'Error') {
      super(String(message));
      apply(wmSet, names, [this, String(name)]);
    }
    get name() {
      if (!apply(wmHas, names, [this])) throw new TypeError('Illegal invocation');
      return apply(wmGet, names, [this]);
    }
    get code() {
      if (!apply(wmHas, names, [this])) throw new TypeError('Illegal invocation');
      return DOM_EXCEPTION_CODES[apply(wmGet, names, [this])] || 0;
    }
  }
  defineProperty(DOMException.prototype, toStringTag, { value: 'DOMException', configurable: true });
  DOM_EXCEPTION_CONSTANTS.forEach((n, i) => {
    defineProperty(DOMException, n, { value: i + 1, enumerable: true });
    defineProperty(DOMException.prototype, n, { value: i + 1, enumerable: true });
  });
  defineProperty(globalThis, 'DOMException', { value: DOMException, writable: true, configurable: true });

  function describe(v) {
    const type = v === null ? 'object' : typeof v;
    let subtype, className, description;
    if (type === 'object' && v !== null) {
      if (isArray(v)) subtype = 'array';
      else if (v instanceof RegExpC) subtype = 'regexp';
      else if (v instanceof DateC) subtype = 'date';
      else if (v instanceof MapC) subtype = 'map';
      else if (v instanceof SetC) subtype = 'set';
      else if (v instanceof WeakMapC) subtype = 'weakmap';
      else if (v instanceof WeakSetC) subtype = 'weakset';
      else if (v instanceof ErrorCtor) subtype = 'error';
      else if (v instanceof P) subtype = 'promise';
      else if (isView(v)) subtype = v instanceof DataViewC ? 'dataview' : 'typedarray';
      else if (v instanceof ArrayBufferC) subtype = 'arraybuffer';
      else if (typeof v.nodeType === 'number' && typeof v.nodeName === 'string') subtype = 'node';
      try {
        const ctor = v.constructor;
        className = (ctor && typeof ctor.name === 'string' && ctor.name) || undefined;
      } catch (_) {}
      if (!className) {
        const tag = apply(objToString, v, []);
        className = apply(stringSlice, tag, [8, -1]);
      }
      if (subtype === 'array') description = `${className}(${v.length})`;
      else if (subtype === 'error') { try { description = StringC(v.stack || v); } catch (_) { description = className; } }
      else if (subtype === 'node') description = apply(stringToLowerCase, StringC(v.nodeName), []);
      else if (subtype === 'regexp' || subtype === 'date') description = StringC(v);
      else if (subtype === 'map' || subtype === 'set') {
        // Chrome's `Map(2)`; a subclass instance without the internal slot keeps its name.
        try { description = `${className}(${apply(subtype === 'map' ? mapSize : setSize, v, [])})`; }
        catch (_) { description = className; }
      } else if (subtype === 'typedarray' && !(v instanceof DataViewC)) description = `${className}(${v.length})`;
      else description = className;
    } else if (type === 'function') {
      className = 'Function';
      try { description = apply(fnToString, v, []); } catch (_) { description = 'function'; }
    } else if (type === 'symbol') {
      try { description = apply(symbolToString, v, []); } catch (_) {}
    }
    return { type, subtype, className, description };
  }

  const H = {
    classProtos,
    classTargets,
    populated,
    DOMException,
    makeInterface(name, native, proto, parentCtor, length, global) {
      const C = native
        ? ({ [name]: function (...args) {
            if (new.target === undefined) {
              throw new TypeError(`Failed to construct '${name}': Please use the 'new' operator, this DOM object constructor cannot be called as a function.`);
            }
            return native(new.target, args);
          } })[name]
        : ({ [name]: function () { throw new TypeError('Illegal constructor'); } })[name];
      // Both constructor forms above start with length 0.
      if (length !== 0) defineProperty(C, 'length', { value: length });
      defineProperty(C, 'prototype', { value: proto, writable: false, enumerable: false, configurable: false });
      defineProperty(proto, 'constructor', { value: C, writable: true, enumerable: false, configurable: true });
      defineProperty(proto, toStringTag, { value: name, writable: false, enumerable: false, configurable: true });
      if (parentCtor) setPrototypeOf(C, parentCtor);
      if (global) defineProperty(global, name, { value: C, writable: true, enumerable: false, configurable: true });
      return C;
    },
    // Every interface of a new realm in one call (realm.rs install_classes):
    // `spec[i]` is [name, parent index or -1, constructor length, exposed].
    // Returns [protos, targets, ctors], indexed like `spec`.
    installInterfaces(spec, natives, objectProto, global, populate) {
      const protos = [], targets = [], ctors = [];
      for (let i = 0; i < spec.length; i++) {
        const [name, parent, length, exposed] = spec[i];
        const proto = objectCreate(parent < 0 ? objectProto : protos[parent]);
        const C = H.makeInterface(name, natives[i], proto, parent < 0 ? null : ctors[parent], length, null);
        protos[i] = proto;
        targets[i] = C;
        ctors[i] = H.lazyInterface(C, proto, exposed ? global : null, name, i, populate);
      }
      return [protos, targets, ctors];
    },
    // Interface object as seen by script: a Proxy over the constructor whose
    // property traps first install the class members natively (once), then
    // forward unchanged. Construction/call/[[GetPrototypeOf]] need no members.
    lazyInterface(C, proto, global, name, id, populate) {
      let done = false;
      classProtos[id] = proto;
      classTargets[id] = C;
      const ensure = () => { if (!done) { populate(id, H); done = true; } };
      const P = new ProxyC(C, {
        get(t, k, r) { ensure(); return reflectGet(t, k, r); },
        set(t, k, v, r) { ensure(); return reflectSet(t, k, v, r); },
        has(t, k) { ensure(); return reflectHas(t, k); },
        ownKeys(t) { ensure(); return reflectOwnKeys(t); },
        getOwnPropertyDescriptor(t, k) { ensure(); return reflectGOPD(t, k); },
        defineProperty(t, k, d) { ensure(); return reflectDefine(t, k, d); },
        deleteProperty(t, k) { ensure(); return reflectDelete(t, k); },
        // Members must exist before the target stops accepting properties.
        preventExtensions(t) { ensure(); return reflectPreventExtensions(t); },
      });
      defineProperty(proto, 'constructor', { value: P, writable: true, enumerable: false, configurable: true });
      if (global) defineProperty(global, name, { value: P, writable: true, enumerable: false, configurable: true });
      return P;
    },
    accessor(proto, name, get, set) {
      try { defineProperty(get, 'name', { value: 'get ' + name }); } catch (_) {}
      if (set) { try { defineProperty(set, 'name', { value: 'set ' + name }); } catch (_) {} }
      defineProperty(proto, name, { get, set, enumerable: true, configurable: true });
    },
    constant(target, name, value) {
      defineProperty(target, name, { value, writable: false, enumerable: true, configurable: false });
    },
    iterable(proto) {
      defineProperty(proto, Symbol.iterator, { value: ArrayProto.values, writable: true, configurable: true });
      for (const k of ['entries', 'keys', 'values', 'forEach']) {
        defineProperty(proto, k, { value: ArrayProto[k], writable: true, enumerable: true, configurable: true });
      }
    },
    setLength(fn, length) {
      try { defineProperty(fn, 'length', { value: length }); } catch (_) {}
    },
    setName(fn, name) {
      try { defineProperty(fn, 'name', { value: name }); } catch (_) {}
    },
    then(value, onFulfilled, onRejected) {
      apply(promiseThen, value === undefined ? microtaskSeed : promiseResolve(value), [onFulfilled, onRejected]);
    },
    callWith(fn, thisArg, ...args) {
      return apply(fn, thisArg, args);
    },
    // Emulates a host global class on the default JSC global, whose
    // [[Prototype]] is immutable: copy the interface members of the chain
    // (root first) onto the global and make `instanceof` accept it.
    installGlobal(global, protos, ctors, name) {
      for (let i = 0; i < protos.length; i++) {
        const proto = protos[i];
        for (const key of apply(arrayConcat, getOwnPropertyNames(proto), [getOwnPropertySymbols(proto)])) {
          if (key === 'constructor' || key === toStringTag) continue;
          defineProperty(global, key, getOwnPropertyDescriptor(proto, key));
        }
      }
      for (let i = 0; i < ctors.length; i++) {
        const C = ctors[i];
        defineProperty(C, Symbol.hasInstance, {
          value: { [Symbol.hasInstance](v) { return v === global || apply(hasInstance, this, [v]); } }[Symbol.hasInstance],
          configurable: true,
        });
      }
      defineProperty(global, toStringTag, { value: name, writable: false, enumerable: false, configurable: true });
    },
    describe,
    // `describe` as one NUL-separated string (type, subtype, className,
    // description; empty for absent): one native string read instead of
    // four property reads. Hot: every remote-object handle.
    describePacked(v) {
      const d = describe(v);
      return d.type + '\0' + (d.subtype ?? '') + '\0' + (d.className ?? '') + '\0' + (d.description ?? '');
    },
    properties(obj, ownOnly) {
      const out = [];
      const seen = new Set();
      const add = (k) => apply(setAdd, seen, [k]);
      const has = (k) => apply(setHas, seen, [k]);
      let o = obj;
      let isOwn = true;
      while (o !== null && o !== undefined) {
        const keys = apply(arrayConcat, getOwnPropertyNames(o), [getOwnPropertySymbols(o)]);
        for (let i = 0; i < keys.length; i++) {
          const key = keys[i];
          if (has(key)) continue;
          add(key);
          const d = getOwnPropertyDescriptor(o, key);
          const e = { name: typeof key === 'symbol' ? String(key) : key, isOwn,
                      writable: !!d.writable, enumerable: !!d.enumerable, configurable: !!d.configurable };
          if (typeof key === 'symbol') e.symbol = key;
          if ('value' in d) e.value = d.value;
          if (d.get) e.get = d.get;
          if (d.set) e.set = d.set;
          // Accessors on prototypes: report the current value for convenience.
          if (!isOwn && d.get) { try { e.value = reflectGet(o, key, obj); e.isAccessorValue = true; } catch (_) {} }
          apply(arrayPush, out, [e]);
        }
        if (ownOnly) break;
        o = getPrototypeOf(o);
        isOwn = false;
      }
      return out;
    },
    // CDP `Runtime.ObjectPreview` of `v` as JSON text (V8's shape: up to 5
    // properties of an object, 100 of an array, 5 entries of a Map/Set;
    // internal slots as `[[PrimitiveValue]]`/`[[PromiseState]]`).
    // `description` replaces the computed one (the caller's RemoteObject);
    // `promise` is a promise's [state, result] read by `promiseProbe`.
    preview(v, description, promise) {
      return jsonStringify(objectPreview(v, description, true, promise));
    },
    // `Runtime.getProperties` internalProperties of `obj`: [name, value]
    // pairs. A promise's state is read by a reaction, which runs once the
    // native caller's outermost call returns: `promiseProbe` attaches it
    // and returns the reader.
    internals(obj, withPrototype) {
      const out = [];
      if (withPrototype) apply(arrayPush, out, [['[[Prototype]]', getPrototypeOf(obj)]]);
      const prim = primitiveOf(obj);
      if (prim) apply(arrayPush, out, [['[[PrimitiveValue]]', prim.value]]);
      const kind = collectionKind(obj);
      if (kind) {
        const entries = [];
        const it = apply(kind === 'map' ? mapEntries : setValues, obj, []);
        for (;;) {
          const r = apply(kind === 'map' ? mapIterNext : setIterNext, it, []);
          if (r.done) break;
          apply(arrayPush, entries, [kind === 'map' ? { key: r.value[0], value: r.value[1] } : { value: r.value }]);
        }
        apply(arrayPush, out, [['[[Entries]]', entries]]);
      }
      return out;
    },
    promiseProbe(p) {
      if (!(p instanceof P)) return undefined;
      let state = 'pending', result;
      apply(promiseThen, p, [(v) => { state = 'fulfilled'; result = v; }, (e) => { state = 'rejected'; result = e; }]);
      return () => [state, result];
    },
    // The current stack (JSC format), for console stack traces.
    stack() {
      return new ErrorCtor().stack;
    },
  };

  function primitiveOf(v) {
    if (v === null || typeof v !== 'object') return undefined;
    const tag = apply(objToString, v, []);
    const f = PRIMITIVE_VALUE_OF[tag];
    if (!f) return undefined;
    try { return { value: apply(f, v, []) }; } catch (_) { return undefined; }
  }

  function collectionKind(v) {
    try { apply(mapSize, v, []); return 'map'; } catch (_) {}
    try { apply(setSize, v, []); return 'set'; } catch (_) {}
    return undefined;
  }

  function previewText(x) {
    switch (typeof x) {
      case 'string': return x.length > 100 ? apply(stringSlice, x, [0, 50]) + '…' + apply(stringSlice, x, [-49]) : x;
      case 'number': return objectIs(x, -0) ? '-0' : StringC(x);
      case 'bigint': return StringC(x) + 'n';
      case 'symbol': return apply(symbolToString, x, []);
      case 'function': return '';
      case 'undefined': return 'undefined';
      case 'boolean': return x ? 'true' : 'false';
    }
    if (x === null) return 'null';
    return describe(x).description || '';
  }

  function propertyPreview(name, x) {
    const p = { name, type: x === null ? 'object' : typeof x, value: previewText(x) };
    if (x === null) p.subtype = 'null';
    else if (p.type === 'object') {
      const s = describe(x).subtype;
      if (s) p.subtype = s;
    }
    return p;
  }

  function entryPreview(x) {
    if (x === null || typeof x !== 'object') {
      const p = { type: x === null ? 'object' : typeof x, description: previewText(x), overflow: false, properties: [] };
      if (x === null) p.subtype = 'null';
      return p;
    }
    return objectPreview(x, undefined, false, undefined);
  }

  function objectPreview(v, description, withEntries, promise) {
    const d = describe(v);
    const out = { type: d.type, description: description !== undefined ? description : (d.description || ''),
                  overflow: false, properties: [] };
    if (d.subtype) out.subtype = d.subtype;
    const props = out.properties;
    const isArray = d.subtype === 'array' || d.subtype === 'typedarray';
    const limit = isArray ? 100 : 5;
    const push = (name, x) => {
      if (props.length >= limit) { out.overflow = true; return false; }
      apply(arrayPush, props, [propertyPreview(name, x)]);
      return true;
    };
    try {
      const prim = primitiveOf(v);
      if (prim) { push('[[PrimitiveValue]]', prim.value); return out; }
      if (v instanceof P) {
        if (promise) {
          push('[[PromiseState]]', promise[0]);
          if (promise[0] !== 'pending') push('[[PromiseResult]]', promise[1]);
        }
        return out;
      }
      const kind = collectionKind(v);
      if (kind) {
        push('size', apply(kind === 'map' ? mapSize : setSize, v, []));
        if (withEntries) {
          out.entries = [];
          const it = apply(kind === 'map' ? mapEntries : setValues, v, []);
          for (let n = 0; ; n++) {
            const r = apply(kind === 'map' ? mapIterNext : setIterNext, it, []);
            if (r.done) break;
            if (n >= 5) { out.overflow = true; break; }
            apply(arrayPush, out.entries, [kind === 'map'
              ? { key: entryPreview(r.value[0]), value: entryPreview(r.value[1]) }
              : { value: entryPreview(r.value) }]);
          }
        }
        return out;
      }
      if (isArray) {
        const length = v.length;
        for (let i = 0; i < length; i++) {
          if (!(i in v)) continue;
          if (!push(StringC(i), v[i])) break;
        }
        return out;
      }
      if (d.subtype === 'error') {
        for (const k of ['stack', 'message']) {
          const desc = getOwnPropertyDescriptor(v, k);
          if (desc && 'value' in desc) push(k, desc.value);
        }
        return out;
      }
      const keys = getOwnPropertyNames(v);
      for (let i = 0; i < keys.length; i++) {
        const desc = getOwnPropertyDescriptor(v, keys[i]);
        if (!desc || !desc.enumerable) continue;
        if (!('value' in desc)) {
          // An accessor: its getter is never run.
          if (props.length >= limit) { out.overflow = true; break; }
          apply(arrayPush, props, [{ name: keys[i], type: 'accessor' }]);
          continue;
        }
        if (!push(keys[i], desc.value)) break;
      }
    } catch (_) {}
    return out;
  }

  return H;
})()
