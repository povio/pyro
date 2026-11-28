// Cross-VM message format of dedicated workers. Compiled on a realm's first
// worker use, so pages without workers never parse it.
//
// `encode` validates and copies with the realm's original `structuredClone`,
// then walks that copy, a graph of plain data the page cannot intercept, into
// JSON text plus the bytes of its ArrayBuffers. Lone surrogates stay escaped in
// the text. Only strings go through JSON.stringify, so a `toJSON` on a
// prototype cannot affect the output.
//
// `host` is native: `host(value, 0)` describes a Blob or File (['F', bytes
// index, type, name or null, lastModified]), a FileList (['L', ...files]) or
// a CryptoKey (['K', [type, extractable, algorithm, usages, key bytes]]),
// else undefined; `host(tree, 1)` makes the Blob or File of an 'F' tree,
// `host(files, 2)` the FileList of decoded files and `host(parts, 3)` the
// CryptoKey of decoded key parts; `host(value, 4)` says whether `value` is a
// MessagePort that can be transferred (DataCloneError if it was already);
// `host(value, 5)` is 'R', 'W' or 'T' for a ReadableStream, WritableStream or
// TransformStream of this realm (DataCloneError if locked), `host(stream, 6)`
// pipes it through new channels and returns the ports to send, and
// `host([kind, ports], 7)` makes the stream of a decoded 'S' tree.
// `host(value, 0)` also describes an ImageBitmap or OffscreenCanvas as
// ['C', index] (its pixels or placeholder travel beside the text);
// `host(value, 8)` is 'C' for one that can be transferred, 'A' for AudioData (DataCloneError
// if detached), `host(value, 9)` detaches it and `host(index, 10)` makes the
// one of a decoded 'C' tree. For WebCodecs resources, host(value, 11) stores
// the immutable slots and returns their index, and host(index, 12) restores
// a 'Q' record in the receiving realm. `host(value, 0)` also describes a
// FileSystemHandle as ['O', index] (the locator travels beside the text);
// `host(index, 18)` makes the handle of a decoded 'O' tree (DataCloneError
// unless the receiver is a secure context of the handle's origin).
// Blob bytes travel beside the text, shared, not copied. Transferred
// MessagePorts travel beside it too: `encode` returns them in transfer-list
// order, then the ports of the transferred streams, with the count of the
// former; the caller detaches them, and `decode` receives their new ports.
((fail, host, DOMExceptionC) => {
    const O = Object, A = Array, M = Map, S = Set, D = Date, R = RegExp;
    const AB = ArrayBuffer, DV = DataView, U8 = Uint8Array, TE = TypeError;
    const errors = {Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError};
    const stringify = JSON.stringify, parse = JSON.parse;
    const apply = Reflect.apply, keys = O.keys, descriptor = O.getOwnPropertyDescriptor;
    const prototype = O.getPrototypeOf, define = O.defineProperty, create = O.create;
    const tag = Symbol.toStringTag, iteratorKey = Symbol.iterator, objectTag = O.prototype.toString;
    const stringSlice = String.prototype.slice, bigintString = BigInt.prototype.toString, big = BigInt;
    const isArray = A.isArray, isView = AB.isView;
    const mapGet = M.prototype.get, mapSet = M.prototype.set, mapHas = M.prototype.has;
    const mapForEach = M.prototype.forEach, setForEach = S.prototype.forEach, setAdd = S.prototype.add;
    const dateTime = D.prototype.getTime;
    const regexpSource = descriptor(R.prototype, 'source').get, regexpFlags = descriptor(R.prototype, 'flags').get;
    const getter = (proto, key) => descriptor(proto, key)?.get;
    const abLength = getter(AB.prototype, 'byteLength'), abResizable = getter(AB.prototype, 'resizable');
    const abMax = getter(AB.prototype, 'maxByteLength'), abTransfer = AB.prototype.transfer;
    const dvBuffer = getter(DV.prototype, 'buffer'), dvOffset = getter(DV.prototype, 'byteOffset');
    const dvLength = getter(DV.prototype, 'byteLength');
    const domName = getter(DOMExceptionC.prototype, 'name'), domMessage = getter(DOMExceptionC.prototype, 'message');
    const data = (object, key) => { const d = descriptor(object, key); return d && descriptor(d, 'value') ? d : undefined; };
    const typedProto = prototype(U8.prototype), typedName = getter(typedProto, tag);
    const typedBuffer = getter(typedProto, 'buffer'), typedOffset = getter(typedProto, 'byteOffset');
    const typedLength = getter(typedProto, 'length');
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
        {value, enumerable: true, writable: true, configurable: true});
    const hide = (object, key, value) => define(object, key, {value, writable: true, configurable: true});
    const isBuffer = (value) => {
        try { call(abLength, value); return true; } catch (_) { return false; }
    };

    // The buffers `postMessage(message, transfer)` detaches once the message is
    // encoded, and the MessagePorts it transfers. Worker messages always cross
    // a VM, so transferring a buffer is a copy followed by a detach, which is
    // what the sender can observe.
    const none = [[], [], [], [], []];
    function transferList(options) {
        if (options === undefined || options === null) return none;
        let list;
        if (isArray(options)) list = options;
        else if (typeof options !== 'object' && typeof options !== 'function')
            throw new TE('postMessage options must be an object or a sequence');
        else list = options.transfer;
        if (list === undefined) return none;
        if (list === null || typeof list[iteratorKey] !== 'function') throw new TE('transfer must be iterable');
        const entries = [];
        for (const entry of list) {
            if (entry === null || (typeof entry !== 'object' && typeof entry !== 'function'))
                throw new TE('transfer entries must be objects');
            put(entries, entries.length, entry);
        }
        const buffers = [], ports = [], streams = [], kinds = [], resources = [], seen = new M();
        for (let i = 0; i < entries.length; ++i) {
            const entry = entries[i];
            if (call(mapHas, seen, [entry])) fail('An object is listed twice in the transfer list');
            call(mapSet, seen, [entry, true]);
            if (isBuffer(entry)) {
                try { new U8(entry); } catch (_) { fail('An ArrayBuffer is detached and could not be transferred'); }
                put(buffers, buffers.length, entry);
            } else if (host(entry, 4)) {
                put(ports, ports.length, entry);
            } else if (host(entry, 8) !== undefined) {
                // A transferable canvas object or AudioData.
                put(resources, resources.length, entry);
            } else {
                // A ReadableStream, WritableStream or TransformStream ('R',
                // 'W', 'T'; DataCloneError when locked).
                const kind = host(entry, 5);
                if (kind === undefined) fail('Value is not transferable');
                put(streams, streams.length, entry);
                put(kinds, kinds.length, kind);
            }
        }
        if (buffers.length && !abTransfer) fail('ArrayBuffer transfer is not available', true);
        return [buffers, ports, streams, kinds, resources];
    }

    function encode(clone, value, options, forStorage) {
        const lists = transferList(options), transfer = lists[0], ports = lists[1];
        const streams = lists[2], kinds = lists[3], resources = lists[4];
        // Listed ports, streams and resources stay themselves in the copy;
        // the walk numbers them. A stream's ports follow the listed ports.
        const keep = [];
        for (let i = 0; i < ports.length; ++i) put(keep, keep.length, ports[i]);
        for (let i = 0; i < streams.length; ++i) put(keep, keep.length, streams[i]);
        for (let i = 0; i < resources.length; ++i) put(keep, keep.length, resources[i]);
        const copy = clone(value, undefined, undefined, keep.length ? keep : undefined, undefined, undefined, forStorage);
        // Only a message that transfers something pays for these lookups.
        const portIndex = ports.length ? new M() : undefined;
        const streamTrees = streams.length ? new M() : undefined;
        for (let i = 0; i < ports.length; ++i) call(mapSet, portIndex, [ports[i], i]);
        for (let i = 0, at = ports.length; i < streams.length; ++i) {
            const two = kinds[i] === 'T';
            call(mapSet, streamTrees, [streams[i], '["S",' + stringify(kinds[i]) + ',' + at + (two ? ',' + (at + 1) : '') + ']']);
            at += two ? 2 : 1;
        }
        const buffers = [], memory = new M();
        let next = 0;
        const entries = (source) => {
            let text = '';
            const names = keys(source);
            for (let i = 0; i < names.length; ++i) {
                text += ',' + stringify(names[i]) + ',' + node(source[names[i]]);
            }
            return text;
        };
        function node(source) {
            switch (typeof source) {
                case 'string': return stringify(source);
                case 'boolean': return source ? 'true' : 'false';
                case 'undefined': return '["u"]';
                case 'bigint': return '["b",' + stringify(call(bigintString, source)) + ']';
                case 'number':
                    if (source !== source) return '["n","NaN"]';
                    if (source === Infinity) return '["n","Infinity"]';
                    if (source === -Infinity) return '["n","-Infinity"]';
                    if (source === 0 && 1 / source < 0) return '["n","-0"]';
                    return '' + source;
            }
            if (source === null) return 'null';
            if (portIndex && call(mapHas, portIndex, [source])) return '["P",' + call(mapGet, portIndex, [source]) + ']';
            if (call(mapHas, memory, [source])) return '["r",' + call(mapGet, memory, [source]) + ']';
            call(mapSet, memory, [source, next++]);
            if (streamTrees && call(mapHas, streamTrees, [source])) return call(mapGet, streamTrees, [source]);
            if (isArray(source)) return '["a",' + source.length + entries(source) + ']';
            if (isView(source)) {
                const name = call(typedName, source);
                if (name !== undefined) {
                    const original = call(typedBuffer, source), offset = call(typedOffset, source);
                    // A length-tracking view's length is null (probed before the buffer is serialized).
                    const length = tracksLength(source, original, offset, typedByteLength) ? null : call(typedLength, source);
                    const buffer = node(original);
                    return '["v",' + stringify(name) + ',' + buffer + ',' + offset + ',' + length + ']';
                }
                const original = call(dvBuffer, source), offset = call(dvOffset, source);
                const length = tracksLength(source, original, offset, dvLength) ? null : call(dvLength, source);
                const buffer = node(original);
                return '["w",' + buffer + ',' + offset + ',' + length + ']';
            }
            // Read native WebCodecs slots without consulting public getters.
            const resource = host(source, 11);
            if (resource !== undefined) return '["Q",' + resource + ']';
            const kind = call(objectTag, source);
            switch (kind) {
                case '[object DOMPoint]': case '[object DOMPointReadOnly]':
                case '[object DOMRect]': case '[object DOMRectReadOnly]':
                case '[object DOMMatrix]': case '[object DOMMatrixReadOnly]': case '[object DOMQuad]': {
                    const parts = host(source, 15);
                    if (!parts) fail('Invalid geometry value');
                    return '["G",' + stringify(parts[0]) + ',' + node(parts[1]) + ']';
                }
                case '[object ImageData]': {
                    const parts = host(source, 13);
                    if (parts == null) fail('Invalid ImageData');
                    // Encode the pixel view through the graph so a separately
                    // listed reference to image.data retains its identity.
                    return '["I",' + parts[0] + ',' + parts[1] + ',' + stringify(parts[3]) + ',' + node(parts[2]) + ']';
                }
                case '[object Date]': return '["d",' + node(call(dateTime, source)) + ']';
                case '[object RegExp]':
                    return '["x",' + stringify(call(regexpSource, source)) + ','
                        + stringify(call(regexpFlags, source)) + ']';
                case '[object ArrayBuffer]': {
                    const index = buffers.length;
                    put(buffers, index, source);
                    const max = abResizable && call(abResizable, source) ? call(abMax, source) : -1;
                    return '["f",' + index + ',' + max + ']';
                }
                case '[object Map]': {
                    let text = '["m"';
                    call(mapForEach, source, [(v, k) => {
                        text += ',' + node(k) + ',' + node(v);
                    }]);
                    return text + ']';
                }
                case '[object Set]': {
                    let text = '["s"';
                    call(setForEach, source, [(v) => { text += ',' + node(v); }]);
                    return text + ']';
                }
                case '[object Object]': return '["o"' + entries(source) + ']';
                case '[object Error]': {
                    // The clone made these own data properties.
                    const message = data(source, 'message'), stack = data(source, 'stack');
                    const cause = data(source, 'cause'), name = source.name;
                    return '["e",' + stringify(typeof name === 'string' && descriptor(errors, name) ? name : 'Error')
                        + ',' + node(message && message.value) + ',' + node(stack && stack.value)
                        + (cause ? ',' + node(cause.value) : '') + ']';
                }
                case '[object DOMException]': {
                    const stack = data(source, 'stack');
                    return '["E",' + stringify(call(domName, source)) + ',' + stringify(call(domMessage, source))
                        + ',' + node(stack && stack.value) + ']';
                }
                // A QuotaExceededError: its message, quota and requested (null for none).
                case '[object QuotaExceededError]': {
                    const QEE = globalThis.QuotaExceededError, stack = data(source, 'stack');
                    const quota = call(descriptor(QEE.prototype, 'quota').get, source);
                    const requested = call(descriptor(QEE.prototype, 'requested').get, source);
                    return '["U",' + stringify(call(domMessage, source)) + ',' + stringify(quota) + ','
                        + stringify(requested) + ',' + node(stack && stack.value) + ']';
                }
            }
            const parts = host(source, 0);
            if (parts !== undefined) {
                if (parts[0] === 'K') return '["K",' + node(parts[1]) + ']';
                if (parts[0] === 'C') return '["C",' + parts[1] + ']';
                if (parts[0] === 'O') return '["O",' + parts[1] + ']';
                if (parts[0] === 'F') {
                    return '["F",' + parts[1] + ',' + stringify(parts[2]) + ','
                        + (parts[3] === null ? 'null' : stringify(parts[3])) + ',' + node(parts[4]) + ']';
                }
                let text = '["L"';
                for (let i = 1; i < parts.length; ++i) text += ',' + node(parts[i]);
                return text + ']';
            }
            const name = call(stringSlice, kind, [8, -1]);
            if (!descriptor(boxed, name)) fail('Object type is not supported');
            return '["B",' + stringify(name) + ',' + node(call(boxed[name], source)) + ']';
        }
        const text = node(copy);
        // Getters ran during the clone: every transferable is checked again
        // before any is consumed, so a failure leaves them all as they were.
        for (let i = 0; i < transfer.length; ++i) {
            try { new U8(transfer[i]); } catch (_) { fail('An ArrayBuffer is detached and could not be transferred'); }
        }
        for (let i = 0; i < ports.length; ++i) host(ports[i], 4);
        for (let i = 0; i < streams.length; ++i) host(streams[i], 5);
        for (let i = 0; i < resources.length; ++i) {
            if (host(resources[i], 8) === 'A' && !call(mapHas, memory, [resources[i]])) {
                // Even a frame only in the transfer list must be readable in
                // the destination agent cluster when deserializing the message.
                host(resources[i], 11);
            }
        }
        for (let i = 0; i < transfer.length; ++i) call(abTransfer, transfer[i]);
        const listed = ports.length, sent = [];
        for (let i = 0; i < listed; ++i) put(sent, i, ports[i]);
        for (let i = 0; i < streams.length; ++i) {
            const out = host(streams[i], 6);
            for (let j = 0; j < out.length; ++j) put(sent, sent.length, out[j]);
        }
        for (let i = 0; i < resources.length; ++i) host(resources[i], 9);
        return [text, buffers, sent, listed];
    }

    function decode(text, buffers, ports) {
        const memory = new M();
        let next = 0;
        const remember = (id, value) => { call(mapSet, memory, [id, value]); return value; };
        const fill = (object, tree, from) => {
            for (let i = from; i + 1 < tree.length; i += 2) put(object, tree[i], node(tree[i + 1]));
            return object;
        };
        function node(tree) {
            if (tree === null || typeof tree !== 'object') return tree;
            switch (tree[0]) {
                case 'u': return undefined;
                case 'b': return big(tree[1]);
                case 'n': return tree[1] === 'NaN' ? NaN : tree[1] === 'Infinity' ? Infinity
                    : tree[1] === '-Infinity' ? -Infinity : -0;
                case 'r': return call(mapGet, memory, [tree[1]]);
                case 'P': {
                    const port = ports[tree[1]];
                    if (port === undefined) fail('Malformed worker message');
                    return port;
                }
            }
            const id = next++;
            switch (tree[0]) {
                case 'a': return fill(remember(id, new A(tree[1])), tree, 2);
                case 'o': return fill(remember(id, create(O.prototype)), tree, 1);
                case 'v': return remember(id, tree[4] === null
                    ? new typed[tree[1]](node(tree[2]), tree[3]) : new typed[tree[1]](node(tree[2]), tree[3], tree[4]));
                case 'w': return remember(id, tree[3] === null
                    ? new DV(node(tree[1]), tree[2]) : new DV(node(tree[1]), tree[2], tree[3]));
                case 'd': return remember(id, new D(node(tree[1])));
                case 'x': return remember(id, new R(tree[1], tree[2]));
                case 'f': {
                    const bytes = buffers[tree[1]];
                    if (tree[2] < 0) return remember(id, bytes);
                    const copy = new AB(call(abLength, bytes), {maxByteLength: tree[2]});
                    call(byteSet, new U8(copy), [new U8(bytes)]);
                    return remember(id, copy);
                }
                case 'm': {
                    const map = remember(id, new M());
                    for (let i = 1; i + 1 < tree.length; i += 2) {
                        const key = node(tree[i]);
                        call(mapSet, map, [key, node(tree[i + 1])]);
                    }
                    return map;
                }
                case 's': {
                    const set = remember(id, new S());
                    for (let i = 1; i < tree.length; ++i) call(setAdd, set, [node(tree[i])]);
                    return set;
                }
                case 'B': return remember(id, O(node(tree[2])));
                case 'e': {
                    const C = descriptor(errors, tree[1]) ? errors[tree[1]] : Error, message = node(tree[2]);
                    const error = remember(id, message === undefined ? new C() : new C(message));
                    const stack = node(tree[3]);
                    if (typeof stack === 'string') hide(error, 'stack', stack);
                    if (tree.length > 4) hide(error, 'cause', node(tree[4]));
                    return error;
                }
                case 'E': {
                    const error = remember(id, new DOMExceptionC(tree[2], tree[1]));
                    const stack = node(tree[3]);
                    if (typeof stack === 'string') hide(error, 'stack', stack);
                    return error;
                }
                case 'U': {
                    const options = {};
                    if (tree[2] !== null) options.quota = tree[2];
                    if (tree[3] !== null) options.requested = tree[3];
                    const error = remember(id, new globalThis.QuotaExceededError(tree[1], options));
                    const stack = node(tree[4]);
                    if (typeof stack === 'string') hide(error, 'stack', stack);
                    return error;
                }
                case 'G': return remember(id, host(tree[1], 16, node(tree[2])));
                case 'F': return remember(id, host(tree, 1));
                case 'C': return remember(id, host(tree[1], 10));
                case 'O': return remember(id, host(tree[1], 18));
                case 'Q': return remember(id, host(tree[1], 12));
                case 'I': return remember(id, host(tree, 14, node(tree[4])));
                case 'S': {
                    const streamPorts = [ports[tree[2]]];
                    if (tree.length > 3) put(streamPorts, 1, ports[tree[3]]);
                    for (let i = 0; i < streamPorts.length; ++i)
                        if (streamPorts[i] === undefined) fail('Malformed worker message');
                    return remember(id, host([tree[1], streamPorts], 7));
                }
                case 'K': {
                    // Nothing can refer back into the key's parts.
                    const key = host(node(tree[1]), 3);
                    return remember(id, key);
                }
                case 'L': {
                    const files = [];
                    for (let i = 1; i < tree.length; ++i) put(files, files.length, node(tree[i]));
                    return remember(id, host(files, 2));
                }
            }
            fail('Malformed worker message');
        }
        return node(parse(text));
    }
    return [encode, decode];
})
