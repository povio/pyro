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
  const headersHelpers = {
    iterator(next) { return { next, [symbolIterator]() { return this; } }; },
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
        if (isView(v)) {
          let bytes;
          try { bytes = new Uint8Array_(apply(viewBuffer, v, []), apply(viewOffset, v, []), apply(viewLength, v, [])); }
          catch (_) { bytes = dataView(v); }
          append(out, bytes || new Uint8Array_(0)); continue;
        }
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

  return {
    blobParts,
    dataView,
    mutationOptions,
    intersection: [intersectionThresholds, freeze],
    formData: [formDataPopulated, formDataIteratorPopulated],
    headers: [headersHelpers, headersPopulated],
    urlSearchParams,
    structuredClone: structuredCloneFactory,
    // Element's own [Unscopable] `slot` (DOM) joins ChildNode's and ParentNode's.
    unscopables: [unscopable([...child, ...parent, 'slot']), unscopable(child), unscopable(child), unscopable([...parent, 'fullscreen']), unscopable(parent)],
    // An enumerable, configurable getter on the global (pyro-browser's
    // `window[0]`.. frame indices).
    accessor: (global, key, get) => defineProperty(global, key, { get, enumerable: true, configurable: true }),
  };
}
