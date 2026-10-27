// Internal per-realm helpers. Evaluated once per realm before any page script;
// the returned object is held natively and is not reachable from page globals.
// Captures intrinsics up front so later page monkey-patching cannot affect them.
(function () {
  'use strict';
  const O = Object;
  const defineProperty = O.defineProperty;
  const getPrototypeOf = O.getPrototypeOf;
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
      else description = className;
    } else if (type === 'function') {
      className = 'Function';
      try { description = apply(fnToString, v, []); } catch (_) { description = 'function'; }
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
  };
  return H;
})()
