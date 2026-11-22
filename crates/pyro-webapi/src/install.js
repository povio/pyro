// The JavaScript every realm's bindings capture from the realm's intrinsics
// before page script can replace them (lib.rs `install`, worker_scope.rs
// `install`): one evaluation per realm instead of one per module, as the
// fixed cost of an evaluation (source string, code cache lookup, program
// code block, VM entry) was what each of them cost. A factory taking the
// structured clone factory (structured_clone.js, spliced in by install.rs)
// and returning the pieces by name; each piece is documented at its module.
(structuredCloneFactory) => {
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor, defineProperty = Object.defineProperty;
  const create = Object.create, setPrototypeOf = Object.setPrototypeOf, getPrototypeOf = Object.getPrototypeOf;
  const freeze = Object.freeze, ownKeys = Reflect.ownKeys, apply = Reflect.apply;
  const symbolIterator = Symbol.iterator, toStringTag = Symbol.toStringTag, symbolUnscopables = Symbol.unscopables;
  const symbolToPrimitive = Symbol.toPrimitive, objectValueOf = Object.prototype.valueOf;
  const TypeError_ = TypeError, RangeError_ = RangeError, String_ = String, isFinite_ = Number.isFinite;
  const Uint8Array_ = Uint8Array, arraySort = Array.prototype.sort, arrayPush = Array.prototype.push;
  const iteratorPrototypeOfArrays = getPrototypeOf(getPrototypeOf([][symbolIterator]()));
  // A value appended to an array without reaching a setter a page put on
  // Array.prototype.
  const append = (array, value) => defineProperty(array, array.length, { value, writable: true, enumerable: true, configurable: true });

  // encoding.rs: a native DataView's bytes as a genuine Uint8Array over the
  // same view, or null for any other value.
  const dataViewBuffer = getOwnPropertyDescriptor(DataView.prototype, 'buffer').get;
  const dataViewOffset = getOwnPropertyDescriptor(DataView.prototype, 'byteOffset').get;
  const dataViewLength = getOwnPropertyDescriptor(DataView.prototype, 'byteLength').get;
  const dataView = (value) => {
    let backing;
    try { backing = apply(dataViewBuffer, value, []); } catch (_) { return null; }
    return new Uint8Array_(backing, apply(dataViewOffset, value, []), apply(dataViewLength, value, []));
  };

  // mutation_observer: MutationObserverInit as [attributeFilter, attributeOldValue,
  // attributes, characterData, characterDataOldValue, childList, subtree].
  const mutationOptions = (options) => {
    if (options == null) options = {};
    if (typeof options !== 'object' && typeof options !== 'function') throw new TypeError_('Observer options must be a dictionary');
    let filter = options.attributeFilter;
    if (filter !== undefined) {
      if (filter === null || (typeof filter !== 'object' && typeof filter !== 'function')) throw new TypeError_('attributeFilter must be a sequence');
      const list = create(null); let n = 0;
      for (const item of filter) { if (typeof item === 'symbol') throw new TypeError_('Cannot convert symbol to string'); list[n++] = String_(item); }
      list.length = n; filter = list;
    }
    const old = options.attributeOldValue, attributes = options.attributes, chars = options.characterData, charOld = options.characterDataOldValue;
    return [filter, old === undefined ? undefined : !!old, attributes === undefined ? undefined : !!attributes, chars === undefined ? undefined : !!chars, charOld === undefined ? undefined : !!charOld, !!options.childList, !!options.subtree];
  };

  // intersection_observer: [the threshold list of an init, Object.freeze].
  const intersectionThresholds = (value) => {
    let list = [];
    if (value === undefined) list = [0];
    else if (value !== null && (typeof value === 'object' || typeof value === 'function')) { for (const item of value) apply(arrayPush, list, [+item]); }
    else list = [+value];
    for (let i = 0; i < list.length; i++) {
      const n = list[i];
      if (!isFinite_(n)) throw new TypeError_('threshold must be finite');
      if (n < 0 || n > 1) throw new RangeError_('threshold must be between zero and one');
    }
    if (list.length === 0) apply(arrayPush, list, [0]);
    apply(arraySort, list, [(a, b) => a - b]);
    return freeze(list);
  };

  // form_data: [the step completing FormData.prototype, the one completing
  // its iterator prototype], run when the interface is first used
  // (`after_populate`): descriptors without a prototype. Native methods do
  // not see their receiver: each shim passes `this` as the (hidden) first
  // argument of the native it replaces.
  const shim = (object, name) => {
    const native = object[name];
    const fn = function () { return apply(native, this, [this]); };
    defineProperty(fn, 'name', { __proto__: null, value: name, configurable: true });
    defineProperty(object, name, { __proto__: null, value: fn, writable: true, enumerable: true, configurable: true });
    return fn;
  };
  const forEachOf = (entries) => function (callback, thisArg) {
    const iterator = apply(entries, this, []);
    if (typeof callback !== 'function') throw new TypeError_('callback must be callable');
    for (const pair of iterator) apply(callback, thisArg, [pair[1], pair[0], this]);
  };
  const formDataPopulated = (proto) => {
    const entries = shim(proto, 'entries');
    shim(proto, 'keys');
    shim(proto, 'values');
    defineProperty(proto, symbolIterator, { __proto__: null, value: entries, writable: true, configurable: true });
    defineProperty(proto, 'forEach', { __proto__: null, value: function (callback, thisArg) {
      if (typeof callback !== 'function') throw new TypeError_('callback must be callable');
      for (const pair of apply(entries, this, [])) apply(callback, thisArg, [pair[1], pair[0], this]);
    }, writable: true, enumerable: true, configurable: true });
  };
  const formDataIteratorPopulated = (iteratorPrototype) => {
    shim(iteratorPrototype, 'next');
    delete iteratorPrototype.constructor;
    setPrototypeOf(iteratorPrototype, iteratorPrototypeOfArrays);
    defineProperty(iteratorPrototype, toStringTag, { __proto__: null, value: 'FormData Iterator', configurable: true });
  };

  // headers: [helpers, the step completing Headers.prototype once its
  // members are installed].
  const headersPopulated = (proto) => {
    const entries = proto.entries;
    defineProperty(proto, symbolIterator, { __proto__: null, value: entries, writable: true, configurable: true });
    defineProperty(proto, 'forEach', { __proto__: null, value: forEachOf(entries), writable: true, enumerable: true, configurable: true });
  };
  // WebIDL default iterator objects: one "Headers Iterator" prototype per
  // realm inheriting %IteratorPrototype%, whose `next` runs the iterator's
  // own native step (kept out of reach of page script).
  const weakMapGet = WeakMap.prototype.get, weakMapSet = WeakMap.prototype.set;
  const headersIteratorSteps = new WeakMap();
  const headersIteratorPrototype = create(iteratorPrototypeOfArrays);
  defineProperty(headersIteratorPrototype, 'next', { __proto__: null, value: {
    next() {
      const step = apply(weakMapGet, headersIteratorSteps, [this]);
      if (step === undefined) throw new TypeError_('next called on an object that is not a Headers Iterator');
      return step();
    },
  }.next, writable: true, enumerable: true, configurable: true });
  defineProperty(headersIteratorPrototype, toStringTag, { __proto__: null, value: 'Headers Iterator', configurable: true });
  const headersHelpers = {
    iterator(next) {
      const iterator = create(headersIteratorPrototype);
      apply(weakMapSet, headersIteratorSteps, [iterator, next]);
      return iterator;
    },
    fill(init, convert) {
      const result = [];
      if ((typeof init !== 'object' && typeof init !== 'function') || init === null)
        throw new TypeError_('Headers init must be an object');
      const method = init[symbolIterator];
      if (method !== undefined && method !== null) {
        if (typeof method !== 'function') throw new TypeError_('Invalid iterator');
        for (const pair of { [symbolIterator]() { return apply(method, init, []); } }) {
          if ((typeof pair !== 'object' && typeof pair !== 'function') || pair === null)
            throw new TypeError_('Header entry must be an iterable pair');
          const values = [];
          for (const value of pair) append(values, convert(value));
          append(result, values);
        }
      } else {
        for (const key of ownKeys(init)) {
          if (getOwnPropertyDescriptor(init, key)?.enumerable) {
            const name = convert(key);
            append(result, [name, convert(init[key])]);
          }
        }
      }
      return result;
    },
  };

  const fontFeatureMap = (proto) => {
    const nextEntry = proto.nextEntry, nativeSet = proto.set;
    const size = getOwnPropertyDescriptor(proto, 'size').get;
    delete proto.nextEntry;
    const define = (name, value) => defineProperty(proto, name, { __proto__: null, value, writable: true, enumerable: true, configurable: true });
    function entries() { return iterator(this, 0); }
    function iterator(map, kind) {
      apply(size, map, []); // Brand check at iterator creation.
      let cursor = 0, done = false;
      return {
        __proto__: iteratorPrototypeOfArrays,
        next() {
          const entry = done ? undefined : apply(nextEntry, map, [cursor]);
          if (entry === undefined) { done = true; return { value: undefined, done }; }
          cursor = entry[0];
          return { value: kind === 1 ? entry[1] : kind === 2 ? entry[2] : [entry[1], entry[2]], done };
        },
      };
    }
    define('set', function set(key, indexes) {
      apply(size, this, []);
      if (arguments.length < 2) throw new TypeError_('CSSFontFeatureValuesMap.set requires two arguments');
      if (typeof key === 'symbol') throw new TypeError_('Cannot convert symbol to string');
      key = String_(key);
      const values = [];
      const method = indexes !== null && (typeof indexes === 'object' || typeof indexes === 'function')
        ? indexes[symbolIterator] : undefined;
      if (method == null) { append(values, indexes >>> 0); }
      else {
        for (const v of { [symbolIterator]() { return apply(method, indexes, []); } }) append(values, v >>> 0);
      }
      return apply(nativeSet, this, [key, values]);
    });
    define('entries', entries);
    define('keys', function keys() { return iterator(this, 1); });
    define('values', function values() { return iterator(this, 2); });
    define('forEach', forEachOf(entries));
    defineProperty(proto, symbolIterator, { __proto__: null, value: entries, writable: true, configurable: true });
  };

  // url_search_params, given the native USVString conversion: [helpers, the
  // steps completing URLSearchParams.prototype and the iterator prototype
  // once their members are installed].
  const urlSearchParams = (convert) => {
    const iteratorPopulated = (iteratorPrototype) => {
      delete iteratorPrototype.constructor;
      setPrototypeOf(iteratorPrototype, iteratorPrototypeOfArrays);
      defineProperty(iteratorPrototype, toStringTag, { __proto__: null, value: 'URLSearchParams Iterator', configurable: true });
    };
    const populated = (proto) => {
      const entries = proto.entries;
      defineProperty(proto, symbolIterator, { __proto__: null, value: entries, writable: true, configurable: true });
      defineProperty(proto, 'forEach', { __proto__: null, value: forEachOf(entries), writable: true, enumerable: true, configurable: true });
    };
    return [{
      result(value, done) { return { value, done }; },
      fill(init) {
        if (init === undefined) return '';
        if (init === null || (typeof init !== 'object' && typeof init !== 'function')) return convert(init);
        const method = init[symbolIterator], result = [];
        if (method !== undefined && method !== null) {
          if (typeof method !== 'function') throw new TypeError_('Invalid iterator');
          for (const pair of { [symbolIterator]() { return apply(method, init, []); } }) {
            if (pair === null || (typeof pair !== 'object' && typeof pair !== 'function')) throw new TypeError_('Expected iterable pair');
            const values = [];
            for (const value of pair) append(values, convert(value));
            if (values.length !== 2) throw new TypeError_('URLSearchParams entry must have two items');
            append(result, values);
          }
        } else {
          const positions = create(null);
          for (const key of ownKeys(init)) {
            if (getOwnPropertyDescriptor(init, key)?.enumerable) {
              const name = convert(key), value = convert(init[key]);
              if (positions[name] === undefined) { positions[name] = result.length; append(result, [name, value]); }
              else result[positions[name]][1] = value;
            }
          }
        }
        return result;
      },
    }, populated, iteratorPopulated];
  };

  // lib.rs: [Unscopable] members of ChildNode and ParentNode, defined with
  // each interface's members (Element, CharacterData, DocumentType,
  // Document, DocumentFragment). Runs when a page first uses the interface:
  // no iteration, no descriptor that could inherit from a changed
  // Object.prototype.
  const unscopable = (names) => (proto) => {
    const o = create(null);
    for (let i = 0; i < names.length; i++) o[names[i]] = true;
    defineProperty(proto, symbolUnscopables, { __proto__: null, value: o, configurable: true });
  };
  const child = ['before', 'after', 'replaceWith', 'remove'], parent = ['prepend', 'append', 'replaceChildren'];

  // file.rs: the `blobParts` sequence of the Blob and File constructors,
  // converted as Web IDL converts `sequence<BlobPart>`: iterated through its
  // @@iterator, each item converted as it comes. Blobs stay objects (the
  // native side reads them), buffer sources become Uint8Arrays over their
  // bytes, anything else a USVString.
  const blobSize = getOwnPropertyDescriptor(Blob.prototype, 'size').get;
  const bufferLength = getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const sharedLength = typeof SharedArrayBuffer === 'function'
    ? getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'byteLength').get : undefined;
  const isView = ArrayBuffer.isView;
  const typedArrayProto = getPrototypeOf(Uint8Array_.prototype);
  const viewBuffer = getOwnPropertyDescriptor(typedArrayProto, 'buffer').get;
  const viewOffset = getOwnPropertyDescriptor(typedArrayProto, 'byteOffset').get;
  const viewLength = getOwnPropertyDescriptor(typedArrayProto, 'byteLength').get;
  const viewName = getOwnPropertyDescriptor(typedArrayProto, toStringTag).get;
  // JSC's public typed-array enum does not cover every ECMAScript view.
  const viewBytes = value => {
    if (!isView(value)) return null;
    if (apply(viewName, value, []) === undefined) return dataView(value);
    const length = apply(viewLength, value, []);
    if (length === 0) return new Uint8Array_(0);
    return new Uint8Array_(apply(viewBuffer, value, []), apply(viewOffset, value, []), length);
  };
  const resizable = getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable')?.get;
  const growable = typeof SharedArrayBuffer === 'function'
    ? getOwnPropertyDescriptor(SharedArrayBuffer.prototype, 'growable')?.get : undefined;
  // Web IDL [AllowShared] Uint8Array, without [AllowResizable].
  const encodeIntoCapacity = view => {
    if (apply(viewName, view, []) !== 'Uint8Array') throw new TypeError_('Expected a Uint8Array');
    const buffer = apply(viewBuffer, view, []);
    let variable = false;
    if (resizable) {
      try { variable = apply(resizable, buffer, []); }
      catch (_) { if (growable) variable = apply(growable, buffer, []); }
    }
    if (variable) throw new TypeError_('Expected a fixed-length backing buffer');
    return apply(viewLength, view, []);
  };
  // WebCodecs uses fixed BufferSources, including shared buffers and views.
  // Use brand-checking intrinsic getters so an overridden buffer or byteLength
  // property cannot change the range copied.
  const fixedByteView = (value, source = false) => {
    let buffer = value;
    const view = isView(value);
    const typed = view && apply(viewName, value, []) !== undefined;
    if (view) buffer = apply(typed ? viewBuffer : dataViewBuffer, value, []);
    let shared = false, length;
    try { length = apply(bufferLength, buffer, []); }
    catch (error) {
      if (!sharedLength) throw error;
      length = apply(sharedLength, buffer, []);
      shared = true;
    }
    const variable = shared ? growable : resizable;
    if (variable && apply(variable, buffer, [])) throw new TypeError_('Expected a fixed-length buffer');
    // Reading the bytes held by a detached input produces the empty byte
    // sequence. A copy destination must remain writable.
    if (source && length === 0) return new Uint8Array_(0);
    if (!view) return new Uint8Array_(buffer);
    const offset = apply(typed ? viewOffset : dataViewOffset, value, []);
    return new Uint8Array_(buffer, offset, apply(typed ? viewLength : dataViewLength, value, []));
  };
  const atomicLoad = typeof Atomics === 'object' ? Atomics.load : undefined;
  // Only private bytes may reach the native decoder's slice copy. A shared
  // source is read through its Uint8Array, one byte at a time, after options
  // conversion. Ordinary Uint8Array element reads are also tear-free.
  const privateDecoderView = view => {
    const buffer = apply(viewBuffer, view, []);
    try {
      apply(bufferLength, buffer, []);
      return view;
    } catch (_) {
      const length = apply(viewLength, view, []);
      const copy = new Uint8Array_(length);
      for (let i = 0; i < length; i++) copy[i] = atomicLoad ? atomicLoad(view, i) : view[i];
      return copy;
    }
  };
  const transferBuffer = ArrayBuffer.prototype.transfer, byteSet = Uint8Array_.prototype.set;
  const Set_ = Set, setAdd = Set.prototype.add, setHas = Set.prototype.has;
  const CloneException = DOMException;
  const chunkTransferList = value => {
    const out = [];
    if (value !== undefined) {
      if (value === null || (typeof value !== 'object' && typeof value !== 'function'))
        throw new TypeError_('Chunk transfer list must be a sequence');
      for (const entry of value) {
        apply(bufferLength, entry, []);
        if (resizable && apply(resizable, entry, [])) throw new TypeError_('Expected a fixed-length transfer buffer');
        append(out, entry);
      }
    }
    return out;
  };
  const copyChunkInput = (bytes, transfers, size) => {
    const seen = new Set_();
    for (let i = 0; i < transfers.length; i++) {
      const entry = transfers[i];
      if (apply(setHas, seen, [entry])) throw new CloneException('Duplicate transfer', 'DataCloneError');
      apply(setAdd, seen, [entry]);
      try { new Uint8Array_(entry, 0, 0); }
      catch (_) { throw new CloneException('Detached transfer', 'DataCloneError'); }
    }
    // Take a snapshot, including for shared data, before detaching any input.
    const input = size === undefined ? bytes :
        new Uint8Array_(apply(viewBuffer, bytes, []), apply(viewOffset, bytes, []), size);
    const snapshot = new Uint8Array_(input);
    for (let i = 0; i < transfers.length; i++) apply(transferBuffer, transfers[i], [0]);
    return snapshot;
  };
  const copyChunkTo = (dst, src) => {
    if (apply(viewLength, src, []) > apply(viewLength, dst, [])) throw new TypeError_('Chunk destination is too small');
    apply(byteSet, dst, [src]);
  };
  const toWellFormed = String.prototype.toWellFormed;
  const blobParts = (parts) => {
    const out = [];
    if (parts === undefined) return out;
    if (parts === null || (typeof parts !== 'object' && typeof parts !== 'function'))
      throw new TypeError_("Failed to construct 'Blob': The provided value cannot be converted to a sequence.");
    const method = parts[symbolIterator];
    if (typeof method !== 'function')
      throw new TypeError_("Failed to construct 'Blob': The object must have a callable @@iterator property.");
    const it = apply(method, parts, []);
    for (;;) {
      const step = it.next();
      if (step.done) break;
      const v = step.value;
      if (v !== null && typeof v === 'object') {
        try { apply(blobSize, v, []); append(out, v); continue; } catch (_) {}
        // A buffer source over a detached buffer holds no bytes.
        const view = viewBytes(v);
        if (view !== null) { append(out, view); continue; }
        let isBuffer = false;
        try { apply(bufferLength, v, []); isBuffer = true; } catch (_) {}
        if (!isBuffer && sharedLength) try { apply(sharedLength, v, []); isBuffer = true; } catch (_) {}
        if (isBuffer) {
          let bytes;
          try { bytes = new Uint8Array_(v); } catch (_) { bytes = new Uint8Array_(0); }
          append(out, bytes); continue;
        }
      }
      append(out, apply(toWellFormed, String_(v), []));
    }
    return out;
  };

  // pyro-browser's Location: every member is [LegacyUnforgeable] (HTML), an
  // own, non-configurable property of each Location object and none of
  // Location.prototype, plus own `valueOf` (%Object.prototype.valueOf%) and
  // `@@toPrimitive` (undefined). `[hook, install]`: `hook` (the class's
  // populate hook) moves the members off the prototype, `install` defines
  // them on a new Location object.
  const locationUnforgeables = () => {
    const members = [];
    const hook = (proto) => {
      const keys = ownKeys(proto);
      for (let i = 0; i < keys.length; i++) {
        const key = keys[i];
        if (key === 'constructor' || key === toStringTag) continue;
        const d = getOwnPropertyDescriptor(proto, key);
        setPrototypeOf(d, null);
        d.configurable = false;
        if (d.get === undefined && d.set === undefined) d.writable = false;
        delete proto[key];
        append(members, key);
        append(members, d);
      }
      append(members, 'valueOf');
      append(members, { __proto__: null, value: objectValueOf, writable: false, enumerable: false, configurable: false });
      append(members, symbolToPrimitive);
      append(members, { __proto__: null, value: undefined, writable: false, enumerable: false, configurable: false });
    };
    const install = (location) => {
      for (let i = 0; i < members.length; i += 2) defineProperty(location, members[i], members[i + 1]);
    };
    return [hook, install];
  };

  // fetch.rs `install_wasm_streaming`: WebAssembly.compileStreaming and
  // instantiateStreaming (WebAssembly Web API), which JavaScriptCore leaves to
  // its host (JSAPIGlobalObject's method table has no compileStreaming hook,
  // so the namespace lacks both). `read(response, method)` checks the
  // Response (MIME type, CORS-same-origin, ok status) and consumes its body as
  // an ArrayBuffer (response.rs `read_for_wasm`); the bytes then go through
  // the realm's own WebAssembly.compile and instantiate, captured here.
  //
  // CSP3 "EnsureCSPDoesNotBlockWasmByteCompilation" (csp.rs): `allowed()` (the
  // native cspWasm) runs the realm's wasm-eval check and reports its
  // violations. The returned `arm` (csp.rs `sync_wasm_gate`, once the realm's
  // policies lack 'wasm-unsafe-eval' and 'unsafe-eval') replaces compile,
  // instantiate and the Module constructor with gated ones, the property
  // descriptors kept; a realm without such a policy never sees a wrapper.
  // Armed, compiling bytes throws (rejects with) a CompileError before
  // anything else (as V8: before the argument is read, the streaming ones
  // before the source is), and instantiate with a compiled Module passes.
  const wasmStreaming = (read, allowed) => {
    const wasm = typeof WebAssembly === 'object' && WebAssembly !== null ? WebAssembly : undefined;
    if (wasm === undefined) return undefined;
    const compile = wasm.compile, instantiate = wasm.instantiate;
    const Module = wasm.Module, CompileError = wasm.CompileError;
    const moduleExports = typeof Module === 'function' ? Module.exports : undefined;
    const Promise_ = Promise, then = Promise.prototype.then, construct = Reflect.construct;
    let armed = false;
    const refusal = (method) => new CompileError(
      `${method}: Refused to compile or instantiate WebAssembly module because neither 'wasm-unsafe-eval' nor 'unsafe-eval' is an allowed source of script in the Content Security Policy`);
    const rejection = (method) => {
      const error = refusal(method);
      return new Promise_((_, reject) => reject(error));
    };
    const blocked = () => armed && !allowed();
    // A WebAssembly.Module (Module.exports is its brand check).
    const isModule = (value) => {
      if (moduleExports === undefined || value === null || typeof value !== 'object') return false;
      try { apply(moduleExports, Module, [value]); return true; } catch (_) { return false; }
    };

    if (!('compileStreaming' in wasm)) {
      // Spec: "a promise resolved with source", then the response's bytes.
      const bytesOf = (source, method) =>
        apply(then, new Promise_(resolve => resolve(source)), [response => read(response, method)]);
      const methods = {
        compileStreaming(source, options) {
          if (blocked()) return rejection('WebAssembly.compileStreaming()');
          return apply(then, bytesOf(source, 'compileStreaming'),
            [bytes => apply(compile, wasm, options === undefined ? [bytes] : [bytes, options])]);
        },
        instantiateStreaming(source, importObject, options) {
          if (blocked()) return rejection('WebAssembly.instantiateStreaming()');
          if (importObject !== undefined && (importObject === null || (typeof importObject !== 'object' && typeof importObject !== 'function')))
            return new Promise_((_, reject) => reject(new TypeError_("Failed to execute 'instantiateStreaming' on 'WebAssembly': Argument 1 must be an object")));
          return apply(then, bytesOf(source, 'instantiateStreaming'),
            [bytes => apply(instantiate, wasm, options === undefined ? [bytes, importObject] : [bytes, importObject, options])]);
        },
      };
      for (const name of ['compileStreaming', 'instantiateStreaming']) {
        const method = methods[name];
        defineProperty(method, 'length', { value: 1, configurable: true });
        defineProperty(wasm, name, { value: method, writable: true, enumerable: true, configurable: true });
      }
    }

    // `replacement` in `object[name]`'s place, with its name and length and
    // the descriptor of the property it replaces.
    const replace = (object, name, original, replacement) => {
      const own = getOwnPropertyDescriptor(object, name);
      if (own === undefined || own.value !== original) return false;
      defineProperty(replacement, 'length', { __proto__: null, value: original.length, configurable: true });
      defineProperty(replacement, 'name', { __proto__: null, value: original.name, configurable: true });
      defineProperty(object, name, { __proto__: null, value: replacement, writable: own.writable, enumerable: own.enumerable, configurable: own.configurable });
      return true;
    };
    return () => {
      if (armed) return;
      armed = true;
      if (typeof compile === 'function') {
        replace(wasm, 'compile', compile, {
          compile() {
            if (blocked()) return rejection('WebAssembly.compile()');
            return apply(compile, this, arguments);
          },
        }.compile);
      }
      if (typeof instantiate === 'function') {
        replace(wasm, 'instantiate', instantiate, {
          instantiate(source) {
            if (!isModule(source) && blocked()) return rejection('WebAssembly.instantiate()');
            return apply(instantiate, this, arguments);
          },
        }.instantiate);
      }
      if (typeof Module === 'function') {
        const prototype = Module.prototype;
        const Gated = function (bytes) {
          'use strict';
          // Called without new: the native's TypeError.
          if (new.target === undefined) return apply(Module, this, arguments);
          if (blocked()) throw refusal('WebAssembly.Module()');
          return construct(Module, arguments, new.target);
        };
        // The statics (exports, imports, customSections) and prototype, as
        // the native has them, so instanceof and the IDL checks hold.
        for (const key of ownKeys(Module)) {
          if (key === 'length' || key === 'name') continue;
          const descriptor = getOwnPropertyDescriptor(Module, key);
          descriptor.__proto__ = null;
          defineProperty(Gated, key, descriptor);
        }
        if (replace(wasm, 'Module', Module, Gated) && prototype !== null && typeof prototype === 'object') {
          const own = getOwnPropertyDescriptor(prototype, 'constructor');
          if (own !== undefined && own.value === Module)
            defineProperty(prototype, 'constructor', { __proto__: null, value: Gated, writable: own.writable, enumerable: own.enumerable, configurable: own.configurable });
        }
      }
    };
  };

  return {
    blobParts,
    encoding: { __proto__: null, viewBytes, encodeIntoCapacity, fixedByteView, privateDecoderView, chunkTransferList, copyChunkInput, copyChunkTo, webIdlNumber: value => +value },
    mutationOptions,
    intersection: [intersectionThresholds, freeze],
    formData: [formDataPopulated, formDataIteratorPopulated],
    headers: [headersHelpers, headersPopulated],
    urlSearchParams,
    fontFeatureMap,
    mediaSourceStatics(_prototype, constructor) {
      const descriptor = getOwnPropertyDescriptor({ get canConstructInDedicatedWorker() { return false; } }, 'canConstructInDedicatedWorker');
      defineProperty(constructor, 'canConstructInDedicatedWorker', descriptor);
    },
    structuredClone: structuredCloneFactory,
    wasmStreaming,
    // Element's own [Unscopable] `slot` (DOM) joins ChildNode's and ParentNode's.
    unscopables: [unscopable([...child, ...parent, 'slot']), unscopable(child), unscopable(child), unscopable([...parent, 'fullscreen']), unscopable(parent)],
    // An enumerable, configurable getter on the global (pyro-browser's
    // `window[0]`.. frame indices).
    accessor: (global, key, get) => defineProperty(global, key, { get, enumerable: true, configurable: true }),
    locationUnforgeables,
  };
}
