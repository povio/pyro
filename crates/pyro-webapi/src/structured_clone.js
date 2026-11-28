// `guard(value)` is native: whether `value` is an ordinary object. With a
// mode it is `host(value, mode, extra)`: mode 0 clones a serializable host
// object (Blob, File, FileList, encoded media chunk or AudioData) into this realm
// or returns undefined; extra signals serialization for storage. Mode 1
// prepares the transfer of a transferable one (a MessagePort: its new port
// here, undefined if the value is not transferable, DataCloneError if it is
// detached); mode 2 completes the transfer prepared for `value` as `extra`;
// mode 3 copies a platform object made in JS (CryptoKey, which is cluster-bound:
// a receiver that is not a secure context refuses it) or returns undefined;
// mode 4 is unused here; mode 5 prepares the transfer of a stream ([its new
// stream here, the new ports it reads or writes], undefined if the value is
// not a stream, DataCloneError if it is locked) and mode 6 completes it (the
// original is piped through a channel whose other ports become `extra`'s);
// mode 8 prepares the transfer of an ImageBitmap or OffscreenCanvas (its copy
// here, undefined for anything else, DataCloneError if it is detached) and
// mode 9 detaches the original. Mode 3 also copies an ImageBitmap.
// Mode 12 prepares transfer of AudioData, mode 13 revalidates that it is
// still open, and mode 14 closes the sender after successful serialization.
((guard, fail, DOMExceptionC) => {
    const host = (value, mode, extra, attachment) => guard(value, mode, extra, attachment);
    const O = Object, A = Array, M = Map, S = Set, D = Date, R = RegExp, TE = TypeError;
    const AB = ArrayBuffer, U8 = Uint8Array, DV = DataView;
    const errors = {Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError};
    const apply = Reflect.apply, keys = O.keys, descriptor = O.getOwnPropertyDescriptor;
    const prototype = O.getPrototypeOf, define = O.defineProperty, create = O.create;
    const tag = Symbol.toStringTag, iteratorKey = Symbol.iterator, objectTag = O.prototype.toString;
    const stringSlice = String.prototype.slice;
    const arrayIsArray = A.isArray, isView = AB.isView;
    const mapGet = M.prototype.get, mapSet = M.prototype.set, mapHas = M.prototype.has;
    const mapEntries = M.prototype.entries, mapNext = prototype(new M().entries()).next;
    const setValues = S.prototype.values, setNext = prototype(new S().values()).next, setAdd = S.prototype.add;
    const dateTime = D.prototype.getTime, regexpSource = descriptor(R.prototype, 'source').get;
    const flags = ['hasIndices','global','ignoreCase','multiline','dotAll','unicode','unicodeSets','sticky'];
    const flagLetters = ['d','g','i','m','s','u','v','y'];
    const flagGetters = flags.map(name => descriptor(R.prototype, name)?.get);
    const getter = (proto, key) => descriptor(proto, key)?.get;
    const abLength = getter(AB.prototype, 'byteLength'), abResizable = getter(AB.prototype, 'resizable');
    const abMax = getter(AB.prototype, 'maxByteLength'), abTransfer = AB.prototype.transfer;
    const domName = getter(DOMExceptionC.prototype, 'name'), domMessage = getter(DOMExceptionC.prototype, 'message');
    const dvBuffer = getter(DV.prototype, 'buffer'), dvOffset = getter(DV.prototype, 'byteOffset');
    const dvLength = getter(DV.prototype, 'byteLength');
    const typedProto = prototype(U8.prototype), typedName = getter(typedProto, tag);
    const typedBuffer = getter(typedProto, 'buffer'), typedOffset = getter(typedProto, 'byteOffset');
    const typedLength = getter(typedProto, 'length'), typedValues = typedProto.values;
    const byteSet = U8.prototype.set;
    const typed = {Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
        Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array};
    if (typeof Float16Array === 'function') typed.Float16Array = Float16Array;
    const boxed = {Boolean: Boolean.prototype.valueOf, Number: Number.prototype.valueOf,
        String: String.prototype.valueOf, BigInt: BigInt.prototype.valueOf};
    const call = (fn, self, args = []) => apply(fn, self, args);
    // A view whose length tracks its resizable buffer's (built without a length) serializes so.
    // ECMAScript has no getter for that: a view ending at its buffer's end is probed by growing
    // the buffer by up to 8 bytes and back (at its maximum, it counts as tracking).
    const abResize = AB.prototype.resize, typedByteLength = getter(typedProto, 'byteLength');
    function tracksLength(view, buffer, offset, viewBytes) {
        if (!call(abResizable, buffer)) return false;
        const length = call(abLength, buffer), before = call(viewBytes, view);
        const room = call(abMax, buffer) - length;
        if (length - offset - before >= 8 || length - offset - before < 0) return false;
        if (room <= 0) return true;
        call(abResize, buffer, [length + (room < 8 ? room : 8)]);
        const grew = call(viewBytes, view) !== before;
        call(abResize, buffer, [length]);
        return grew;
    }

    const put = (object, key, value) => define(object, key,
        {value, enumerable:true, writable:true, configurable:true});
    const hide = (object, key, value) => define(object, key, {value, writable:true, configurable:true});
    const own = (object, key) => { const d = descriptor(object, key); return d && descriptor(d, 'value') ? d : undefined; };
    // Data properties an Error keeps (its own `stack`, as Chrome does).
    function errorParts(source, result) {
        const stack = own(source, 'stack');
        if (stack && typeof stack.value === 'string') hide(result, 'stack', stack.value);
        return result;
    }
    // `ports` (internal callers) receives the transferred MessagePorts' new ports.
    // `keep` (worker_wire.js) lists MessagePorts and streams, already
    // validated, that stay themselves in the copy: the cross-VM format
    // transfers them afterwards. `streams` (internal callers) receives the
    // transferred streams' new streams, `resources` the transferred
    // canvas objects' and AudioData frames' copies.
    // A VideoFrame's copy (`new VideoFrame(frame)` shares the frame's resource); DataCloneError
    // for a closed frame.
    function videoFrameCopy(frame) {
        const VF = globalThis.VideoFrame;
        if (typeof VF !== 'function') fail('Object type is not supported');
        try { return new VF(frame); } catch (_) { fail('A VideoFrame is closed and could not be cloned'); }
    }
    function clone(value, options, ports, keep, streams, resources, forStorage = false, clusterBound) {
        const memory = new M(), buffers = [], moved = [];
        let piped, detached, audio, deferred, closing;
        if (keep) for (let i = 0; i < keep.length; ++i) call(mapSet, memory, [keep[i], keep[i]]);
        if (options != null) {
            if (typeof options !== 'object' && typeof options !== 'function')
                throw new TE('structuredClone options must be an object');
            const transfer = options.transfer;
            if (transfer !== undefined) {
                if (transfer == null || typeof transfer[iteratorKey] !== 'function')
                    throw new TE('transfer must be iterable');
                const list = [];
                for (const entry of transfer) {
                    if (entry === null || (typeof entry !== 'object' && typeof entry !== 'function'))
                        throw new TE('transfer entries must be objects');
                    put(list, list.length, entry);
                }
                // Nothing is detached unless the whole value clones.
                const listed = new M();
                // HTML checks that no listed object is detached only once the value is serialized:
                // a getter's exception comes first. A DataCloneError of an already detached entry
                // waits until then.
                const later = (f) => {
                    try { return f(); } catch (e) {
                        if (!e || e.name !== 'DataCloneError') throw e;
                        if (!deferred) deferred = e;
                        return null;
                    }
                };
                for (let i = 0; i < list.length; ++i) {
                    const entry = list[i];
                    if (call(mapHas, listed, [entry])) fail('An object is listed twice in the transfer list');
                    call(mapSet, listed, [entry, true]);
                    let length = -1;
                    try { length = call(abLength, entry); } catch (_) {}
                    if (length >= 0) {
                        let live = true;
                        try { new U8(entry); } catch (_) { live = false; }
                        if (!live) {
                            later(() => fail('An ArrayBuffer is detached and could not be transferred'));
                            continue;
                        }
                        if (!abTransfer) fail('ArrayBuffer transfer is not available', true);
                        put(buffers, buffers.length, entry);
                        continue;
                    }
                    const ordinary = guard(entry);
                    const port = ordinary ? undefined : later(() => host(entry, 1));
                    if (port === null) continue;
                    if (port === undefined) {
                        if (!ordinary) {
                            const frame = host(entry, 12, clusterBound);
                            if (frame === undefined) fail('Value is not transferable');
                            call(mapSet, memory, [entry, frame]);
                            if (!audio) audio = [];
                            put(audio, audio.length, entry);
                            put(audio, audio.length, frame);
                            continue;
                        }
                        // A VideoFrame (video_frame.js): a frame sharing its resource, the
                        // original closed once the whole value cloned.
                        if (call(objectTag, entry) === '[object VideoFrame]') {
                            const frame = later(() => videoFrameCopy(entry));
                            if (frame === null) continue;
                            call(mapSet, memory, [entry, frame]);
                            if (!closing) closing = [];
                            put(closing, closing.length, entry);
                            continue;
                        }
                        // An ImageBitmap or OffscreenCanvas: its copy, the
                        // original detached once the whole value cloned.
                        const canvas = later(() => host(entry, 8));
                        if (canvas === null) continue;
                        if (canvas !== undefined) {
                            call(mapSet, memory, [entry, canvas]);
                            if (!detached) detached = [];
                            put(detached, detached.length, entry);
                            put(detached, detached.length, canvas);
                            continue;
                        }
                        // A stream: its new stream, over new ports that the
                        // original is piped into once the whole value cloned.
                        const stream = host(entry, 5);
                        if (stream === undefined) fail('Value is not transferable');
                        call(mapSet, memory, [entry, stream[0]]);
                        if (!piped) piped = [];
                        put(piped, piped.length, entry);
                        put(piped, piped.length, stream);
                        continue;
                    }
                    call(mapSet, memory, [entry, port]);
                    put(moved, moved.length, entry);
                    put(moved, moved.length, port);
                }
            }
        }
        let nodes = 0, bytes = 0;
        const remember = (source, copy) => { call(mapSet, memory, [source, copy]); return copy; };
        function copy(source, depth) {
            const type = typeof source;
            if (type === 'symbol' || type === 'function') fail('Value is not cloneable');
            if (source === null || type !== 'object') return source;
            if (call(mapHas, memory, [source])) return call(mapGet, memory, [source]);
            if (!guard(source)) {
                const copy = host(source, 0, forStorage, clusterBound);
                if (copy === undefined) fail('Proxy and host objects are not cloneable');
                return remember(source, copy);
            }
            let array;
            try { array = arrayIsArray(source); } catch (_) { fail('Revoked proxy is not cloneable'); }
            if (++nodes > 65536 || depth > 512) fail('Clone graph limit exceeded', true);
            let result;
            if (array) {
                result = remember(source, new A(source.length));
            } else if (isView(source)) {
                const name = call(typedName, source);
                if (name !== undefined) {
                    try { call(typedValues, source); } catch (_) { fail('Invalid typed array'); }
                    const original = call(typedBuffer, source), offset = call(typedOffset, source);
                    const length = call(typedLength, source);
                    // Before the buffer is copied (or transferred and detached).
                    const tracking = tracksLength(source, original, offset, typedByteLength);
                    const buffer = copy(original, depth + 1);
                    result = tracking ? new typed[name](buffer, offset) : new typed[name](buffer, offset, length);
                } else {
                    let buffer, offset, length;
                    try {
                        buffer = call(dvBuffer, source); offset = call(dvOffset, source); length = call(dvLength, source);
                    } catch (_) { fail('Invalid DataView'); }
                    const tracking = tracksLength(source, buffer, offset, dvLength);
                    result = tracking ? new DV(copy(buffer, depth + 1), offset) : new DV(copy(buffer, depth + 1), offset, length);
                }
                return remember(source, result);
            } else {
                // Do not run Symbol.toStringTag getters or proxy prototype traps.
                // Ordinary records with custom tags are outside this bounded subset.
                let prototypeDepth = 0;
                for (let p = source; p !== null; p = prototype(p)) {
                    if (++prototypeDepth > 512) fail('Clone prototype depth limit exceeded', true);
                    if (!guard(p)) fail('Proxy or host prototype is not supported', true);
                    const ownTag = descriptor(p, tag);
                    if (ownTag && (p === source || descriptor(ownTag, 'get') || descriptor(ownTag, 'set')))
                        fail('Custom object tags are not supported', true);
                }
                const kind = call(objectTag, source);
                // Modules and shared buffers cannot be serialized for storage.
                // Apply this at the point of traversal, before any general clone support.
                if (forStorage && (kind === '[object WebAssembly.Module]' || kind === '[object SharedArrayBuffer]'))
                    fail('Object cannot be serialized for storage');
                if (kind === '[object Date]') return remember(source, new D(call(dateTime, source)));
                if (kind === '[object RegExp]') {
                    let text = '';
                    for (let i=0; i<flags.length; ++i)
                        if (flagGetters[i] && call(flagGetters[i], source)) text += flagLetters[i];
                    return remember(source, new R(call(regexpSource, source), text));
                }
                if (kind === '[object Error]') {
                    // HTML: the name if it is a standard one, the own data
                    // `message`, and (as Chrome) the own data `cause`.
                    const name = source.name, message = own(source, 'message');
                    const C = typeof name === 'string' && descriptor(errors, name) ? errors[name] : Error;
                    result = remember(source, message ? new C(`${message.value}`) : new C());
                    const cause = own(source, 'cause');
                    if (cause) hide(result, 'cause', copy(cause.value, depth + 1));
                    return errorParts(source, result);
                }
                if (kind === '[object DOMException]') {
                    let name, message;
                    try { name = call(domName, source); message = call(domMessage, source); }
                    catch (_) { fail('Object type is not supported'); }
                    return errorParts(source, remember(source, new DOMExceptionC(message, name)));
                }
                // A QuotaExceededError (a DOMException subclass) is serializable with its message,
                // quota and requested (HTML; WPT structuredclone_0).
                if (kind === '[object QuotaExceededError]') {
                    const QEE = globalThis.QuotaExceededError;
                    let message, quota, requested;
                    try {
                        message = call(domMessage, source);
                        quota = call(descriptor(QEE.prototype, 'quota').get, source);
                        requested = call(descriptor(QEE.prototype, 'requested').get, source);
                    } catch (_) { fail('Object type is not supported'); }
                    const options = {};
                    if (quota !== null) options.quota = quota;
                    if (requested !== null) options.requested = requested;
                    return errorParts(source, remember(source, new QEE(message, options)));
                }
                if (kind === '[object ArrayBuffer]') {
                    let length, view;
                    try { length = call(abLength, source); view = new U8(source); }
                    catch (_) { fail('Detached or invalid ArrayBuffer'); }
                    bytes += length;
                    if (bytes > 64 * 1024 * 1024) fail('Clone buffer limit exceeded', true);
                    result = abResizable && call(abResizable, source)
                        ? new AB(length, {maxByteLength: call(abMax, source)}) : new AB(length);
                    call(byteSet, new U8(result), [view]);
                    return remember(source, result);
                }
                if (kind === '[object Map]' || kind === '[object Set]') {
                    const map = kind === '[object Map]';
                    result = remember(source, map ? new M() : new S());
                    // Snapshot entries before user getters in descendants can mutate the source.
                    const entries = [], iterator = call(map ? mapEntries : setValues, source);
                    for (;;) {
                        const next = call(map ? mapNext : setNext, iterator);
                        if (next.done) break;
                        if (entries.length >= 65536) fail('Clone collection limit exceeded', true);
                        put(entries, entries.length, next.value);
                    }
                    for (let i=0; i<entries.length; ++i) {
                        const entry = entries[i];
                        if (map) call(mapSet, result, [copy(entry[0], depth+1), copy(entry[1], depth+1)]);
                        else call(setAdd, result, [copy(entry, depth+1)]);
                    }
                    return result;
                }
                if (kind === '[object DOMPoint]' || kind === '[object DOMPointReadOnly]' ||
                    kind === '[object DOMRect]' || kind === '[object DOMRectReadOnly]' ||
                    kind === '[object DOMMatrix]' || kind === '[object DOMMatrixReadOnly]' || kind === '[object DOMQuad]') {
                    const parts = host(source, 10);
                    if (!parts) fail('Object type is not supported');
                    // Quad points participate in the outer graph's identity map.
                    const slots = parts[0] === 'DOMQuad' ? copy(parts[1], depth + 1) : parts[1];
                    return remember(source, host(parts[0], 11, slots));
                }
                // A VideoFrame is serializable (not for storage): a frame sharing its resource.
                if (kind === '[object VideoFrame]') {
                    if (forStorage) fail('Object cannot be serialized for storage');
                    return remember(source, videoFrameCopy(source));
                }
                if (kind === '[object ImageData]') {
                    // Serializable: width, height, color space and a copy of the pixels, read
                    // through the interface's own getters (the realm's ImageData).
                    const IDC = globalThis.ImageData, P = IDC && IDC.prototype;
                    let width, height, data, colorSpace, pixelFormat;
                    const read = (proto) => {
                        const get = (k) => call(descriptor(proto, k).get, source);
                        width = get('width'); height = get('height'); data = get('data'); colorSpace = get('colorSpace'); pixelFormat = get('pixelFormat');
                    };
                    try { read(P); }
                    catch (_) {
                        // An ImageData of another realm (a parent's, cloned into an iframe by its
                        // history.pushState): this realm's getters reject it (illegal invocation),
                        // so use the ImageData prototype the object itself inherits.
                        try {
                            let proto = prototype(source);
                            while (proto !== null && !descriptor(proto, 'width')?.get) proto = prototype(proto);
                            if (proto === null) throw 0;
                            read(proto);
                        } catch (_) { fail('Object type is not supported'); }
                    }
                    bytes += data.length;
                    if (bytes > 64 * 1024 * 1024) fail('Clone buffer limit exceeded', true);
                    return remember(source, new IDC(copy(data, depth + 1), width, height, {colorSpace, pixelFormat}));
                }
                const name = call(stringSlice, kind, [8, -1]);
                if (descriptor(boxed, name)) return remember(source, O(call(boxed[name], source)));
                if (kind !== '[object Object]') {
                    const platformCopy = host(source, 3, forStorage, clusterBound);
                    if (platformCopy !== undefined) return remember(source, platformCopy);
                    fail('Object type is not supported');
                }
                result = remember(source, create(O.prototype));
            }
            const names = keys(source);
            if (names.length > 65536) fail('Clone property limit exceeded', true);
            for (let i=0; i<names.length; ++i) {
                const key = names[i], property = descriptor(source, key);
                if (property && property.enumerable) put(result, key, copy(source[key], depth+1));
            }
            return result;
        }
        const result = copy(value, 0);
        if (deferred) throw deferred;
        // A getter may have detached a listed buffer, ImageBitmap or OffscreenCanvas meanwhile.
        for (let i = 0; i < buffers.length; ++i) {
            try { new U8(buffers[i]); } catch (_) { fail('An ArrayBuffer is detached and could not be transferred'); }
        }
        if (detached) for (let i = 0; i < detached.length; i += 2) {
            if (!host(detached[i], 17)) fail('The object is detached and could not be transferred');
        }
        // Transferred VideoFrames are closed: their copies hold the resource.
        if (closing) for (let i = 0; i < closing.length; ++i) {
            const close = globalThis.VideoFrame && globalThis.VideoFrame.prototype.close;
            if (close) call(close, closing[i]);
        }
        // A getter can close a listed frame during serialization.
        if (audio) for (let i = 0; i < audio.length; i += 2) host(audio[i], 13);
        for (let i = 0; i < buffers.length; ++i) call(abTransfer, buffers[i]);
        for (let i = 0; i < moved.length; i += 2) {
            host(moved[i], 2, moved[i + 1]);
            if (ports) put(ports, ports.length, moved[i + 1]);
        }
        if (piped) for (let i = 0; i < piped.length; i += 2) {
            host(piped[i], 6, piped[i + 1]);
            if (streams) put(streams, streams.length, piped[i + 1][0]);
        }
        if (detached) for (let i = 0; i < detached.length; i += 2) {
            host(detached[i], 9);
            if (resources) put(resources, resources.length, detached[i + 1]);
        }
        if (audio) for (let i = 0; i < audio.length; i += 2) {
            host(audio[i], 14);
            if (resources) put(resources, resources.length, audio[i + 1]);
        }
        return result;
    }
    function structuredClone(value, options = undefined) {
        if (!arguments.length) throw new TE('structuredClone requires a value');
        return clone(value, options, undefined);
    }
    return [structuredClone, clone];
})
