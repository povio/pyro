// The underlying WritableStream owns ordering, locking, piping, backpressure
// and error propagation. A native lease owns the open file's cursor and lock.
(function (ops) {
  'use strict';
  const token = Symbol();
  const apply = Reflect.apply;
  const descriptor = Object.getOwnPropertyDescriptor;
  const Uint8 = Uint8Array;
  const isView = ArrayBuffer.isView;
  const arrayBufferLength = descriptor(ArrayBuffer.prototype, 'byteLength').get;
  const sharedLength = typeof SharedArrayBuffer === 'function'
    ? descriptor(SharedArrayBuffer.prototype, 'byteLength').get : undefined;
  const blobSize = descriptor(Blob.prototype, 'size').get;
  const blobArrayBuffer = Blob.prototype.arrayBuffer;
  const encoder = new TextEncoder();
  const encode = TextEncoder.prototype.encode;
  const setBytes = Uint8.prototype.set;
  const getWriter = WritableStream.prototype.getWriter;
  const writerWrite = WritableStreamDefaultWriter.prototype.write;
  const releaseWriter = WritableStreamDefaultWriter.prototype.releaseLock;
  const define = Object.defineProperty;
  const getNames = Object.getOwnPropertyNames;
  const TypeErrorC = TypeError;
  const trunc = Math.trunc;
  const finite = Number.isFinite;
  const UINT64_LIMIT = 2 ** 64;
  const toBigInt = BigInt;
  const asUint64 = BigInt.asUintN;
  const bigIntString = BigInt.prototype.toString;

  function viewGetters(proto) {
    return {
      buffer: descriptor(proto, 'buffer').get,
      offset: descriptor(proto, 'byteOffset').get,
      length: descriptor(proto, 'byteLength').get,
    };
  }
  const typed = viewGetters(Object.getPrototypeOf(Uint8.prototype));
  const dataView = viewGetters(DataView.prototype);

  function branded(get, value) {
    if (!get) return false;
    try { apply(get, value, []); return true; } catch { return false; }
  }

  // Use the view's actual byte span, including for non-byte typed arrays and
  // DataViews. Getters on an author-defined subclass cannot change the span.
  function bufferView(value, allowShared) {
    let buffer = value, offset = 0, length;
    if (isView(value)) {
      const access = branded(typed.buffer, value) ? typed : dataView;
      buffer = apply(access.buffer, value, []);
      offset = apply(access.offset, value, []);
      length = apply(access.length, value, []);
    } else if (!branded(arrayBufferLength, value) && !branded(sharedLength, value)) {
      return undefined;
    }
    if (!allowShared && !branded(arrayBufferLength, buffer)) {
      throw new TypeErrorC('A shared buffer is not a file stream chunk.');
    }
    return new Uint8(buffer, offset, length);
  }

  function dictionary(value) {
    if (value === null || value === undefined) return {};
    if (typeof value !== 'object' && typeof value !== 'function') {
      throw new TypeErrorC('Expected a dictionary.');
    }
    return value;
  }

  function unsigned(value, enforceRange) {
    const n = trunc(+value);
    if (enforceRange) {
      if (!finite(n) || n < 0 || n >= UINT64_LIMIT) throw new TypeErrorC('Offset outside unsigned 64-bit range.');
      return n;
    }
    if (!finite(n)) return 0;
    if (n >= 0 && n < UINT64_LIMIT) return n;
    // WebIDL wraps ordinary unsigned long long arguments modulo 2^64.
    // A Number cannot carry the exact wrapped result of e.g. -1 to Rust.
    return apply(bigIntString, asUint64(64, toBigInt(n)), []);
  }

  function asData(value) {
    if (typeof value !== 'object' && typeof value !== 'function') return `${value}`;
    const view = bufferView(value, false);
    if (view !== undefined) return new Uint8(view);
    if (branded(blobSize, value)) return value;
    // Template-literal coercion uses WebIDL ToString (including rejecting a
    // Symbol). TextEncoder performs the USVString scalar-value conversion.
    return `${value}`;
  }

  class Command {
    constructor(type, position, size, data) {
      this.type = type;
      this.position = position;
      this.size = size;
      this.data = data;
    }
  }

  // WebIDL dictionary conversion gets members in lexicographic order.
  function convertChunk(value) {
    if (value !== null && value !== undefined &&
        (typeof value !== 'object' && typeof value !== 'function' ||
         isView(value) || branded(arrayBufferLength, value) ||
         branded(sharedLength, value) || branded(blobSize, value))) {
      return new Command('write', null, null, asData(value));
    }
    const options = dictionary(value);
    const rawData = options.data;
    const data = rawData == null ? rawData : asData(rawData);
    const pos = options.position;
    const position = pos == null ? null : unsigned(pos, false);
    const len = options.size;
    const size = len == null ? null : unsigned(len, false);
    const type = `${options.type}`;
    if (type !== 'write' && type !== 'seek' && type !== 'truncate') {
      throw new TypeErrorC('Expected a write, seek or truncate command.');
    }
    return new Command(type, position, size, data);
  }

  async function writeChunk(lease, chunk) {
    try {
      const command = chunk instanceof Command ? chunk : convertChunk(chunk);
      if (command.type === 'seek') {
        if (command.position === null) throw new TypeErrorC('Seek requires a position.');
        ops.seek(lease, command.position);
      } else if (command.type === 'truncate') {
        if (command.size === null) throw new TypeErrorC('Truncate requires a size.');
        ops.truncate(lease, command.size);
      } else {
        const data = command.data;
        if (data == null) throw new TypeErrorC('Write requires data.');
        let bytes;
        if (typeof data === 'string') bytes = apply(encode, encoder, [data]);
        else if (isView(data)) bytes = data;
        else bytes = new Uint8(await apply(blobArrayBuffer, data, []));
        ops.write(lease, bytes, command.position);
      }
    } catch (error) {
      // Streams erroring from a rejected sink write do not call sink.abort.
      // Release the exclusive/siloed transaction and discard its buffer now.
      ops.abort(lease);
      throw error;
    }
  }

  function queue(stream, command) {
    const writer = apply(getWriter, stream, []);
    try { return apply(writerWrite, writer, [command]); }
    finally { apply(releaseWriter, writer, []); }
  }

  class FileSystemWritableFileStream extends WritableStream {
    #mode;
    constructor(...args) {
      if (args[0] !== token) throw new TypeErrorC('Illegal constructor');
      const lease = args[1];
      super({
        write(chunk) { return writeChunk(lease, chunk); },
        close() {
          try { ops.close(lease); }
          catch (error) { ops.abort(lease); throw error; }
        },
        abort() { ops.abort(lease); },
      });
      this.#mode = args[2];
    }
    get mode() { return this.#mode; }
    async write(data) {
      if (!(#mode in this) || !arguments.length) throw new TypeErrorC('Expected a file stream and a chunk.');
      return queue(this, convertChunk(data));
    }
    async seek(position) {
      if (!(#mode in this) || !arguments.length) throw new TypeErrorC('Expected a file stream and a position.');
      return queue(this, new Command('seek', unsigned(position, false), null, undefined));
    }
    async truncate(size) {
      if (!(#mode in this) || !arguments.length) throw new TypeErrorC('Expected a file stream and a size.');
      return queue(this, new Command('truncate', null, unsigned(size, false), undefined));
    }
  }

  function syncArguments(buffer, options) {
    const bytes = bufferView(buffer, true);
    if (bytes === undefined) throw new TypeErrorC('Expected a buffer source.');
    const at = dictionary(options).at;
    return [bytes, at === undefined ? null : unsigned(at, true)];
  }

  class FileSystemSyncAccessHandle {
    #lease;
    #mode;
    constructor(...args) {
      if (args[0] !== token) throw new TypeErrorC('Illegal constructor');
      this.#lease = args[1];
      this.#mode = args[2];
    }
    get mode() { return this.#mode; }
    read(buffer, options = undefined) {
      const lease = this.#lease;
      const [view, at] = syncArguments(buffer, options);
      const bytes = ops.read(lease, apply(typed.length, view, []), at);
      apply(setBytes, view, [bytes]);
      return apply(typed.length, bytes, []);
    }
    write(buffer, options = undefined) {
      const lease = this.#lease;
      const [view, at] = syncArguments(buffer, options);
      return ops.write(lease, new Uint8(view), at);
    }
    truncate(size) {
      const lease = this.#lease;
      if (!arguments.length) throw new TypeErrorC('Expected a size.');
      ops.truncate(lease, unsigned(size, true));
    }
    getSize() { return ops.size(this.#lease); }
    flush() { ops.flush(this.#lease); }
    close() { ops.close(this.#lease); }
  }

  async function createWritable(entry, options) {
    const dict = dictionary(options);
    const keep = !!dict.keepExistingData;
    const rawMode = dict.mode;
    const mode = rawMode === undefined ? 'siloed' : `${rawMode}`;
    if (mode !== 'siloed' && mode !== 'exclusive') throw new TypeErrorC('Unknown writable stream mode.');
    await undefined; // Queue opening after argument conversion, like the entry operations.
    const lease = ops.openWritable(entry, keep, mode);
    try { return new FileSystemWritableFileStream(token, lease, mode); }
    catch (error) { ops.abort(lease); throw error; }
  }

  async function createSync(entry, options) {
    const rawMode = dictionary(options).mode;
    const mode = rawMode === undefined ? 'readwrite' : `${rawMode}`;
    if (mode !== 'readwrite' && mode !== 'read-only' && mode !== 'readwrite-unsafe') {
      throw new TypeErrorC('Unknown access handle mode.');
    }
    await undefined;
    const lease = ops.openSync(entry, mode);
    return new FileSystemSyncAccessHandle(token, lease, mode);
  }

  for (const cls of [FileSystemWritableFileStream, FileSystemSyncAccessHandle]) {
    for (const key of getNames(cls.prototype)) {
      if (key !== 'constructor') define(cls.prototype, key, {enumerable: true});
    }
    define(cls.prototype, Symbol.toStringTag, {value: cls.name, configurable: true});
  }
  return {FileSystemWritableFileStream, FileSystemSyncAccessHandle, createWritable, createSync};
})
