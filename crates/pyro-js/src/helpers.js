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
  // An indexed iterable's members (`iterable` below), as the realm made them.
  const arrayIterableNames = ['entries', 'keys', 'values', 'forEach'];
  const arrayIterables = [ArrayProto.entries, ArrayProto.keys, ArrayProto.values, ArrayProto.forEach];
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
  const reflectConstruct = Reflect.construct;
  // Interface state for member installation after disposal (see realm.rs
  // populate_disposed); `populated` is kept in sync by the native side.
  // Held here, so it lives exactly as long as some interface proxy (whose
  // handler's traps reference this object) is reachable. `hooks[id]` runs
  // once class `id` is populated (realm.rs `after_populate`); most classes
  // have none, and a read of a missing one must not reach Array.prototype.
  const classProtos = [];
  const classTargets = [];
  const populated = [];
  const hooks = { __proto__: null };
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
  const symbolHasInstance = Symbol.hasInstance;
  const symbolIsConcatSpreadable = Symbol.isConcatSpreadable;
  const symbolIterator = Symbol.iterator;
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
  const mapGet = MapC.prototype.get, mapSet = MapC.prototype.set;
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

  // An interface attribute: accessor pair, named `get x`/`set x`.
  // Interfaces are populated when a page first uses them, after its scripts
  // may have put `get`, `set` or `value` on Object.prototype: the member
  // installation's descriptors have no prototype.
  function defineAccessor(proto, name, get, set, enumerable) {
    try { defineProperty(get, 'name', { __proto__: null, value: 'get ' + name }); } catch (_) {}
    if (set) { try { defineProperty(set, 'name', { __proto__: null, value: 'set ' + name }); } catch (_) {} }
    defineProperty(proto, name, { __proto__: null, get, set, enumerable: enumerable !== false, configurable: true });
  }

  // DOMException (https://webidl.spec.whatwg.org/#idl-DOMException): a
  // platform object whose prototype chains to Error.prototype, with `name`,
  // `message` and `code` as brand-checked accessors on the prototype (not own
  // data properties) and no class-side inheritance from Error. The instance
  // is a real Error (`Reflect.construct(Error, ...)`), so it carries JSC's
  // stack trace, and takes its prototype from `new.target` (QuotaExceededError
  // and page subclasses), falling back to DOMException.prototype when that
  // is not an object ("internally create a new object implementing the
  // interface", https://webidl.spec.whatwg.org/#internally-create-a-new-object-implementing-the-interface).
  const exceptionSlots = new WM();
  const DOMException = { DOMException: function (message = '', name = 'Error') {
    if (new.target === undefined) {
      throw new TypeError("Failed to construct 'DOMException': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    // DOMString conversion is ToString: a Symbol throws (String() would
    // describe it instead).
    const slots = [`${message}`, `${name}`];
    const e = reflectConstruct(ErrorCtor, [], DOMException);
    if (new.target !== DOMException) {
      // Read after the arguments are converted.
      const proto = new.target.prototype;
      if (proto !== null && (typeof proto === 'object' || typeof proto === 'function')) setPrototypeOf(e, proto);
    }
    apply(wmSet, exceptionSlots, [e, slots]);
    return e;
  } }.DOMException;
  const exceptionSlot = (self, member) => {
    const slots = apply(wmGet, exceptionSlots, [self]);
    if (slots === undefined) {
      throw new TypeError(`Failed to read the '${member}' property from 'DOMException': Illegal invocation`);
    }
    return slots;
  };
  const DOMExceptionProto = objectCreate(ErrorCtor.prototype);
  defineProperty(DOMException, 'prototype', { value: DOMExceptionProto, writable: false, enumerable: false, configurable: false });
  defineProperty(DOMExceptionProto, 'constructor', { value: DOMException, writable: true, enumerable: false, configurable: true });
  defineAccessor(DOMExceptionProto, 'name', function name() { return exceptionSlot(this, 'name')[1]; });
  defineAccessor(DOMExceptionProto, 'message', function message() { return exceptionSlot(this, 'message')[0]; });
  defineAccessor(DOMExceptionProto, 'code', function code() { return DOM_EXCEPTION_CODES[exceptionSlot(this, 'code')[1]] || 0; });
  defineProperty(DOMExceptionProto, toStringTag, { value: 'DOMException', configurable: true });
  DOM_EXCEPTION_CONSTANTS.forEach((n, i) => {
    defineProperty(DOMException, n, { value: i + 1, enumerable: true });
    defineProperty(DOMExceptionProto, n, { value: i + 1, enumerable: true });
  });
  defineProperty(globalThis, 'DOMException', { value: DOMException, writable: true, configurable: true });

  // QuotaExceededError: the DOMException subclass WebIDL makes of the former
  // "QuotaExceededError" name (https://webidl.spec.whatwg.org/#quotaexceedederror).
  // Native code throwing that name constructs this class (error.rs).
  const quotaLimits = new WM();
  const NumberC = Number;
  const isFinite = NumberC.isFinite;
  const toDouble = (v, member) => {
    const n = NumberC(v);
    if (!isFinite(n)) {
      throw new TypeError(`Failed to construct 'QuotaExceededError': Failed to read the '${member}' property from 'QuotaExceededErrorOptions': The provided double value is non-finite.`);
    }
    return n;
  };
  class QuotaExceededError extends DOMException {
    constructor(message = '', options = {}) {
      super(message, 'QuotaExceededError');
      // Dictionary conversion first (members in lexicographic order), then the
      // constructor's range checks.
      let quota = null, requested = null;
      if (options !== undefined && options !== null) {
        if (typeof options !== 'object' && typeof options !== 'function') {
          throw new TypeError("Failed to construct 'QuotaExceededError': The provided value is not of type 'QuotaExceededErrorOptions'.");
        }
        const q = options.quota;
        if (q !== undefined) quota = toDouble(q, 'quota');
        const r = options.requested;
        if (r !== undefined) requested = toDouble(r, 'requested');
      }
      if (quota !== null && quota < 0) {
        throw new RangeError("Failed to construct 'QuotaExceededError': The quota must not be negative.");
      }
      if (requested !== null && requested < 0) {
        throw new RangeError("Failed to construct 'QuotaExceededError': The requested value must not be negative.");
      }
      if (quota !== null && requested !== null && requested < quota) {
        throw new RangeError("Failed to construct 'QuotaExceededError': The requested value must not be less than the quota.");
      }
      apply(wmSet, quotaLimits, [this, [requested, quota]]);
    }
    get requested() {
      if (!apply(wmHas, quotaLimits, [this])) throw new TypeError('Illegal invocation');
      return apply(wmGet, quotaLimits, [this])[0];
    }
    get quota() {
      if (!apply(wmHas, quotaLimits, [this])) throw new TypeError('Illegal invocation');
      return apply(wmGet, quotaLimits, [this])[1];
    }
  }
  defineProperty(QuotaExceededError.prototype, toStringTag, { value: 'QuotaExceededError', configurable: true });
  defineProperty(globalThis, 'QuotaExceededError', { value: QuotaExceededError, writable: true, configurable: true });

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

  // Interface functions get their `name` defined, as SetFunctionName leaves
  // it: naming through a computed key (`({ [name]: function () {} })[name]`)
  // made a structure transition and two inline cache misses per distinct
  // name, hundreds per realm. Own keys stay length, name, prototype. Used
  // while the realm is made only, so an ordinary descriptor (JSC reads one
  // inheriting from an untouched Object.prototype without lookups).
  const nameValue = { value: '', writable: false, enumerable: false, configurable: true };
  const named = (fn, name) => {
    nameValue.value = name;
    defineProperty(fn, 'name', nameValue);
    return fn;
  };

  // A constructor shim over `native(newTarget, args)` (class.rs ctor_invoke),
  // named `name`; its own keys come in the order length, name, prototype.
  function nativeConstructor(name, native, length) {
    const C = named(function (...args) {
      if (new.target === undefined) {
        throw new TypeError(`Failed to construct '${name}': Please use the 'new' operator, this DOM object constructor cannot be called as a function.`);
      }
      return native(new.target, args);
    }, name);
    if (length !== 0) defineProperty(C, 'length', { value: length });
    return C;
  }

  // Interface objects are Proxies over their constructors whose property
  // traps first install the class members natively (once), then forward
  // unchanged. Construction, calls and [[GetPrototypeOf]] need no members.
  // Every proxy of a realm shares these traps: a handler is an object over
  // them that only carries its class id, so an interface costs one small
  // object here instead of a closure per trap.
  let populateClass = null;
  const ensureClass = (id) => { if (!populated[id]) populateClass(id, H); };
  // No prototype: a trap the handler does not define (`apply`, `construct`)
  // must not be found on Object.prototype, where a page could have put one.
  const lazyTraps = {
    __proto__: null,
    get(t, k, r) { ensureClass(this.id); return reflectGet(t, k, r); },
    set(t, k, v, r) { ensureClass(this.id); return reflectSet(t, k, v, r); },
    has(t, k) { ensureClass(this.id); return reflectHas(t, k); },
    ownKeys(t) { ensureClass(this.id); return reflectOwnKeys(t); },
    getOwnPropertyDescriptor(t, k) { ensureClass(this.id); return reflectGOPD(t, k); },
    defineProperty(t, k, d) { ensureClass(this.id); return reflectDefine(t, k, d); },
    deleteProperty(t, k) { ensureClass(this.id); return reflectDelete(t, k); },
    // Members must exist before the target stops accepting properties.
    preventExtensions(t) { ensureClass(this.id); return reflectPreventExtensions(t); },
  };
  function lazyProxy(target, id) {
    const handler = objectCreate(lazyTraps);
    handler.id = id;
    return new ProxyC(target, handler);
  }
  // Descriptors reused for every interface (only `value` changes): the
  // installation defines a few properties per interface, hundreds per realm.
  // `exposedValue` is used while the realm is made, so it is an ordinary
  // object (JSC's fast descriptor read).
  const exposedValue = { value: undefined, writable: true, enumerable: false, configurable: true };
  // Population runs after page script. A data descriptor's four fields are
  // its own; `get` and `set` an ordinary one looks up on Object.prototype,
  // where a page may have put them. While it has neither, the ordinary
  // descriptor [0] reads the same as the one without a prototype [1], and
  // JSC reads it without lookups (a define costs about 40% less).
  // `descriptorIndex()` says which applies now.
  const objectProto = O.prototype;
  const objectHasOwn = O.hasOwn;
  const descriptorIndex = () => (objectHasOwn(objectProto, 'get') || objectHasOwn(objectProto, 'set') ? 1 : 0);
  const dataDescriptors = (writable, enumerable, configurable) => [
    { value: undefined, writable, enumerable, configurable },
    { __proto__: null, value: undefined, writable, enumerable, configurable },
  ];
  const hiddenValues = dataDescriptors(true, false, true);
  const fixedValues = dataDescriptors(false, false, false);
  const tagValues = dataDescriptors(false, false, true);
  const memberValues = dataDescriptors(true, true, true);
  const constantValues = dataDescriptors(false, true, false);
  const defineData = (o, k, d, value) => { d.value = value; defineProperty(o, k, d); d.value = undefined; };

  // The realm's interface table (`installInterfaces`), for `materialize`:
  // the names, and per interface [parent or -1, constructor length, flags]
  // (class.rs `SPEC_*`).
  let interfaceNames = null, interfaceData = null, interfaceObjectProto = null, interfaceLists = null;
  const SPEC_EXPOSED = 1, SPEC_CALLBACK = 2, SPEC_STRIDE = 3;
  // [LegacyFactoryFunction]s by interface id: their `prototype` is the
  // interface prototype object, defined when it is made. No prototype: a
  // read of an interface without any, after page script, meets no getter
  // a page put on Array.prototype or Object.prototype.
  const factoryTargets = { __proto__: null };

  const H = {
    classProtos,
    classTargets,
    populated,
    hooks,
    DOMException,
    // Every interface of a new realm in one call (realm.rs install_classes):
    // `names` are the interface names separated by newlines, `data` an
    // Int32Array of [parent index or -1, constructor length, flags] per
    // interface (SPEC_EXPOSED: a property of the global; SPEC_CALLBACK: a
    // callback interface). Returns [protos, targets, ctors], indexed like
    // the names; `protos[i]` stays empty until `materialize(i)`. A callback
    // interface (NodeFilter) is not a constructor and has no `prototype`
    // property, so `instanceof` on it throws
    // (https://webidl.spec.whatwg.org/#legacy-callback-interface-object).
    installInterfaces(names, data, natives, objectProto, global, populate) {
      populateClass = populate;
      interfaceNames = names.split('\n');
      interfaceData = data;
      interfaceObjectProto = objectProto;
      const protos = [], targets = [], ctors = [];
      interfaceLists = [protos, targets, ctors];
      for (let i = 0; i < interfaceNames.length; i++) {
        const name = interfaceNames[i], k = i * SPEC_STRIDE;
        const parent = data[k], length = data[k + 1], flags = data[k + 2];
        const exposed = (flags & SPEC_EXPOSED) !== 0, callback = (flags & SPEC_CALLBACK) !== 0;
        const native = natives[i];
        const C = native
          ? nativeConstructor(name, native, length)
          : callback
            // Not a constructor, no `prototype` (as a method would be).
            ? named(() => { throw new TypeError('Illegal constructor'); }, name)
            : named(function () { throw new TypeError('Illegal constructor'); }, name);
        if (!native && length !== 0) defineProperty(C, 'length', { value: length });
        if (parent >= 0) setPrototypeOf(C, ctors[parent]);
        classTargets[i] = C;
        const P = lazyProxy(C, i);
        if (exposed) {
          exposedValue.value = P;
          defineProperty(global, name, exposedValue);
        }
        targets[i] = C;
        ctors[i] = P;
        // Own slots now, while no page script ran: what is stored later (by
        // `materialize`, or natively) must not reach a setter, nor a read a
        // getter, that a page put on Array.prototype.
        protos[i] = undefined;
        classProtos[i] = undefined;
        populated[i] = false;
      }
      exposedValue.value = undefined;
      return interfaceLists;
    },
    // The interface prototype object of `id`, made when the interface is
    // first populated (realm.rs populate_class, root first, so the parent's
    // exists): before that nothing can reach it, as every way there (an
    // instance, the interface object's traps) populates first. A realm
    // makes the few its page uses instead of all of them.
    // Every step but the creation runs again when called again: a first use
    // that failed part way (a stack overflow, a termination) is completed by
    // the next one instead of leaving a prototype without its constructor or
    // tag. Redefining a property with its own value is allowed, even for the
    // non-configurable `prototype`. The native side calls this until it
    // returned once (realm.rs `materialize`).
    materialize(id) {
      // Runs while page script may have changed Array.prototype: indexed
      // reads of own elements only (no destructuring, no iteration).
      const name = interfaceNames[id], parent = interfaceData[id * SPEC_STRIDE];
      const callback = (interfaceData[id * SPEC_STRIDE + 2] & SPEC_CALLBACK) !== 0;
      let proto = classProtos[id];
      if (proto === undefined) {
        proto = objectCreate(parent < 0 ? interfaceObjectProto : classProtos[parent]);
        classProtos[id] = proto;
      }
      interfaceLists[0][id] = proto;
      const k = descriptorIndex();
      if (!callback) {
        defineData(interfaceLists[1][id], 'prototype', fixedValues[k], proto);
        const factories = factoryTargets[id];
        if (factories) for (let i = 0; i < factories.length; i++) defineData(factories[i], 'prototype', fixedValues[k], proto);
      }
      defineData(proto, toStringTag, tagValues[k], name);
      defineData(proto, 'constructor', hiddenValues[k], interfaceLists[2][id]);
      return proto;
    },
    // A [LegacyFactoryFunction] of interface `id` (`new Image(w, h)`): a
    // constructor whose `prototype` is the interface prototype object, own
    // keys in the order length, name, prototype, with the same lazy member
    // installation as the interface object
    // (https://webidl.spec.whatwg.org/#legacy-factory-functions).
    legacyFactory(global, name, native, length, id, populate) {
      const F = nativeConstructor(name, native, length);
      const proto = classProtos[id];
      if (proto !== undefined) defineProperty(F, 'prototype', { value: proto, writable: false, enumerable: false, configurable: false });
      else (factoryTargets[id] ??= []).push(F);
      populateClass = populate;
      const P = lazyProxy(F, id);
      defineProperty(global, name, { value: P, writable: true, enumerable: false, configurable: true });
      return P;
    },
    accessor: defineAccessor,
    // The global class's members, on the global since its creation, onto
    // its interface prototype object when a script first reaches it: the
    // same functions and attributes (realm.rs `copy_global_members`).
    copyMembers(from, to, names) {
      for (let i = 0; i < names.length; i++) {
        const d = getOwnPropertyDescriptor(from, names[i]);
        if (d !== undefined) defineProperty(to, names[i], d);
      }
    },
    // Methods or statics of a population step, `list` as name, function,
    // name, function, ... (realm.rs `MemberBatch`). Defined, not assigned:
    // a page may have put an accessor of the same name on Object.prototype
    // (or a parent interface object) before the interface's first use,
    // which an assignment would call instead of creating the member.
    members(target, ...list) {
      const d = memberValues[descriptorIndex()];
      for (let i = 0; i < list.length; i += 2) {
        d.value = list[i + 1];
        defineProperty(target, list[i], d);
      }
      d.value = undefined;
    },
    // WebIDL sequence<T> conversion (https://webidl.spec.whatwg.org/#es-sequence):
    // the iterator protocol of `v` itself (arrays included: an overridden
    // @@iterator or %ArrayIteratorPrototype%.next is honoured, holes read
    // through the prototype), as a fresh array of the yielded values.
    sequence(v, what, convert) {
      if (v === null || (typeof v !== 'object' && typeof v !== 'function')) {
        throw new TypeError(`${what}: The provided value cannot be converted to a sequence.`);
      }
      const method = v[symbolIterator];
      if (typeof method !== 'function') {
        throw new TypeError(`${what}: The object must have a callable @@iterator property.`);
      }
      const out = convert === undefined ? [] : undefined;
      const iterator = apply(method, v, []);
      if (iterator === null || (typeof iterator !== 'object' && typeof iterator !== 'function')) throw new TypeError(`${what}: Iterator result is not an object.`);
      const next = iterator.next;
      for (let n = 0; ; n++) {
        const r = apply(next, iterator, []);
        if (r === null || (typeof r !== 'object' && typeof r !== 'function')) throw new TypeError(`${what}: Iterator result is not an object.`);
        if (r.done) return out;
        const item = r.value;
        if (convert !== undefined) {
          // WebIDL 3.2.21.1 propagates member conversion errors directly.
          convert(item);
        } else {
          // CreateDataProperty avoids accessors on Array.prototype's indices.
          reflectDefine(out, n, { value: item, writable: true, enumerable: true, configurable: true });
        }
      }
    },
    // Population steps (see `descriptorIndex` and defineAccessor):
    // descriptors whose fields are read as their own, no iteration, the
    // Array.prototype methods of the realm's creation.
    // An interface's constants, `list` as name, value, ...: each on the
    // interface object, then on its prototype (realm.rs `MemberBatch`).
    constants(target, proto, ...list) {
      const d = constantValues[descriptorIndex()];
      for (let i = 0; i < list.length; i += 2) {
        d.value = list[i + 1];
        defineProperty(target, list[i], d);
        defineProperty(proto, list[i], d);
      }
      d.value = undefined;
    },
    iterable(proto) {
      defineProperty(proto, symbolIterator, { __proto__: null, value: arrayIterables[2], writable: true, configurable: true });
      for (let i = 0; i < arrayIterableNames.length; i++) {
        defineProperty(proto, arrayIterableNames[i], { __proto__: null, value: arrayIterables[i], writable: true, enumerable: true, configurable: true });
      }
    },
    // An indexed getter without an `iterable<V>` declaration: @@iterator only.
    iterator(proto) {
      defineProperty(proto, symbolIterator, { __proto__: null, value: arrayIterables[2], writable: true, configurable: true });
    },

    setLength(fn, length) {
      try { defineProperty(fn, 'length', { __proto__: null, value: length }); } catch (_) {}
    },
    setName(fn, name) {
      try { defineProperty(fn, 'name', { __proto__: null, value: name }); } catch (_) {}
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
    // `ownLast`: the global class's own members are on the global already
    // (realm.rs `install_classes`); its prototype is still empty. The
    // brand goes on the constructor functions (`targets`), not through the
    // interface objects, whose traps would populate the prototypes.
    installGlobal(global, protos, ctors, targets, name, ownLast) {
      for (let i = 0; i < protos.length - (ownLast ? 1 : 0); i++) {
        const proto = protos[i];
        for (const key of apply(arrayConcat, getOwnPropertyNames(proto), [getOwnPropertySymbols(proto)])) {
          if (key === 'constructor' || key === toStringTag) continue;
          defineProperty(global, key, getOwnPropertyDescriptor(proto, key));
        }
      }
      for (let i = 0; i < targets.length; i++) {
        const C = targets[i];
        defineProperty(C, Symbol.hasInstance, {
          value: { [Symbol.hasInstance](v) { return v === global || apply(hasInstance, this, [v]); } }[Symbol.hasInstance],
          configurable: true,
        });
      }
      // The global is not an instance of its interface prototype object, so
      // `constructor` (found on that prototype in a browser) is copied too:
      // `window.constructor === Window`.
      defineProperty(global, 'constructor', { value: ctors[ctors.length - 1], writable: true, enumerable: false, configurable: true });
      defineProperty(global, toStringTag, { value: name, writable: false, enumerable: false, configurable: true });
      installNamedPropertiesObject(global, protos, name);
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
    // A WindowProxy for a Window the embedder reaches through `target()`:
    // the Window's global object when it is same origin with this realm,
    // null while it has none (the stand-in's own members and expandos
    // then answer), its origin (a string) when it is cross origin. `standIn`
    // is the native object with the members a WindowProxy keeps whatever
    // its Window (`closed`, `close()`, `location`, `opener`, `postMessage()`
    // ...), `child(key)` answers a child window by index or by target name
    // as this realm may see it cross origin (undefined for none), and
    // `deny(name)` throws the SecurityError of a cross-origin access. HTML's
    // WindowProxy internal methods: same origin, every one goes to the
    // Window; cross origin, only its child windows (by index and name),
    // CrossOriginProperties (the methods as functions of this realm,
    // `location` a cross-origin Location with only `href` (set) and
    // `replace()`) and `then` and the well-known symbols (undefined) are
    // there, and anything else throws. What is handed out cross origin is
    // kept per (origin, key) as crossOriginPropertyDescriptorMap has it,
    // apart from what same origin hands out. Functions read through it
    // run with the Window, not the proxy, as `this` (a WindowProxy is the
    // Window to the bindings). Neither `standIn` nor the target is ever
    // handed to page code, and no list is made through page-observable
    // hooks (species, spreading, iteration).
    windowProxy(standIn, target, child, deny) {
      const ALWAYS = { __proto__: null, closed: 1, close: 1, focus: 1, blur: 1 };
      const SELF = { __proto__: null, window: 1, self: 1, frames: 1, top: 1, parent: 1 };
      const CROSS_GETTERS = { __proto__: null, window: 1, self: 1, frames: 1, top: 1, parent: 1, location: 1, opener: 1, closed: 1, length: 1 };
      const CROSS_METHODS = { __proto__: null, close: 1, focus: 1, blur: 1, postMessage: 1 };
      const CROSS_NAMES = ['window', 'self', 'location', 'close', 'closed', 'focus', 'blur', 'frames', 'length', 'top', 'opener', 'parent', 'postMessage', 'then'];
      const undefinedNames = (k) => k === 'then' || k === toStringTag || k === symbolHasInstance || k === symbolIsConcatSpreadable;
      const isIndex = (k) => typeof k === 'string' && k !== '' && StringC(k >>> 0) === k && (k >>> 0) !== 4294967295;
      const isCross = (w) => typeof w === 'string';
      const named = (fn, name) => { try { defineProperty(fn, 'name', { __proto__: null, value: name }); } catch (_) {} };
      const data = (value, enumerable) => ({ __proto__: null, value, writable: false, enumerable, configurable: true });
      // Appends without [[Set]] (no setter of Array.prototype runs).
      const append = (list, value) => { reflectDefine(list, list.length, data(value, true)); };
      const expandos = objectCreate(null);
      let proxy;
      // Same origin (and before the page): functions of the Window or the
      // stand-in, each as one function that calls it with `self()` for the
      // proxy as `this` (kept per function).
      const wrapped = new WeakMapC();
      const wrap = (fn, self) => {
        if (typeof fn !== 'function') return fn;
        let w = apply(wmGet, wrapped, [fn]);
        if (w === undefined) {
          w = new ProxyC(fn, { __proto__: null, apply: (f, thisArg, args) => apply(f, thisArg === proxy ? self() : thisArg, args) });
          apply(wmSet, wrapped, [fn, w]);
        }
        return w;
      };
      // Cross origin: what was handed out, by origin and key.
      const crossMap = new MapC();
      const crossKept = (origin, key, make) => {
        const id = origin + '\0' + (typeof key === 'symbol' ? apply(symbolToString, key, []) : key);
        let v = apply(mapGet, crossMap, [id]);
        if (v === undefined) {
          v = make();
          apply(mapSet, crossMap, [id, v]);
        }
        return v;
      };
      const window = () => { const w = target(); return w === null || isCross(w) ? null : w; };
      const standInSelf = () => standIn;
      const fromStandIn = (k) => SELF[k] ? proxy : wrap(reflectGet(standIn, k, standIn), standInSelf);
      // A cross-origin method: a function of this realm calling the stand-in's.
      const crossMethod = (origin, k) => crossKept(origin, k, () => {
        const method = reflectGet(standIn, k, standIn);
        const f = (...args) => apply(method, standIn, args);
        named(f, k);
        return f;
      });
      // The cross-origin Location: `href` to set, `replace()`, nothing else.
      const crossLocation = (origin) => crossKept(origin, 'location', () => {
        const location = () => reflectGet(standIn, 'location', standIn);
        const replace = (url) => { const l = location(); apply(reflectGet(l, 'replace', l), l, [url]); };
        named(replace, 'replace');
        const setHref = (v) => { const l = location(); reflectSet(l, 'href', v, l); };
        const keys = () => ['href', 'replace', 'then', toStringTag, symbolHasInstance, symbolIsConcatSpreadable];
        const describe = (k) => {
          if (k === 'replace') return data(replace, false);
          if (k === 'href') return { __proto__: null, get: undefined, set: setHref, enumerable: false, configurable: true };
          if (undefinedNames(k)) return data(undefined, false);
          return deny(k);
        };
        return new ProxyC(objectCreate(null), {
          __proto__: null,
          get(t, k) { const d = describe(k); if ('value' in d) return d.value; return deny(k); },
          set(t, k, v) { if (k === 'href') { setHref(v); return true; } return deny(k); },
          has(t, k) { describe(k); return true; },
          deleteProperty(t, k) { return deny(k); },
          defineProperty(t, k) { return deny(k); },
          getOwnPropertyDescriptor(t, k) { return describe(k); },
          ownKeys() { return keys(); },
          getPrototypeOf() { return null; },
          setPrototypeOf(t, v) { return v === null; },
          isExtensible() { return true; },
          preventExtensions() { return false; },
        });
      });
      // The getter and setter pair kept per (origin, key), described afresh.
      const accessors = ({ get, set }) => ({ __proto__: null, describe: () => ({ __proto__: null, get, set, enumerable: false, configurable: true }) });
      // HTML's CrossOriginGetOwnPropertyHelper and the child windows.
      const crossDescriptor = (origin, k) => {
        if (isIndex(k)) {
          const w = child(k);
          return w !== undefined ? data(w, false) : deny(k);
        }
        if (CROSS_METHODS[k]) return data(crossMethod(origin, k), false);
        if (CROSS_GETTERS[k]) {
          return crossKept(origin, 'get ' + k, () => accessors((() => {
            const get = () => (k === 'location' ? crossLocation(origin) : CROSS_METHODS[k] ? crossMethod(origin, k) : fromStandIn(k));
            named(get, 'get ' + k);
            let set;
            if (k === 'location') {
              set = (v) => { const l = reflectGet(standIn, 'location', standIn); reflectSet(l, 'href', v, l); };
              named(set, 'set location');
            }
            return { __proto__: null, get, set };
          })())).describe();
        }
        if (typeof k === 'string') {
          const w = child(k);
          if (w !== undefined) return data(w, false);
        }
        if (undefinedNames(k)) return data(undefined, false);
        return deny(k);
      };
      const crossGet = (origin, k) => {
        const d = crossDescriptor(origin, k);
        if ('value' in d) return d.value;
        return d.get === undefined ? deny(k) : apply(d.get, proxy, []);
      };
      const crossKeys = () => {
        const keys = [];
        const length = reflectGet(standIn, 'length', standIn);
        for (let i = 0; i < length; i++) append(keys, StringC(i));
        for (let i = 0; i < CROSS_NAMES.length; i++) append(keys, CROSS_NAMES[i]);
        append(keys, toStringTag);
        append(keys, symbolHasInstance);
        append(keys, symbolIsConcatSpreadable);
        return keys;
      };
      const handler = {
        __proto__: null,
        get(t, k) {
          const w = target();
          if (isCross(w)) return crossGet(w, k);
          if (ALWAYS[k]) return fromStandIn(k);
          if (SELF[k]) return proxy;
          if (w === null) return k in expandos ? expandos[k] : fromStandIn(k);
          if (k === 'opener') return fromStandIn(k);
          return wrap(reflectGet(w, k, w), window);
        },
        set(t, k, v) {
          const w = target();
          if (isCross(w)) {
            if (k === 'location') {
              const l = reflectGet(standIn, 'location', standIn);
              return reflectSet(l, 'href', v, l);
            }
            return deny(k);
          }
          if (isIndex(k)) return false;
          // `opener` and `location` keep the stand-in's state too.
          if (k === 'opener' || k === 'location') return reflectSet(standIn, k, v, standIn);
          if (w === null) return reflectSet(expandos, k, v, expandos);
          return reflectSet(w, k, v, w);
        },
        has(t, k) {
          const w = target();
          if (isCross(w)) {
            crossDescriptor(w, k);
            return true;
          }
          if (w === null) return reflectHas(expandos, k) || ALWAYS[k] === 1 || SELF[k] === 1 || k === 'location' || k === 'opener';
          return reflectHas(w, k);
        },
        deleteProperty(t, k) {
          const w = target();
          if (isCross(w)) return deny(k);
          if (w === null) return reflectDelete(expandos, k);
          if (isIndex(k)) return !reflectHas(w, k);
          return reflectDelete(w, k);
        },
        defineProperty(t, k, d) {
          const w = target();
          if (isCross(w)) return deny(k);
          if (isIndex(k)) return false;
          // The proxy can only report configurable properties (its target
          // holds none), so a non-configurable one is refused.
          if (d.configurable === false) return false;
          return reflectDefine(w === null ? expandos : w, k, d);
        },
        getOwnPropertyDescriptor(t, k) {
          const w = target();
          if (isCross(w)) return crossDescriptor(w, k);
          const d = reflectGOPD(w === null ? expandos : w, k);
          if (d === undefined) return undefined;
          d.configurable = true;
          if ('value' in d) d.value = SELF[k] ? proxy : wrap(d.value, w === null ? standInSelf : window);
          return d;
        },
        ownKeys() {
          const w = target();
          if (isCross(w)) return crossKeys();
          return reflectOwnKeys(w === null ? expandos : w);
        },
        getPrototypeOf() {
          const w = target();
          return w === null || isCross(w) ? null : getPrototypeOf(w);
        },
        setPrototypeOf(t, v) {
          return v === handler.getPrototypeOf();
        },
        isExtensible() {
          return true;
        },
        preventExtensions() {
          return false;
        },
      };
      proxy = new ProxyC(objectCreate(null), handler);
      return proxy;
    },
    // The current stack (JSC format), for console stack traces.
    stack() {
      return new ErrorCtor().stack;
    },
  };

  // The named properties object of a `[Global]` interface with named
  // properties (https://webidl.spec.whatwg.org/#named-properties-object):
  // in the prototype chain between the interface prototype object
  // (Window.prototype) and its parent's (EventTarget.prototype). JSC's named
  // lookup object (the global's immutable [[Prototype]], `real`) cannot be
  // re-chained, so the chain gets an exotic object emulating the spec's
  // internal methods over it: named properties are read through `real`,
  // subject to the named property visibility algorithm; [[DefineOwnProperty]],
  // [[Delete]] and [[PreventExtensions]] fail (so a [[Set]] with the object
  // itself as receiver fails too, unless it reaches a setter further up);
  // own keys are only @@toStringTag.
  function installNamedPropertiesObject(global, protos, name) {
    if (protos.length < 2) return;
    const real = getPrototypeOf(global);
    const tag = real !== null && real !== undefined ? getOwnPropertyDescriptor(real, toStringTag) : undefined;
    if (!tag || tag.value !== name + 'Properties') return;
    const parent = protos[protos.length - 2];
    const interfaceProto = protos[protos.length - 1];
    const target = objectCreate(parent);
    defineProperty(target, toStringTag, { value: tag.value, writable: false, enumerable: false, configurable: true });
    const isArrayIndex = (k) => { const n = k >>> 0; return n !== 0xFFFFFFFF && StringC(n) === k; };
    // Named property visibility (https://webidl.spec.whatwg.org/#dfn-named-property-visibility):
    // a supported name is hidden by an own property of the global (which
    // also carries the interface members; its indices are the WindowProxy's
    // child frames, not properties of the Window) and by a property of any
    // other object on its prototype chain.
    const named = (k) => {
      if (typeof k !== 'string') return undefined;
      if (!isArrayIndex(k) && reflectGOPD(global, k) !== undefined) return undefined;
      if (reflectGOPD(interfaceProto, k) !== undefined || reflectHas(parent, k)) return undefined;
      const d = reflectGOPD(real, k);
      return d === undefined ? undefined : { value: d.value, writable: true, enumerable: false, configurable: true };
    };
    const wp = new ProxyC(target, {
      getPrototypeOf: () => parent,
      setPrototypeOf: (t, p) => p === parent,
      isExtensible: () => true,
      preventExtensions: () => false,
      getOwnPropertyDescriptor: (t, k) => named(k) ?? reflectGOPD(t, k),
      defineProperty: () => false,
      has: (t, k) => named(k) !== undefined || reflectHas(t, k),
      get(t, k, r) { const d = named(k); return d === undefined ? reflectGet(t, k, r) : d.value; },
      // [[Set]] is ordinary. Starting at the target (which has no named
      // properties) gives the same result: a visible name is on no object
      // further up, so the lookup ends in a data property on the receiver,
      // which fails when that is this object ([[DefineOwnProperty]]); a
      // setter further up (`__proto__`, whose [[SetPrototypeOf]] then
      // fails) runs with the receiver as `this`.
      set: (t, k, v, r) => reflectSet(t, k, v, r),
      deleteProperty: () => false,
      ownKeys: (t) => reflectOwnKeys(t),
    });
    setPrototypeOf(interfaceProto, wp);
  }

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
      // The ellipsis escaped: one non-ASCII character made this whole file
      // a UTF-16 source, transcoded (and lexed and hashed at twice the
      // size) in every realm.
      case 'string': return x.length > 100 ? apply(stringSlice, x, [0, 50]) + '\u2026' + apply(stringSlice, x, [-49]) : x;
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
          // A DOMException's message is an accessor on its prototype: read
          // from the slot, so a preview never runs a page-defined getter.
          else if (k === 'message' && !desc) {
            const slots = apply(wmGet, exceptionSlots, [v]);
            if (slots !== undefined) push(k, slots[0]);
          }
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
