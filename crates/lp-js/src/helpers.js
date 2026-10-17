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
  const P = Promise;
  const promiseThen = P.prototype.then;
  const promiseResolve = P.resolve.bind(P);
  const call = Function.prototype.call;
  const isArray = Array.isArray;
  const toStringTag = Symbol.toStringTag;
  const objToString = O.prototype.toString;
  const fnToString = Function.prototype.toString;
  const ErrorCtor = Error;

  const DOM_EXCEPTION_CODES = {
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
  const names = new WeakMap();
  class DOMException extends ErrorCtor {
    constructor(message = '', name = 'Error') {
      super(String(message));
      names.set(this, String(name));
    }
    get name() { return names.has(this) ? names.get(this) : 'Error'; }
    get code() { return DOM_EXCEPTION_CODES[names.get(this)] || 0; }
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
      else if (v instanceof RegExp) subtype = 'regexp';
      else if (v instanceof Date) subtype = 'date';
      else if (v instanceof Map) subtype = 'map';
      else if (v instanceof Set) subtype = 'set';
      else if (v instanceof WeakMap) subtype = 'weakmap';
      else if (v instanceof WeakSet) subtype = 'weakset';
      else if (v instanceof ErrorCtor) subtype = 'error';
      else if (v instanceof P) subtype = 'promise';
      else if (ArrayBuffer.isView(v)) subtype = v instanceof DataView ? 'dataview' : 'typedarray';
      else if (v instanceof ArrayBuffer) subtype = 'arraybuffer';
      else if (typeof v.nodeType === 'number' && typeof v.nodeName === 'string') subtype = 'node';
      try {
        const ctor = v.constructor;
        className = (ctor && typeof ctor.name === 'string' && ctor.name) || undefined;
      } catch (_) {}
      if (!className) {
        const tag = call.call(objToString, v);
        className = tag.slice(8, -1);
      }
      if (subtype === 'array') description = `${className}(${v.length})`;
      else if (subtype === 'error') { try { description = String(v.stack || v); } catch (_) { description = className; } }
      else if (subtype === 'node') description = String(v.nodeName).toLowerCase();
      else if (subtype === 'regexp' || subtype === 'date') description = String(v);
      else description = className;
    } else if (type === 'function') {
      className = 'Function';
      try { description = call.call(fnToString, v); } catch (_) { description = 'function'; }
    }
    return { type, subtype, className, description };
  }

  return {
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
      defineProperty(C, 'length', { value: length });
      defineProperty(C, 'prototype', { value: proto, writable: false, enumerable: false, configurable: false });
      defineProperty(proto, 'constructor', { value: C, writable: true, enumerable: false, configurable: true });
      defineProperty(proto, toStringTag, { value: name, writable: false, enumerable: false, configurable: true });
      if (parentCtor) setPrototypeOf(C, parentCtor);
      if (global) defineProperty(global, name, { value: C, writable: true, enumerable: false, configurable: true });
      return C;
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
      call.call(promiseThen, promiseResolve(value), onFulfilled, onRejected);
    },
    callWith(fn, thisArg, ...args) {
      return call.call(fn, thisArg, ...args);
    },
    describe,
    properties(obj, ownOnly) {
      const out = [];
      const seen = new Set();
      let o = obj;
      let isOwn = true;
      while (o !== null && o !== undefined) {
        for (const key of [...getOwnPropertyNames(o), ...getOwnPropertySymbols(o)]) {
          if (seen.has(key)) continue;
          seen.add(key);
          const d = getOwnPropertyDescriptor(o, key);
          const e = { name: typeof key === 'symbol' ? String(key) : key, isOwn,
                      writable: !!d.writable, enumerable: !!d.enumerable, configurable: !!d.configurable };
          if (typeof key === 'symbol') e.symbol = key;
          if ('value' in d) e.value = d.value;
          if (d.get) e.get = d.get;
          if (d.set) e.set = d.set;
          // Accessors on prototypes: report the current value for convenience.
          if (!isOwn && d.get) { try { e.value = Reflect.get(o, key, obj); e.isAccessorValue = true; } catch (_) {} }
          out.push(e);
        }
        if (ownOnly) break;
        o = getPrototypeOf(o);
        isOwn = false;
      }
      return out;
    },
  };
})()
