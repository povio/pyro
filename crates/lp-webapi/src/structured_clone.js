// `guard(value)` is native: whether `value` is an ordinary object. With a
// mode it is `host(value, mode, extra)`: mode 0 clones a serializable host
// object (Blob, File, FileList) into this realm or returns undefined; mode 1
// prepares the transfer of a transferable one (a MessagePort: its new port
// here, undefined if the value is not transferable, DataCloneError if it is
// detached); mode 2 completes the transfer prepared for `value` as `extra`;
// mode 3 copies a platform object made in JS (CryptoKey) or returns undefined;
// mode 4 is unused here; mode 5 prepares the transfer of a stream ([its new
// stream here, the new ports it reads or writes], undefined if the value is
// not a stream, DataCloneError if it is locked) and mode 6 completes it (the
// original is piped through a channel whose other ports become `extra`'s).
((guard, fail, DOMExceptionC) => {
    const host = (value, mode, extra) => guard(value, mode, extra);
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
    // transferred streams' new streams.
    function clone(value, options, ports, keep, streams) {
        const memory = new M(), buffers = [], moved = [];
        let piped;
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
                for (let i = 0; i < list.length; ++i) {
                    const entry = list[i];
                    if (call(mapHas, listed, [entry])) fail('An object is listed twice in the transfer list');
                    call(mapSet, listed, [entry, true]);
                    let length = -1;
                    try { length = call(abLength, entry); } catch (_) {}
                    if (length >= 0) {
                        try { new U8(entry); } catch (_) { fail('An ArrayBuffer is detached and could not be transferred'); }
                        if (!abTransfer) fail('ArrayBuffer transfer is not available', true);
                        put(buffers, buffers.length, entry);
                        continue;
                    }
                    const ordinary = guard(entry);
                    const port = ordinary ? undefined : host(entry, 1);
                    if (port === undefined) {
                        // A stream: its new stream, over new ports that the
                        // original is piped into once the whole value cloned.
                        const stream = ordinary ? host(entry, 5) : undefined;
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
                const copy = host(source, 0);
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
                    const buffer = copy(call(typedBuffer, source), depth + 1);
                    result = new typed[name](buffer, call(typedOffset, source), call(typedLength, source));
                } else {
                    let buffer, offset, length;
                    try {
                        buffer = call(dvBuffer, source); offset = call(dvOffset, source); length = call(dvLength, source);
                    } catch (_) { fail('Invalid DataView'); }
                    result = new DV(copy(buffer, depth + 1), offset, length);
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
                const name = call(stringSlice, kind, [8, -1]);
                if (descriptor(boxed, name)) return remember(source, O(call(boxed[name], source)));
                if (kind !== '[object Object]') {
                    const platformCopy = host(source, 3);
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
        for (let i = 0; i < buffers.length; ++i) call(abTransfer, buffers[i]);
        for (let i = 0; i < moved.length; i += 2) {
            host(moved[i], 2, moved[i + 1]);
            if (ports) put(ports, ports.length, moved[i + 1]);
        }
        if (piped) for (let i = 0; i < piped.length; i += 2) {
            host(piped[i], 6, piped[i + 1]);
            if (streams) put(streams, streams.length, piped[i + 1][0]);
        }
        return result;
    }
    function structuredClone(value, options = undefined) {
        if (!arguments.length) throw new TE('structuredClone requires a value');
        return clone(value, options, undefined);
    }
    return [structuredClone, clone];
})
