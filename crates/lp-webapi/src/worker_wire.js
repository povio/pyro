// Cross-VM message format of dedicated workers. Compiled on a realm's first
// worker use, so pages without workers never parse it.
//
// `encode` validates and copies with the realm's original `structuredClone`,
// then walks that copy, a graph of plain data the page cannot intercept, into
// JSON text plus the bytes of its ArrayBuffers. Lone surrogates stay escaped in
// the text. Only strings go through JSON.stringify, so a `toJSON` on a
// prototype cannot affect the output.
((fail) => {
    const O = Object, A = Array, M = Map, S = Set, D = Date, R = RegExp;
    const AB = ArrayBuffer, DV = DataView, U8 = Uint8Array, TE = TypeError;
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
    const put = (object, key, value) => define(object, key,
        {value, enumerable: true, writable: true, configurable: true});
    const isBuffer = (value) => {
        try { call(abLength, value); return true; } catch (_) { return false; }
    };

    // The buffers `postMessage(message, transfer)` detaches once the message is
    // encoded. Worker messages always cross a VM, so transferring is a copy
    // followed by a detach, which is what the sender can observe.
    function transferList(options) {
        if (options === undefined || options === null) return [];
        let list;
        if (isArray(options)) list = options;
        else if (typeof options !== 'object' && typeof options !== 'function')
            throw new TE('postMessage options must be an object or a sequence');
        else list = options.transfer;
        if (list === undefined) return [];
        if (list === null || typeof list[iteratorKey] !== 'function') throw new TE('transfer must be iterable');
        const buffers = [], seen = new M();
        for (const entry of list) {
            if (entry === null || (typeof entry !== 'object' && typeof entry !== 'function'))
                throw new TE('transfer entries must be objects');
            if (!isBuffer(entry)) fail('Only ArrayBuffers can be transferred to or from a worker', true);
            if (call(mapHas, seen, [entry])) fail('ArrayBuffer listed twice in the transfer list');
            call(mapSet, seen, [entry, true]);
            put(buffers, buffers.length, entry);
        }
        if (buffers.length && !abTransfer) fail('ArrayBuffer transfer is not available', true);
        return buffers;
    }

    function encode(clone, value, options) {
        const transfer = transferList(options);
        const copy = clone(value);
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
            if (call(mapHas, memory, [source])) return '["r",' + call(mapGet, memory, [source]) + ']';
            call(mapSet, memory, [source, next++]);
            if (isArray(source)) return '["a",' + source.length + entries(source) + ']';
            if (isView(source)) {
                const name = call(typedName, source);
                if (name !== undefined) {
                    const buffer = node(call(typedBuffer, source));
                    return '["v",' + stringify(name) + ',' + buffer + ',' + call(typedOffset, source) + ','
                        + call(typedLength, source) + ']';
                }
                const buffer = node(call(dvBuffer, source));
                return '["w",' + buffer + ',' + call(dvOffset, source) + ',' + call(dvLength, source) + ']';
            }
            const kind = call(objectTag, source);
            switch (kind) {
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
            }
            const name = call(stringSlice, kind, [8, -1]);
            if (!descriptor(boxed, name)) fail('Object type is not supported');
            return '["B",' + stringify(name) + ',' + node(call(boxed[name], source)) + ']';
        }
        const text = node(copy);
        for (let i = 0; i < transfer.length; ++i) call(abTransfer, transfer[i]);
        return [text, buffers];
    }

    function decode(text, buffers) {
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
            }
            const id = next++;
            switch (tree[0]) {
                case 'a': return fill(remember(id, new A(tree[1])), tree, 2);
                case 'o': return fill(remember(id, create(O.prototype)), tree, 1);
                case 'v': return remember(id, new typed[tree[1]](node(tree[2]), tree[3], tree[4]));
                case 'w': return remember(id, new DV(node(tree[1]), tree[2], tree[3]));
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
            }
            fail('Malformed worker message');
        }
        return node(parse(text));
    }
    return [encode, decode];
})
