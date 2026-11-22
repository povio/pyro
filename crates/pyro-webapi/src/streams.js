// WHATWG Streams for one realm, made on first use: ReadableStream (default and
// byte), WritableStream, TransformStream, the queuing strategies,
// TextEncoderStream/TextDecoderStream and CompressionStream/DecompressionStream
// (over `natives.zlib`, compression.rs), plus the helpers streams.rs calls for
// Response bodies and Blob.stream(). Operations follow the spec's abstract
// operations and keep their names, shortened; public classes hold a state
// object in a private field, so page code cannot reach the internals.
//
// The internal slots are not private fields but WeakMaps the engine shares
// among the agent's realms of one origin (`natives.slot`, streams.rs), so a
// stream made in another same-origin window passes this realm's brand checks
// and works with its operations, as the spec's platform objects do whatever
// their realm.
//
// Byte streams follow the spec's pull-into descriptors: enqueue() and BYOB
// reads transfer buffers, `byobRequest` views the pending read's own buffer,
// and a released reader's partly filled read carries over to the next reader.
// Byte tee follows ReadableByteStreamTee (default and BYOB source readers).
(function (natives) {
  'use strict';
  const token = Symbol('pyro');
  const zlib = natives.zlib;
  const PromiseC = Promise;
  const promiseThen = PromiseC.prototype.then;
  const promiseAll = PromiseC.all.bind(PromiseC);
  const apply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getPrototypeOf = Object.getPrototypeOf;
  const TypeErrorC = TypeError;
  const RangeErrorC = RangeError;
  const NumberC = Number;
  const Uint8ArrayC = Uint8Array;
  const DataViewC = DataView;
  const isView = ArrayBuffer.isView;
  // Whether `v` is a Uint8Array of any realm (its [[TypedArrayName]], which
  // %TypedArray%.prototype[@@toStringTag] reads), as WebIDL's conversion checks.
  const typedArrayName = Object.getOwnPropertyDescriptor(getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const isUint8Array = (v) => isView(v) && apply(typedArrayName, v, []) === 'Uint8Array';
  // Pull-into descriptors use the view's intrinsic kind and offsets. A typed
  // array subclass can replace its public constructor and instance properties.
  const viewGetters = prototype => ({
    buffer: Object.getOwnPropertyDescriptor(prototype, 'buffer').get,
    offset: Object.getOwnPropertyDescriptor(prototype, 'byteOffset').get,
    length: Object.getOwnPropertyDescriptor(prototype, 'byteLength').get,
  });
  const typedView = viewGetters(getPrototypeOf(Uint8ArrayC.prototype));
  const dataView = viewGetters(DataViewC.prototype);
  const typedCtors = { Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
    Int32Array, Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array };
  if (typeof Float16Array !== 'undefined') typedCtors.Float16Array = Float16Array;
  const transferBuffer = ArrayBuffer.prototype.transfer;
  const bufferLength = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength').get;
  const detachedGetter = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'detached');
  // TransferArrayBuffer and IsDetachedBuffer (byteLength 0 where the engine
  // has no `detached`; the callers never hold zero-length buffers otherwise).
  const transfer = (b) => (transferBuffer ? apply(transferBuffer, b, []) : b);
  const isDetached = (b) => (detachedGetter ? apply(detachedGetter.get, b, []) : apply(bufferLength, b, []) === 0);
  const parseJSON = JSON.parse;
  const TextDecoderC = TextDecoder;
  const TextEncoderC = TextEncoder;
  const decode = TextDecoder.prototype.decode;
  const encode = TextEncoder.prototype.encode;
  const AbortControllerC = AbortController;
  const abortController = AbortController.prototype.abort;
  const signalOf = Object.getOwnPropertyDescriptor(AbortController.prototype, 'signal').get;
  const AbortSignalC = AbortSignal;
  const addListener = EventTarget.prototype.addEventListener;
  const removeListener = EventTarget.prototype.removeEventListener;
  const fetchFn = fetch;
  const DOMExceptionC = DOMException;
  // What stream transfer uses, if the realm still has it (without it,
  // streams are not transferable).
  let MessageChannelC, eventData, portPost, portStart, portClose, port1Of, port2Of;
  try {
    const own = Object.getOwnPropertyDescriptor;
    eventData = own(MessageEvent.prototype, 'data').get;
    portPost = MessagePort.prototype.postMessage;
    portStart = MessagePort.prototype.start;
    portClose = MessagePort.prototype.close;
    port1Of = own(MessageChannel.prototype, 'port1').get;
    port2Of = own(MessageChannel.prototype, 'port2').get;
    MessageChannelC = MessageChannel;
  } catch (_) {}
  // An internal slot of one interface (`natives.slot`, streams.rs: the root
  // context's functions over a map the realms of this origin share).
  // `get` throws for an object without the slot, as reading a missing
  // private field would. The functions stay in this closure: handed to page
  // code they would give it the root context.
  const slot = (name) => {
    const s = natives.slot(natives.origin, name);
    const has = s.has;
    const get = s.get;
    return {
      has,
      get: (o) => {
        const v = get(o);
        if (v === undefined && !has(o)) throw new TypeErrorC('Illegal invocation');
        return v;
      },
      set: s.set,
    };
  };
  const asyncIterator = Symbol.asyncIterator;
  const iterator = Symbol.iterator;
  const AsyncIteratorPrototype = getPrototypeOf(getPrototypeOf((async function* () {}).prototype));

  const noop = () => {};
  const isObject = (v) => (typeof v === 'object' && v !== null) || typeof v === 'function';
  const resolved = (v) => PromiseC.resolve(v);
  const rejected = (e) => PromiseC.reject(e);
  const then = (p, f, r) => apply(promiseThen, p, [f, r]);
  const handled = (p) => { then(p, undefined, noop); return p; };
  const microtask = (f) => { then(resolved(), f); };
  const defer = () => {
    const d = { promise: undefined, resolve: undefined, reject: undefined, settled: false };
    d.promise = new PromiseC((res, rej) => {
      d.resolve = (v) => { d.settled = true; res(v); };
      d.reject = (e) => { d.settled = true; rej(e); };
    });
    return d;
  };
  const settledDefer = (p) => ({ promise: p, resolve: noop, reject: noop, settled: true });
  // A call into page code as a promise; throwing rejects.
  const call = (fn, self, args) => {
    try {
      return resolved(apply(fn, self, args));
    } catch (e) {
      return rejected(e);
    }
  };
  const callback = (dict, name, label) => {
    const v = dict[name];
    if (v !== undefined && typeof v !== 'function') throw new TypeErrorC(`Failed to construct '${label}': ${name} is not a function`);
    return v;
  };
  const dictionary = (v, label) => {
    if (v === undefined || v === null) return {};
    if (!isObject(v)) throw new TypeErrorC(`Failed to construct '${label}': The provided value is not an object.`);
    return v;
  };
  const strategyOf = (strategy, label) => {
    const d = dictionary(strategy, label);
    const hwm = d.highWaterMark;
    return { highWaterMark: hwm === undefined ? undefined : NumberC(hwm), size: callback(d, 'size', label) };
  };
  const highWaterMark = (strategy, fallback) => {
    const hwm = strategy.highWaterMark;
    if (hwm === undefined) return fallback;
    if (hwm !== hwm || hwm < 0) throw new RangeErrorC('Invalid highWaterMark');
    return hwm;
  };
  const sizeAlgorithm = (strategy) => {
    const size = strategy.size;
    return size === undefined ? () => 1 : (chunk) => apply(size, undefined, [chunk]);
  };
  const checkSize = (size) => {
    const n = NumberC(size);
    if (n !== n || n < 0 || n === Infinity) throw new RangeErrorC('The return value of a queuing strategy size function must be a finite, non-NaN, non-negative number');
    return n;
  };
  // WebIDL shape: enumerable members and a @@toStringTag.
  const idl = (C, name) => {
    for (const target of [C.prototype, C]) {
      for (const key of getOwnPropertyNames(target)) {
        if (key === 'constructor' || key === 'prototype' || key === 'length' || key === 'name') continue;
        const desc = Object.getOwnPropertyDescriptor(target, key);
        desc.enumerable = true;
        defineProperty(target, key, desc);
      }
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  };
  const illegal = () => new TypeErrorC('Illegal constructor');
  const result = (value, done) => ({ value, done });
  const released = (what) => new TypeErrorC(`Cannot ${what} a stream using a released reader`);
  const invalid = (name, method) => new TypeErrorC(`Failed to execute '${method}' on '${name}': Illegal invocation`);

  // ----- ReadableStream operations --------------------------------------

  const rsLocked = (st) => st.reader !== undefined;
  const rsNew = () => ({ state: 'readable', reader: undefined, storedError: undefined, disturbed: false, controller: undefined, wrapper: undefined });

  function rsClose(st, canceled = false) {
    st.state = 'closed';
    const rd = st.reader;
    if (!rd) return;
    rd.closed.resolve(undefined);
    // ReadableStreamClose settles a default reader's read requests; a BYOB
    // reader's read-into requests wait for respond(0), unless canceled.
    if (rd.byob && !canceled) return;
    const reqs = rd.reqs;
    rd.reqs = [];
    for (let i = 0; i < reqs.length; i++) reqs[i].close(undefined);
  }
  function rsError(st, e) {
    st.state = 'errored';
    st.storedError = e;
    const rd = st.reader;
    if (!rd) return;
    rd.closed.reject(e);
    handled(rd.closed.promise);
    const reqs = rd.reqs;
    rd.reqs = [];
    for (let i = 0; i < reqs.length; i++) reqs[i].error(e);
  }
  function rsCancel(st, reason) {
    st.disturbed = true;
    if (st.state === 'closed') return resolved();
    if (st.state === 'errored') return rejected(st.storedError);
    rsClose(st, true);
    const c = st.controller;
    c.queue = [];
    c.queueSize = 0;
    if (c.byte) bcClearPending(c);
    const r = c.cancelAlg(reason);
    rcClear(c);
    return then(r, noop);
  }

  // Controller (default and byte share one shape; `byte` selects behavior).
  function rcSetup(st, byte, start, pullAlg, cancelAlg, hwm, size, autoAllocate) {
    const c = {
      stream: st, byte, queue: [], queueSize: 0, started: false, closeRequested: false, pullAgain: false, pulling: false,
      hwm, size, pullAlg, cancelAlg, autoAllocate, byobRequest: null, pending: [], wrapper: undefined,
    };
    st.controller = c;
    c.wrapper = byte ? new ReadableByteStreamController(token, c) : new ReadableStreamDefaultController(token, c);
    if (start) {
      then(resolved(start(c.wrapper)), () => {
        c.started = true;
        rcCallPull(c);
      }, (e) => rcError(c, e));
    } else {
      c.started = true;
    }
    return c;
  }
  const rcClear = (c) => {
    c.pullAlg = undefined;
    c.cancelAlg = undefined;
    c.size = undefined;
  };
  const rcDesired = (c) => {
    const state = c.stream.state;
    if (state === 'errored') return null;
    if (state === 'closed') return 0;
    return c.hwm - c.queueSize;
  };
  const rcCanClose = (c) => !c.closeRequested && c.stream.state === 'readable';
  function rcShouldPull(c) {
    if (!rcCanClose(c) || !c.started) return false;
    const rd = c.stream.reader;
    if (rd && rd.reqs.length > 0) return true;
    return rcDesired(c) > 0;
  }
  function rcCallPull(c) {
    if (!rcShouldPull(c)) return;
    if (c.pulling) {
      c.pullAgain = true;
      return;
    }
    c.pulling = true;
    then(c.pullAlg(), () => {
      c.pulling = false;
      if (c.pullAgain) {
        c.pullAgain = false;
        rcCallPull(c);
      }
    }, (e) => rcError(c, e));
  }
  function rcError(c, e) {
    const st = c.stream;
    if (st.state !== 'readable') return;
    if (c.byte) bcClearPending(c);
    c.queue = [];
    c.queueSize = 0;
    rcClear(c);
    rsError(st, e);
  }
  function rcClose(c) {
    if (!rcCanClose(c)) return;
    c.closeRequested = true;
    if (c.queue.length === 0) {
      rcClear(c);
      rsClose(c.stream);
    }
  }
  function rcEnqueue(c, chunk) {
    if (!rcCanClose(c)) return;
    const st = c.stream;
    const rd = st.reader;
    if (rd && rd.reqs.length > 0) {
      rd.reqs.shift().chunk(chunk);
    } else {
      let size;
      try {
        size = checkSize(c.size(chunk));
      } catch (e) {
        rcError(c, e);
        throw e;
      }
      c.queue.push({ value: chunk, size });
      c.queueSize += size;
    }
    rcCallPull(c);
  }
  function rcPullSteps(c, req) {
    if (c.byte) {
      // ReadableByteStreamController [[PullSteps]].
      if (c.queueSize > 0) {
        bcFillReadRequest(c, req);
        return;
      }
      const size = c.autoAllocate;
      if (size !== undefined) {
        let buffer;
        try {
          buffer = new ArrayBuffer(size);
        } catch (e) {
          req.error(e);
          return;
        }
        c.pending.push({
          buffer, bufferByteLength: size, byteOffset: 0, byteLength: size, bytesFilled: 0,
          minimumFill: 1, elementSize: 1, ctor: Uint8ArrayC, readerType: 'default',
        });
      }
      c.stream.reader.reqs.push(req);
      rcCallPull(c);
      return;
    }
    if (c.queue.length > 0) {
      const { value, size } = c.queue.shift();
      c.queueSize -= size;
      if (c.queueSize < 0) c.queueSize = 0;
      if (c.closeRequested && c.queue.length === 0) {
        rcClear(c);
        rsClose(c.stream);
      } else {
        rcCallPull(c);
      }
      req.chunk(value);
    } else {
      c.stream.reader.reqs.push(req);
      rcCallPull(c);
    }
  }

  // Byte streams (ReadableByteStreamController). A queue entry's `value` is a
  // Uint8Array over a buffer the controller owns. `c.pending` holds the
  // pull-into descriptors: {buffer, bufferByteLength, byteOffset, byteLength,
  // bytesFilled, minimumFill, elementSize, ctor, readerType} with readerType
  // 'default', 'byob' or 'none' (its reader was released).
  const bcPush = (c, bytes) => {
    c.queue.push({ value: bytes, size: bytes.length });
    c.queueSize += bytes.length;
  };
  // ReadableByteStreamControllerEnqueueClonedChunkToQueue.
  const bcPushClone = (c, buffer, offset, length) => {
    const copy = new Uint8ArrayC(length);
    copy.set(new Uint8ArrayC(buffer, offset, length));
    bcPush(c, copy);
  };
  function bcInvalidate(c) {
    const req = c.byobRequest;
    if (!req) return;
    ReadableStreamBYOBRequest$c.set(req, undefined);
    ReadableStreamBYOBRequest$view.set(req, null);
    c.byobRequest = null;
  }
  const bcClearPending = (c) => {
    bcInvalidate(c);
    c.pending = [];
  };
  // ReadableByteStreamControllerHandleQueueDrain.
  function bcDrain(c) {
    if (c.queueSize === 0 && c.closeRequested) {
      rcClear(c);
      rsClose(c.stream);
    } else {
      rcCallPull(c);
    }
  }
  // ReadableByteStreamControllerEnqueueDetachedPullIntoToQueue.
  function bcEnqueueDetached(c, d) {
    if (d.bytesFilled > 0) bcPushClone(c, d.buffer, d.byteOffset, d.bytesFilled);
    c.pending.shift();
  }
  // ReadableByteStreamControllerFillPullIntoDescriptorFromQueue.
  function bcFillFromQueue(c, d) {
    const maxCopy = Math.min(c.queueSize, d.byteLength - d.bytesFilled);
    const maxFilled = d.bytesFilled + maxCopy;
    const aligned = maxFilled - (maxFilled % d.elementSize);
    let remaining = maxCopy;
    let ready = false;
    if (aligned >= d.minimumFill) {
      remaining = aligned - d.bytesFilled;
      ready = true;
    }
    while (remaining > 0) {
      const head = c.queue[0];
      const n = Math.min(remaining, head.size);
      new Uint8ArrayC(d.buffer, d.byteOffset + d.bytesFilled, n).set(n === head.size ? head.value : head.value.subarray(0, n));
      if (n === head.size) {
        c.queue.shift();
      } else {
        head.value = head.value.subarray(n);
        head.size -= n;
      }
      c.queueSize -= n;
      d.bytesFilled += n;
      remaining -= n;
    }
    return ready;
  }
  // ReadableByteStreamControllerProcessPullIntoDescriptorsUsingQueue.
  function bcProcessPending(c) {
    const filled = [];
    while (c.pending.length > 0 && c.queueSize > 0) {
      if (bcFillFromQueue(c, c.pending[0])) filled.push(c.pending.shift());
    }
    return filled;
  }
  // ReadableByteStreamControllerConvertPullIntoDescriptor.
  const bcConvert = (d) => new d.ctor(transfer(d.buffer), d.byteOffset, d.bytesFilled / d.elementSize);
  // ReadableByteStreamControllerCommitPullIntoDescriptor: fulfills the
  // reader's first request (read request or read-into request).
  function bcCommit(c, d) {
    const st = c.stream;
    const done = st.state === 'closed';
    const view = bcConvert(d);
    const rd = st.reader;
    if (!rd || rd.reqs.length === 0) return;
    const req = rd.reqs.shift();
    if (d.readerType === 'default') {
      if (done) req.close();
      else req.chunk(view);
    } else if (done) {
      req.close(view);
    } else {
      req.chunk(view);
    }
  }
  const bcCommitAll = (c, filled) => {
    for (let i = 0; i < filled.length; i++) bcCommit(c, filled[i]);
  };
  // ReadableByteStreamControllerFillReadRequestFromQueue.
  function bcFillReadRequest(c, req) {
    const { value, size } = c.queue.shift();
    c.queueSize -= size;
    if (c.queueSize < 0) c.queueSize = 0;
    bcDrain(c);
    req.chunk(value);
  }
  // ReadableByteStreamControllerEnqueue, over bytes the controller owns
  // (the public enqueue() transfers the chunk's buffer first).
  function bcEnqueue(c, bytes) {
    if (!rcCanClose(c)) return;
    if (c.pending.length > 0) {
      const first = c.pending[0];
      if (isDetached(first.buffer)) throw new TypeErrorC('The BYOB request\'s buffer has been detached and so cannot be filled with an enqueued chunk');
      bcInvalidate(c);
      first.buffer = transfer(first.buffer);
      if (first.readerType === 'none') bcEnqueueDetached(c, first);
    }
    const rd = c.stream.reader;
    if (rd && !rd.byob) {
      while (rd.reqs.length > 0 && c.queueSize > 0) bcFillReadRequest(c, rd.reqs.shift());
      if (rd.reqs.length === 0) {
        bcPush(c, bytes);
      } else {
        if (c.pending.length > 0) c.pending.shift();
        rd.reqs.shift().chunk(bytes);
      }
    } else if (rd && rd.byob) {
      bcPush(c, bytes);
      bcCommitAll(c, bcProcessPending(c));
    } else {
      bcPush(c, bytes);
    }
    rcCallPull(c);
  }
  function bcClose(c) {
    if (!rcCanClose(c)) return;
    if (c.queueSize > 0) {
      c.closeRequested = true;
      return;
    }
    if (c.pending.length > 0) {
      const first = c.pending[0];
      if (first.bytesFilled % first.elementSize !== 0) {
        const e = new TypeErrorC('Insufficient bytes to fill elements in the given buffer');
        rcError(c, e);
        throw e;
      }
    }
    c.closeRequested = true;
    rcClear(c);
    rsClose(c.stream);
  }
  // ReadableByteStreamControllerPullInto. `buffer` is the view's buffer,
  // not yet transferred.
  function bcPullInto(c, buffer, byteOffset, byteLength, ctor, elementSize, min, req) {
    try {
      buffer = transfer(buffer);
    } catch (e) {
      req.error(e);
      return;
    }
    const d = {
      buffer, bufferByteLength: apply(bufferLength, buffer, []), byteOffset, byteLength, bytesFilled: 0,
      minimumFill: min * elementSize, elementSize, ctor, readerType: 'byob',
    };
    const st = c.stream;
    if (c.pending.length > 0) {
      c.pending.push(d);
      st.reader.reqs.push(req);
      return;
    }
    if (st.state === 'closed') {
      req.close(new ctor(d.buffer, d.byteOffset, 0));
      return;
    }
    if (c.queueSize > 0) {
      if (bcFillFromQueue(c, d)) {
        const view = bcConvert(d);
        bcDrain(c);
        req.chunk(view);
        return;
      }
      if (c.closeRequested) {
        const e = new TypeErrorC('Insufficient bytes to fill elements in the given buffer');
        rcError(c, e);
        req.error(e);
        return;
      }
    }
    c.pending.push(d);
    st.reader.reqs.push(req);
    rcCallPull(c);
  }
  // ReadableByteStreamControllerRespondInternal.
  function bcRespondInternal(c, bytesWritten) {
    const first = c.pending[0];
    bcInvalidate(c);
    const st = c.stream;
    if (st.state === 'closed') {
      // RespondInClosedState.
      if (first.readerType === 'none') c.pending.shift();
      const rd = st.reader;
      if (rd && rd.byob) {
        while (rd.reqs.length > 0 && c.pending.length > 0) bcCommit(c, c.pending.shift());
      }
    } else {
      // RespondInReadableState.
      first.bytesFilled += bytesWritten;
      if (first.readerType === 'none') {
        bcEnqueueDetached(c, first);
        bcCommitAll(c, bcProcessPending(c));
      } else if (first.bytesFilled >= first.minimumFill) {
        c.pending.shift();
        const remainder = first.bytesFilled % first.elementSize;
        if (remainder > 0) bcPushClone(c, first.buffer, first.byteOffset + first.bytesFilled - remainder, remainder);
        first.bytesFilled -= remainder;
        const filled = bcProcessPending(c);
        bcCommit(c, first);
        bcCommitAll(c, filled);
      }
    }
    rcCallPull(c);
  }
  // ReadableByteStreamControllerRespond(c, 0) on a closed stream.
  function bcRespondClosed(c) {
    const first = c.pending[0];
    first.buffer = transfer(first.buffer);
    bcRespondInternal(c, 0);
  }
  // ReadableByteStreamControllerRespondWithNewView, for byte tee (the public
  // respondWithNewView() checks the same and throws).
  function bcRespondWithNewView(c, view) {
    const { buffer, length } = viewParts(view);
    bcInvalidate(c);
    c.pending[0].buffer = transfer(buffer);
    bcRespondInternal(c, length);
  }
  // ReadableByteStreamControllerGetBYOBRequest.
  function bcRequest(c) {
    if (c.byobRequest === null && c.pending.length > 0) {
      const d = c.pending[0];
      const view = new Uint8ArrayC(d.buffer, d.byteOffset + d.bytesFilled, d.byteLength - d.bytesFilled);
      c.byobRequest = new ReadableStreamBYOBRequest(token, c, view);
    }
    return c.byobRequest;
  }

  // Readers. A read request is {chunk(value), close(), error(e)}.
  function readerInit(rd, st) {
    rd.stream = st;
    st.reader = rd;
    if (st.state === 'readable') {
      rd.closed = defer();
    } else if (st.state === 'closed') {
      rd.closed = settledDefer(resolved());
    } else {
      rd.closed = settledDefer(handled(rejected(st.storedError)));
    }
  }
  const newReader = (st) => {
    const rd = { stream: undefined, closed: undefined, reqs: [], byob: false };
    readerInit(rd, st);
    return rd;
  };
  function readerRead(rd, req) {
    const st = rd.stream;
    st.disturbed = true;
    if (st.state === 'closed') req.close();
    else if (st.state === 'errored') req.error(st.storedError);
    else rcPullSteps(st.controller, req);
  }
  function readerRelease(rd) {
    const st = rd.stream;
    const e = new TypeErrorC('Reader was released');
    if (st.state === 'readable') {
      rd.closed.reject(e);
    } else {
      rd.closed = settledDefer(rejected(e));
    }
    handled(rd.closed.promise);
    // ReadableByteStreamController [[ReleaseSteps]]: keep only the first
    // pull-into, now without a reader.
    const c = st.controller;
    if (c.byte && c.pending.length > 0) {
      const first = c.pending[0];
      first.readerType = 'none';
      c.pending = [first];
    }
    st.reader = undefined;
    rd.stream = undefined;
    const reqs = rd.reqs;
    rd.reqs = [];
    for (let i = 0; i < reqs.length; i++) reqs[i].error(e);
  }
  const makeStream = (byte, start, pullAlg, cancelAlg, hwm, size) => {
    const st = rsNew();
    st.wrapper = new ReadableStream(token, st);
    rcSetup(st, byte, start, pullAlg, cancelAlg, hwm, size);
    return st;
  };

  // CloneAsUint8Array.
  const cloneBytes = (view) => {
    const { buffer, offset, length } = viewParts(view);
    const copy = new Uint8ArrayC(length);
    copy.set(new Uint8ArrayC(buffer, offset, length));
    return copy;
  };
  // ReadableByteStreamTee: each pull reads with a default reader, or with a
  // BYOB reader into the branch's own byobRequest view, switching the
  // source's reader as needed.
  function rsByteTee(st) {
    let rd = newReader(st);
    let reading = false;
    let readAgain1 = false;
    let readAgain2 = false;
    let canceled1 = false;
    let canceled2 = false;
    let reason1;
    let reason2;
    let c1;
    let c2;
    const cancelP = defer();
    const fail = (error) => {
      rcError(c1, error);
      rcError(c2, error);
      cancelP.resolve(rsCancel(st, error));
    };
    const forwardError = (thisReader) => {
      then(thisReader.closed.promise, undefined, (e) => {
        if (thisReader !== rd) return;
        rcError(c1, e);
        rcError(c2, e);
        if (!canceled1 || !canceled2) cancelP.resolve();
      });
    };
    const switchReader = (byob) => {
      readerRelease(rd);
      rd = newReader(st);
      rd.byob = byob;
      forwardError(rd);
    };
    const readAgain = () => {
      if (readAgain1) pull1();
      else if (readAgain2) pull2();
    };
    const pullWithDefault = () => {
      if (rd.byob) switchReader(false);
      readerRead(rd, {
        chunk(chunk) {
          microtask(() => {
            readAgain1 = false;
            readAgain2 = false;
            let chunk2 = chunk;
            if (!canceled1 && !canceled2) {
              try {
                chunk2 = cloneBytes(chunk);
              } catch (error) {
                fail(error);
                return;
              }
            }
            if (!canceled1) bcEnqueue(c1, chunk);
            if (!canceled2) bcEnqueue(c2, chunk2);
            reading = false;
            readAgain();
          });
        },
        close() {
          reading = false;
          if (!canceled1) bcClose(c1);
          if (!canceled2) bcClose(c2);
          if (!canceled1 && c1.pending.length > 0) bcRespondClosed(c1);
          if (!canceled2 && c2.pending.length > 0) bcRespondClosed(c2);
          if (!canceled1 || !canceled2) cancelP.resolve();
        },
        error() {
          reading = false;
        },
      });
    };
    const pullWithBYOB = (view, forBranch2) => {
      if (!rd.byob) switchReader(true);
      const byobC = forBranch2 ? c2 : c1;
      const otherC = forBranch2 ? c1 : c2;
      const byobCanceled = () => (forBranch2 ? canceled2 : canceled1);
      const otherCanceled = () => (forBranch2 ? canceled1 : canceled2);
      const req = {
        chunk(chunk) {
          microtask(() => {
            readAgain1 = false;
            readAgain2 = false;
            if (!otherCanceled()) {
              let cloned;
              try {
                cloned = cloneBytes(chunk);
              } catch (error) {
                fail(error);
                return;
              }
              if (!byobCanceled()) bcRespondWithNewView(byobC, chunk);
              bcEnqueue(otherC, cloned);
            } else if (!byobCanceled()) {
              bcRespondWithNewView(byobC, chunk);
            }
            reading = false;
            readAgain();
          });
        },
        close(chunk) {
          reading = false;
          if (!byobCanceled()) bcClose(byobC);
          if (!otherCanceled()) bcClose(otherC);
          if (chunk !== undefined) {
            if (!byobCanceled()) bcRespondWithNewView(byobC, chunk);
            if (!otherCanceled() && otherC.pending.length > 0) bcRespondClosed(otherC);
          }
          if (!byobCanceled() || !otherCanceled()) cancelP.resolve();
        },
        error() {
          reading = false;
        },
      };
      // ReadableStreamBYOBReaderRead(reader, view, 1, req); the view is the
      // branch's byobRequest view, a Uint8Array.
      st.disturbed = true;
      if (st.state === 'errored') {
        req.error(st.storedError);
        return;
      }
      const { buffer, offset, length } = viewParts(view);
      bcPullInto(st.controller, buffer, offset, length, Uint8ArrayC, 1, 1, req);
    };
    const pullFor = (forBranch2) => () => {
      if (reading) {
        if (forBranch2) readAgain2 = true;
        else readAgain1 = true;
        return resolved();
      }
      reading = true;
      const request = bcRequest(forBranch2 ? c2 : c1);
      if (request === null) pullWithDefault();
      else pullWithBYOB(ReadableStreamBYOBRequest$view.get(request), forBranch2);
      return resolved();
    };
    const pull1 = pullFor(false);
    const pull2 = pullFor(true);
    const cancel = (first) => (reason) => {
      if (first) {
        canceled1 = true;
        reason1 = reason;
      } else {
        canceled2 = true;
        reason2 = reason;
      }
      if (canceled1 && canceled2) cancelP.resolve(rsCancel(st, [reason1, reason2]));
      return cancelP.promise;
    };
    const b1 = makeStream(true, noop, pull1, cancel(true), 0, () => 1);
    const b2 = makeStream(true, noop, pull2, cancel(false), 0, () => 1);
    c1 = b1.controller;
    c2 = b2.controller;
    forwardError(rd);
    return [b1.wrapper, b2.wrapper];
  }

  function rsTee(st, cloneForBranch2) {
    if (st.controller.byte) return rsByteTee(st);
    const rd = newReader(st);
    let reading = false;
    let readAgain = false;
    let canceled1 = false;
    let canceled2 = false;
    let reason1;
    let reason2;
    let c1;
    let c2;
    const cancelP = defer();
    const pull = () => {
      if (reading) {
        readAgain = true;
        return resolved();
      }
      reading = true;
      readerRead(rd, {
        chunk(value) {
          microtask(() => {
            readAgain = false;
            let value2 = value;
            if (!canceled2) {
              try {
                // Fetch clones default-stream chunks for branch 2, even if
                // branch 1 canceled. Public tee() shares default chunks.
                if (cloneForBranch2) value2 = cloneForBranch2(value);
              } catch (error) {
                rcError(c1, error);
                rcError(c2, error);
                cancelP.resolve(rsCancel(st, error));
                return;
              }
            }
            if (!canceled1) rcEnqueue(c1, value);
            if (!canceled2) rcEnqueue(c2, value2);
            reading = false;
            if (readAgain) pull();
          });
        },
        close() {
          reading = false;
          if (!canceled1) rcClose(c1);
          if (!canceled2) rcClose(c2);
          if (!canceled1 || !canceled2) cancelP.resolve();
        },
        error() {
          reading = false;
        },
      });
      return resolved();
    };
    const cancel = (first) => (reason) => {
      if (first) {
        canceled1 = true;
        reason1 = reason;
      } else {
        canceled2 = true;
        reason2 = reason;
      }
      if (canceled1 && canceled2) cancelP.resolve(rsCancel(st, [reason1, reason2]));
      return cancelP.promise;
    };
    const b1 = makeStream(false, noop, pull, cancel(true), 1, () => 1);
    const b2 = makeStream(false, noop, pull, cancel(false), 1, () => 1);
    c1 = b1.controller;
    c2 = b2.controller;
    then(rd.closed.promise, undefined, (e) => {
      rcError(c1, e);
      rcError(c2, e);
      if (!canceled1 || !canceled2) cancelP.resolve();
    });
    return [b1.wrapper, b2.wrapper];
  }

  function rsPipe(src, dest, preventClose, preventAbort, preventCancel, signal) {
    const reader = newReader(src);
    const writer = newWriter(dest);
    src.disturbed = true;
    let shuttingDown = false;
    let currentWrite = resolved();
    const done = defer();
    let onAbort;
    const finalize = (isError, error) => {
      writerRelease(writer);
      readerRelease(reader);
      if (onAbort) apply(removeListener, signal, ['abort', onAbort]);
      if (isError) done.reject(error);
      else done.resolve(undefined);
    };
    const waitForWrites = () => {
      const old = currentWrite;
      return then(old, () => (old !== currentWrite ? waitForWrites() : undefined));
    };
    const shutdown = (action, isError, error) => {
      if (shuttingDown) return;
      shuttingDown = true;
      const act = () => {
        if (!action) return finalize(isError, error);
        then(action(), () => finalize(isError, error), (e) => finalize(true, e));
      };
      if (dest.state === 'writable' && !wsCloseQueued(dest)) then(waitForWrites(), act);
      else act();
    };
    if (signal !== undefined) {
      const abort = () => {
        const error = signal.reason;
        shutdown(() => {
          const actions = [];
          if (!preventAbort && dest.state === 'writable') actions.push(wsAbort(dest, error));
          if (!preventCancel && src.state === 'readable') actions.push(rsCancel(src, error));
          return promiseAll(actions);
        }, true, error);
      };
      if (signal.aborted) {
        abort();
        return done.promise;
      }
      onAbort = abort;
      apply(addListener, signal, ['abort', onAbort]);
    }
    const whenErrored = (st, closed, f) => {
      if (st.state === 'errored') f(st.storedError);
      else then(closed, undefined, f);
    };
    whenErrored(src, reader.closed.promise, (e) => {
      shutdown(preventAbort ? undefined : () => wsAbort(dest, e), true, e);
    });
    whenErrored(dest, writer.closed.promise, (e) => {
      shutdown(preventCancel ? undefined : () => rsCancel(src, e), true, e);
    });
    const onSourceClosed = () => {
      shutdown(preventClose ? undefined : () => writerCloseWithFlush(writer));
    };
    if (src.state === 'closed') onSourceClosed();
    else then(reader.closed.promise, onSourceClosed, noop);
    if (wsCloseQueued(dest) || dest.state === 'closed') {
      const e = new TypeErrorC('The destination writable stream closed before all data could be piped to it.');
      shutdown(preventCancel ? undefined : () => rsCancel(src, e), true, e);
    }
    const step = () => {
      if (shuttingDown) return;
      then(writer.ready.promise, () => {
        if (shuttingDown) return;
        readerRead(reader, {
          chunk(value) {
            currentWrite = then(writerWrite(writer, value), undefined, noop);
            step();
          },
          close: noop,
          error: noop,
        });
      }, noop);
    };
    step();
    return done.promise;
  }

  // ----- ReadableStream classes ------------------------------------------

  let rsOf;
  let isRS;
  const ReadableStream$s = slot('ReadableStream.s');
  class ReadableStream {
    static {
      rsOf = (v) => ReadableStream$s.get(v);
      isRS = (v) => ReadableStream$s.has(v);
    }
    constructor(source = undefined, strategy = undefined) {
      if (source === token) {
        ReadableStream$s.set(this, strategy);
        return;
      }
      if (source !== undefined && !isObject(source)) {
        throw new TypeErrorC("Failed to construct 'ReadableStream': The provided value is not of type 'object'.");
      }
      const strat = strategyOf(strategy, 'ReadableStream');
      const d = dictionary(source, 'ReadableStream');
      const autoRaw = d.autoAllocateChunkSize;
      const auto = autoRaw === undefined ? undefined : NumberC(autoRaw);
      const cancel = callback(d, 'cancel', 'ReadableStream');
      const pull = callback(d, 'pull', 'ReadableStream');
      const start = callback(d, 'start', 'ReadableStream');
      const typeRaw = d.type;
      const type = typeRaw === undefined ? undefined : `${typeRaw}`;
      if (type !== undefined && type !== 'bytes') {
        throw new TypeErrorC(`Failed to construct 'ReadableStream': The provided value '${type}' is not a valid enum value of type ReadableStreamType.`);
      }
      const byte = type === 'bytes';
      if (byte && strat.size !== undefined) throw new RangeErrorC('The strategy for a byte stream cannot have a size function');
      if (byte && auto !== undefined && !(auto > 0)) throw new TypeErrorC('autoAllocateChunkSize must be greater than 0');
      const hwm = highWaterMark(strat, byte ? 0 : 1);
      const st = rsNew();
      st.wrapper = this;
      ReadableStream$s.set(this, st);
      const startAlg = (w) => (start ? apply(start, d, [w]) : undefined);
      rcSetup(
        st,
        byte,
        startAlg,
        pull ? () => call(pull, d, [st.controller.wrapper]) : () => resolved(),
        cancel ? (r) => call(cancel, d, [r]) : () => resolved(),
        hwm,
        byte ? () => 1 : sizeAlgorithm(strat),
        auto,
      );
    }
    get locked() {
      if (!isRS(this)) throw invalid('ReadableStream', 'locked');
      return rsLocked(ReadableStream$s.get(this));
    }
    cancel(reason = undefined) {
      if (!isRS(this)) return rejected(invalid('ReadableStream', 'cancel'));
      if (rsLocked(ReadableStream$s.get(this))) return rejected(new TypeErrorC('Cannot cancel a locked stream'));
      return rsCancel(ReadableStream$s.get(this), reason);
    }
    getReader(options = undefined) {
      if (!isRS(this)) throw invalid('ReadableStream', 'getReader');
      const d = dictionary(options, 'getReader');
      const modeRaw = d.mode;
      const mode = modeRaw === undefined ? undefined : `${modeRaw}`;
      if (mode === undefined) return new ReadableStreamDefaultReader(this);
      if (mode !== 'byob') throw new TypeErrorC(`Failed to execute 'getReader' on 'ReadableStream': The provided value '${mode}' is not a valid enum value of type ReadableStreamReaderMode.`);
      return new ReadableStreamBYOBReader(this);
    }
    pipeThrough(transform, options = undefined) {
      if (!isRS(this)) throw invalid('ReadableStream', 'pipeThrough');
      const pair = dictionary(transform, 'pipeThrough');
      const readable = pair.readable;
      if (!isRS(readable)) throw new TypeErrorC("Failed to execute 'pipeThrough' on 'ReadableStream': readable is not a ReadableStream");
      const writable = pair.writable;
      if (!isWS(writable)) throw new TypeErrorC("Failed to execute 'pipeThrough' on 'ReadableStream': writable is not a WritableStream");
      const o = pipeOptions(options);
      if (rsLocked(ReadableStream$s.get(this))) throw new TypeErrorC('Cannot pipe a locked stream');
      if (wsOf(writable).writer) throw new TypeErrorC('Cannot pipe to a locked stream');
      handled(rsPipe(ReadableStream$s.get(this), wsOf(writable), o.preventClose, o.preventAbort, o.preventCancel, o.signal));
      return readable;
    }
    pipeTo(destination, options = undefined) {
      if (!isRS(this)) return rejected(invalid('ReadableStream', 'pipeTo'));
      if (!isWS(destination)) return rejected(new TypeErrorC("Failed to execute 'pipeTo' on 'ReadableStream': parameter 1 is not of type 'WritableStream'."));
      let o;
      try {
        o = pipeOptions(options);
      } catch (e) {
        return rejected(e);
      }
      if (rsLocked(ReadableStream$s.get(this))) return rejected(new TypeErrorC('Cannot pipe a locked stream'));
      if (wsOf(destination).writer) return rejected(new TypeErrorC('Cannot pipe to a locked stream'));
      return rsPipe(ReadableStream$s.get(this), wsOf(destination), o.preventClose, o.preventAbort, o.preventCancel, o.signal);
    }
    tee() {
      if (!isRS(this)) throw invalid('ReadableStream', 'tee');
      if (rsLocked(ReadableStream$s.get(this))) throw new TypeErrorC('Cannot tee a locked stream');
      return rsTee(ReadableStream$s.get(this));
    }
    values(options = undefined) {
      if (!isRS(this)) throw invalid('ReadableStream', 'values');
      const d = dictionary(options, 'values');
      const preventCancel = !!d.preventCancel;
      if (rsLocked(ReadableStream$s.get(this))) throw new TypeErrorC('Cannot iterate a locked stream');
      const rd = newReader(ReadableStream$s.get(this));
      const it = Object.create(StreamIteratorPrototype);
      iterators.set(it, { rd, preventCancel, finished: false, ongoing: undefined });
      return it;
    }
    static from(iterable) {
      if (!isObject(iterable)) throw new TypeErrorC("Failed to execute 'from' on 'ReadableStream': The provided value is not an object.");
      const method = iterable[asyncIterator];
      const getter = method === undefined || method === null ? iterable[iterator] : method;
      if (typeof getter !== 'function') throw new TypeErrorC("Failed to execute 'from' on 'ReadableStream': The provided value is not iterable.");
      const it = apply(getter, iterable, []);
      if (!isObject(it)) throw new TypeErrorC("Failed to execute 'from' on 'ReadableStream': The iterator is not an object.");
      const next = it.next;
      const st = makeStream(false, noop, () => then(call(next, it, []), (r) => {
        if (!isObject(r)) throw new TypeErrorC('The iterator result is not an object');
        if (r.done) return rcClose(st.controller);
        return then(resolved(r.value), (v) => rcEnqueue(st.controller, v));
      }), (reason) => {
        let ret;
        try {
          ret = it.return;
        } catch (e) {
          return rejected(e);
        }
        if (ret === undefined || ret === null) return resolved();
        return then(call(ret, it, [reason]), (r) => {
          if (!isObject(r)) throw new TypeErrorC('The iterator result is not an object');
        });
      }, 0, () => 1);
      return st.wrapper;
    }
  }
  defineProperty(ReadableStream.prototype, asyncIterator, { value: ReadableStream.prototype.values, writable: true, configurable: true });
  const pipeOptions = (options) => {
    const d = dictionary(options, 'pipeTo');
    const preventAbort = !!d.preventAbort;
    const preventCancel = !!d.preventCancel;
    const preventClose = !!d.preventClose;
    const signal = d.signal;
    if (signal !== undefined && !(signal instanceof AbortSignalC)) {
      throw new TypeErrorC("Failed to execute 'pipeTo' on 'ReadableStream': Failed to read the 'signal' property from 'StreamPipeOptions': The provided value is not of type 'AbortSignal'.");
    }
    return { preventAbort, preventCancel, preventClose, signal };
  };

  // Async iteration: each next() runs after the previous one settled.
  const iterators = new WeakMap();
  const iterNext = (state) => {
    if (state.finished) return resolved(result(undefined, true));
    const rd = state.rd;
    const d = defer();
    readerRead(rd, {
      chunk: (v) => d.resolve(result(v, false)),
      close: () => {
        state.finished = true;
        readerRelease(rd);
        d.resolve(result(undefined, true));
      },
      error: (e) => {
        state.finished = true;
        readerRelease(rd);
        d.reject(e);
      },
    });
    return d.promise;
  };
  const iterReturn = (state, value) => {
    if (state.finished) return resolved(result(value, true));
    state.finished = true;
    const rd = state.rd;
    const st = rd.stream;
    if (state.preventCancel) {
      readerRelease(rd);
      return resolved(result(value, true));
    }
    const cancel = rsCancel(st, value);
    readerRelease(rd);
    return then(cancel, () => result(value, true));
  };
  const StreamIteratorPrototype = Object.create(AsyncIteratorPrototype, {
    next: {
      value: function next() {
        const state = iterators.get(this);
        if (!state) return rejected(new TypeErrorC('Illegal invocation'));
        const run = () => iterNext(state);
        const p = state.ongoing ? then(state.ongoing, run, run) : run();
        state.ongoing = handled(p);
        return p;
      },
      writable: true, enumerable: true, configurable: true,
    },
    return: {
      value: function (value) {
        const state = iterators.get(this);
        if (!state) return rejected(new TypeErrorC('Illegal invocation'));
        const run = () => iterReturn(state, value);
        const p = state.ongoing ? then(state.ongoing, run, run) : run();
        state.ongoing = handled(p);
        return p;
      },
      writable: true, enumerable: true, configurable: true,
    },
    [Symbol.toStringTag]: { value: 'ReadableStream AsyncIterator', configurable: true },
  });

  const ReadableStreamDefaultController$c = slot('ReadableStreamDefaultController.c');
  class ReadableStreamDefaultController {
    constructor(t, c) {
      if (t !== token) throw illegal();
      ReadableStreamDefaultController$c.set(this, c);
    }
    get desiredSize() {
      return rcDesired(ReadableStreamDefaultController$c.get(this));
    }
    close() {
      if (!rcCanClose(ReadableStreamDefaultController$c.get(this))) throw new TypeErrorC('Cannot close a readable stream that is closed, errored or already closing');
      rcClose(ReadableStreamDefaultController$c.get(this));
    }
    enqueue(chunk = undefined) {
      if (!rcCanClose(ReadableStreamDefaultController$c.get(this))) throw new TypeErrorC('Cannot enqueue a chunk into a readable stream that is closed, errored or closing');
      rcEnqueue(ReadableStreamDefaultController$c.get(this), chunk);
    }
    error(e = undefined) {
      rcError(ReadableStreamDefaultController$c.get(this), e);
    }
  }
  // A view's [[ViewedArrayBuffer]], [[ByteOffset]] and [[ByteLength]], read
  // with the intrinsic getters.
  const viewParts = (view) => {
    const getters = apply(typedArrayName, view, []) === undefined ? dataView : typedView;
    return {
      buffer: apply(getters.buffer, view, []),
      offset: apply(getters.offset, view, []),
      length: apply(getters.length, view, []),
    };
  };
  // Web IDL [EnforceRange] unsigned long long.
  const enforceRange = (v, method) => {
    let n = NumberC(v);
    if (n !== n || n === Infinity || n === -Infinity) throw new TypeErrorC(`Failed to execute '${method}' on 'ReadableStreamBYOBRequest': Value is not of type 'unsigned long long'.`);
    n = Math.trunc(n);
    if (n < 0 || n > NumberC.MAX_SAFE_INTEGER) throw new TypeErrorC(`Failed to execute '${method}' on 'ReadableStreamBYOBRequest': Value is outside the 'unsigned long long' value range.`);
    return n === 0 ? 0 : n;
  };
  const ReadableByteStreamController$c = slot('ReadableByteStreamController.c');
  class ReadableByteStreamController {
    constructor(t, c) {
      if (t !== token) throw illegal();
      ReadableByteStreamController$c.set(this, c);
    }
    get byobRequest() {
      return bcRequest(ReadableByteStreamController$c.get(this));
    }
    get desiredSize() {
      return rcDesired(ReadableByteStreamController$c.get(this));
    }
    close() {
      if (!rcCanClose(ReadableByteStreamController$c.get(this))) throw new TypeErrorC('Cannot close a readable stream that is closed, errored or already closing');
      bcClose(ReadableByteStreamController$c.get(this));
    }
    enqueue(chunk) {
      if (!isView(chunk)) throw new TypeErrorC("Failed to execute 'enqueue' on 'ReadableByteStreamController': parameter 1 is not of type 'ArrayBufferView'.");
      if (chunk.byteLength === 0) throw new TypeErrorC('chunk must have a non-zero byteLength');
      if (chunk.buffer.byteLength === 0) throw new TypeErrorC('chunk has a detached buffer');
      const c = ReadableByteStreamController$c.get(this);
      if (!rcCanClose(c)) throw new TypeErrorC('Cannot enqueue a chunk into a readable stream that is closed, errored or closing');
      // ReadableByteStreamControllerEnqueue transfers the chunk's buffer.
      const { buffer, offset, length } = viewParts(chunk);
      bcEnqueue(c, new Uint8ArrayC(transfer(buffer), offset, length));
    }
    error(e = undefined) {
      rcError(ReadableByteStreamController$c.get(this), e);
    }
  }
  const ReadableStreamBYOBRequest$c = slot('ReadableStreamBYOBRequest.c');
  const ReadableStreamBYOBRequest$view = slot('ReadableStreamBYOBRequest.view');
  class ReadableStreamBYOBRequest {
    constructor(t, c, view) {
      if (t !== token) throw illegal();
      ReadableStreamBYOBRequest$c.set(this, c);
      ReadableStreamBYOBRequest$view.set(this, view);
    }
    get view() {
      return ReadableStreamBYOBRequest$view.get(this);
    }
    respond(bytesWritten) {
      const c = ReadableStreamBYOBRequest$c.get(this);
      const n = enforceRange(bytesWritten, 'respond');
      if (c === undefined) throw new TypeErrorC('This BYOB request has been invalidated');
      if (isDetached(viewParts(ReadableStreamBYOBRequest$view.get(this)).buffer)) throw new TypeErrorC('The BYOB request\'s buffer has been detached and so cannot be used as a response');
      // ReadableByteStreamControllerRespond.
      const first = c.pending[0];
      if (c.stream.state === 'closed') {
        if (n !== 0) throw new TypeErrorC('bytesWritten must be 0 when calling respond() on a closed stream');
      } else {
        if (n === 0) throw new TypeErrorC('bytesWritten must be greater than 0 when calling respond() on a readable stream');
        if (first.bytesFilled + n > first.byteLength) throw new RangeErrorC('bytesWritten out of range');
      }
      first.buffer = transfer(first.buffer);
      bcRespondInternal(c, n);
    }
    respondWithNewView(view) {
      const c = ReadableStreamBYOBRequest$c.get(this);
      if (!isView(view)) throw new TypeErrorC("Failed to execute 'respondWithNewView' on 'ReadableStreamBYOBRequest': parameter 1 is not of type 'ArrayBufferView'.");
      if (c === undefined) throw new TypeErrorC('This BYOB request has been invalidated');
      const { buffer, offset, length } = viewParts(view);
      if (isDetached(buffer)) throw new TypeErrorC('The given view\'s buffer has been detached and so cannot be used as a response');
      // ReadableByteStreamControllerRespondWithNewView.
      const first = c.pending[0];
      if (c.stream.state === 'closed') {
        if (length !== 0) throw new TypeErrorC('The view\'s length must be 0 when calling respondWithNewView() on a closed stream');
      } else if (length === 0) {
        throw new TypeErrorC('The view\'s length must be greater than 0 when calling respondWithNewView() on a readable stream');
      }
      if (first.byteOffset + first.bytesFilled !== offset) throw new RangeErrorC('The region specified by view does not match byobRequest');
      if (first.bufferByteLength !== apply(bufferLength, buffer, [])) throw new RangeErrorC('The buffer of view has different capacity than byobRequest');
      if (first.bytesFilled + length > first.byteLength) throw new RangeErrorC('The region specified by view is larger than byobRequest');
      first.buffer = transfer(buffer);
      bcRespondInternal(c, length);
    }
  }
  const ReadableStreamDefaultReader$r = slot('ReadableStreamDefaultReader.r');
  class ReadableStreamDefaultReader {
    static #is = (v) => ReadableStreamDefaultReader$r.has(v);
    constructor(stream) {
      if (!isRS(stream)) throw new TypeErrorC("Failed to construct 'ReadableStreamDefaultReader': parameter 1 is not of type 'ReadableStream'.");
      const st = rsOf(stream);
      if (rsLocked(st)) throw new TypeErrorC('ReadableStream is locked');
      ReadableStreamDefaultReader$r.set(this, newReader(st));
    }
    get closed() {
      if (!ReadableStreamDefaultReader.#is(this)) return rejected(invalid('ReadableStreamDefaultReader', 'closed'));
      return ReadableStreamDefaultReader$r.get(this).closed.promise;
    }
    cancel(reason = undefined) {
      if (!ReadableStreamDefaultReader.#is(this)) return rejected(invalid('ReadableStreamDefaultReader', 'cancel'));
      if (!ReadableStreamDefaultReader$r.get(this).stream) return rejected(released('cancel'));
      return rsCancel(ReadableStreamDefaultReader$r.get(this).stream, reason);
    }
    read() {
      if (!ReadableStreamDefaultReader.#is(this)) return rejected(invalid('ReadableStreamDefaultReader', 'read'));
      if (!ReadableStreamDefaultReader$r.get(this).stream) return rejected(released('read from'));
      const d = defer();
      readerRead(ReadableStreamDefaultReader$r.get(this), {
        chunk: (v) => d.resolve(result(v, false)),
        close: () => d.resolve(result(undefined, true)),
        error: (e) => d.reject(e),
      });
      return d.promise;
    }
    releaseLock() {
      if (!ReadableStreamDefaultReader.#is(this)) throw invalid('ReadableStreamDefaultReader', 'releaseLock');
      if (ReadableStreamDefaultReader$r.get(this).stream) readerRelease(ReadableStreamDefaultReader$r.get(this));
    }
  }
  const ReadableStreamBYOBReader$r = slot('ReadableStreamBYOBReader.r');
  class ReadableStreamBYOBReader {
    static #is = (v) => ReadableStreamBYOBReader$r.has(v);
    constructor(stream) {
      if (!isRS(stream)) throw new TypeErrorC("Failed to construct 'ReadableStreamBYOBReader': parameter 1 is not of type 'ReadableStream'.");
      const st = rsOf(stream);
      if (rsLocked(st)) throw new TypeErrorC('ReadableStream is locked');
      if (!st.controller.byte) throw new TypeErrorC('Cannot construct a ReadableStreamBYOBReader for a stream not constructed with a byte source');
      ReadableStreamBYOBReader$r.set(this, newReader(st));
      ReadableStreamBYOBReader$r.get(this).byob = true;
    }
    get closed() {
      if (!ReadableStreamBYOBReader.#is(this)) return rejected(invalid('ReadableStreamBYOBReader', 'closed'));
      return ReadableStreamBYOBReader$r.get(this).closed.promise;
    }
    cancel(reason = undefined) {
      if (!ReadableStreamBYOBReader.#is(this)) return rejected(invalid('ReadableStreamBYOBReader', 'cancel'));
      if (!ReadableStreamBYOBReader$r.get(this).stream) return rejected(released('cancel'));
      return rsCancel(ReadableStreamBYOBReader$r.get(this).stream, reason);
    }
    read(view, options = undefined) {
      if (!ReadableStreamBYOBReader.#is(this)) return rejected(invalid('ReadableStreamBYOBReader', 'read'));
      if (!isView(view)) return rejected(new TypeErrorC("Failed to execute 'read' on 'ReadableStreamBYOBReader': parameter 1 is not of type 'ArrayBufferView'."));
      let min = 1;
      try {
        const raw = dictionary(options, 'read').min;
        if (raw !== undefined) {
          min = Math.trunc(+raw);
          if (!(min >= 0 && min <= NumberC.MAX_SAFE_INTEGER)) throw new TypeErrorC('options.min is outside the unsigned long long range');
        }
      } catch (e) {
        return rejected(e);
      }
      const kind = apply(typedArrayName, view, []);
      const Ctor = kind === undefined ? DataViewC : typedCtors[kind];
      const getters = kind === undefined ? dataView : typedView;
      let buffer, offset, length;
      try {
        buffer = apply(getters.buffer, view, []);
        offset = apply(getters.offset, view, []);
        length = apply(getters.length, view, []);
      } catch (e) { return rejected(e); }
      if (length === 0) return rejected(new TypeErrorC('view must have non-zero byteLength'));
      if (isDetached(buffer)) return rejected(new TypeErrorC('view\'s buffer has been detached'));
      if (!(min > 0)) return rejected(new TypeErrorC('options.min must be greater than 0'));
      const element = kind === undefined ? 1 : Ctor.BYTES_PER_ELEMENT;
      const elements = length / element;
      if (min > elements) return rejected(new RangeErrorC('options.min must be less than or equal to view\'s length'));
      const rd = ReadableStreamBYOBReader$r.get(this);
      if (!rd.stream) return rejected(released('read from'));
      const st = rd.stream;
      st.disturbed = true;
      const d = defer();
      const req = {
        chunk: (v) => d.resolve(result(v, false)),
        close: (v) => d.resolve(result(v, true)),
        error: (e) => d.reject(e),
      };
      if (st.state === 'errored') req.error(st.storedError);
      else bcPullInto(st.controller, buffer, offset, length, Ctor, element, min, req);
      return d.promise;
    }
    releaseLock() {
      if (!ReadableStreamBYOBReader.#is(this)) throw invalid('ReadableStreamBYOBReader', 'releaseLock');
      if (ReadableStreamBYOBReader$r.get(this).stream) readerRelease(ReadableStreamBYOBReader$r.get(this));
    }
  }

  // ----- WritableStream --------------------------------------------------

  const closeSentinel = {};
  const wsCloseQueued = (st) => st.closeRequest !== undefined || st.inFlightClose !== undefined;
  const wsInFlight = (st) => st.inFlightWrite !== undefined || st.inFlightClose !== undefined;

  function wsAbort(st, reason) {
    if (st.state === 'closed' || st.state === 'errored') return resolved();
    apply(abortController, st.controller.abortController, [reason]);
    if (st.state === 'closed' || st.state === 'errored') return resolved();
    if (st.pendingAbort) return st.pendingAbort.d.promise;
    const already = st.state === 'erroring';
    const d = defer();
    st.pendingAbort = { d, reason: already ? undefined : reason, already };
    if (!already) wsStartErroring(st, reason);
    return d.promise;
  }
  function wsClose(st) {
    if (st.state === 'closed' || st.state === 'errored') return rejected(new TypeErrorC('Cannot close a stream that is closed or errored'));
    const d = defer();
    st.closeRequest = d;
    if (st.writer && st.backpressure && st.state === 'writable') st.writer.ready.resolve(undefined);
    const c = st.controller;
    c.queue.push({ value: closeSentinel, size: 0 });
    wcAdvance(c);
    return d.promise;
  }
  function wsDealWithRejection(st, e) {
    if (st.state === 'writable') wsStartErroring(st, e);
    else wsFinishErroring(st);
  }
  function wsStartErroring(st, reason) {
    st.state = 'erroring';
    st.storedError = reason;
    if (st.writer) writerEnsureReadyRejected(st.writer, reason);
    if (!wsInFlight(st) && st.controller.started) wsFinishErroring(st);
  }
  function wsRejectCloseAndClosed(st) {
    if (st.closeRequest) {
      st.closeRequest.reject(st.storedError);
      st.closeRequest = undefined;
    }
    if (st.writer) {
      st.writer.closed.reject(st.storedError);
      handled(st.writer.closed.promise);
    }
  }
  function wsFinishErroring(st) {
    st.state = 'errored';
    const c = st.controller;
    c.queue = [];
    c.queueSize = 0;
    const e = st.storedError;
    const requests = st.writeRequests;
    st.writeRequests = [];
    for (let i = 0; i < requests.length; i++) requests[i].reject(e);
    const abort = st.pendingAbort;
    st.pendingAbort = undefined;
    if (!abort) return wsRejectCloseAndClosed(st);
    if (abort.already) {
      abort.d.reject(e);
      return wsRejectCloseAndClosed(st);
    }
    const p = c.abortAlg(abort.reason);
    wcClear(c);
    then(p, () => {
      abort.d.resolve(undefined);
      wsRejectCloseAndClosed(st);
    }, (r) => {
      abort.d.reject(r);
      wsRejectCloseAndClosed(st);
    });
  }
  function wsUpdateBackpressure(st, backpressure) {
    if (st.writer && backpressure !== st.backpressure) {
      if (backpressure) st.writer.ready = defer();
      else st.writer.ready.resolve(undefined);
    }
    st.backpressure = backpressure;
  }

  function newWriter(st) {
    const w = { stream: st, ready: undefined, closed: undefined, wrapper: undefined };
    st.writer = w;
    if (st.state === 'writable') {
      w.ready = !wsCloseQueued(st) && st.backpressure ? defer() : settledDefer(resolved());
      w.closed = defer();
    } else if (st.state === 'erroring') {
      w.ready = settledDefer(handled(rejected(st.storedError)));
      w.closed = defer();
    } else if (st.state === 'closed') {
      w.ready = settledDefer(resolved());
      w.closed = settledDefer(resolved());
    } else {
      w.ready = settledDefer(handled(rejected(st.storedError)));
      w.closed = settledDefer(handled(rejected(st.storedError)));
    }
    return w;
  }
  function writerEnsureReadyRejected(w, e) {
    if (w.ready.settled) w.ready = settledDefer(rejected(e));
    else w.ready.reject(e);
    handled(w.ready.promise);
  }
  function writerEnsureClosedRejected(w, e) {
    if (w.closed.settled) w.closed = settledDefer(rejected(e));
    else w.closed.reject(e);
    handled(w.closed.promise);
  }
  function writerRelease(w) {
    const e = new TypeErrorC('Writer was released');
    writerEnsureReadyRejected(w, e);
    writerEnsureClosedRejected(w, e);
    w.stream.writer = undefined;
    w.stream = undefined;
  }
  function writerCloseWithFlush(w) {
    const st = w.stream;
    if (wsCloseQueued(st) || st.state === 'closed') return resolved();
    if (st.state === 'errored') return rejected(st.storedError);
    return wsClose(st);
  }
  function writerWrite(w, chunk) {
    const st = w.stream;
    const c = st.controller;
    let size;
    try {
      size = c.size(chunk);
    } catch (e) {
      wcErrorIfNeeded(c, e);
      size = 1;
    }
    if (st !== w.stream) return rejected(new TypeErrorC('Writer was released'));
    if (st.state === 'errored') return rejected(st.storedError);
    if (wsCloseQueued(st) || st.state === 'closed') return rejected(new TypeErrorC('The stream is closing or closed'));
    if (st.state === 'erroring') return rejected(st.storedError);
    const d = defer();
    st.writeRequests.push(d);
    try {
      const n = checkSize(size);
      c.queue.push({ value: chunk, size: n });
      c.queueSize += n;
    } catch (e) {
      wcErrorIfNeeded(c, e);
      return d.promise;
    }
    if (!wsCloseQueued(st) && st.state === 'writable') wsUpdateBackpressure(st, c.hwm - c.queueSize <= 0);
    wcAdvance(c);
    return d.promise;
  }

  const wcClear = (c) => {
    c.writeAlg = undefined;
    c.closeAlg = undefined;
    c.abortAlg = undefined;
    c.size = undefined;
  };
  function wcErrorIfNeeded(c, e) {
    if (c.stream.state === 'writable') {
      wcClear(c);
      wsStartErroring(c.stream, e);
    }
  }
  function wcAdvance(c) {
    const st = c.stream;
    if (!c.started || st.inFlightWrite !== undefined) return;
    if (st.state === 'erroring') return wsFinishErroring(st);
    if (st.state !== 'writable' && st.state !== 'erroring') return;
    if (c.queue.length === 0) return;
    const { value } = c.queue[0];
    if (value === closeSentinel) {
      st.inFlightClose = st.closeRequest;
      st.closeRequest = undefined;
      c.queue.shift();
      const p = c.closeAlg();
      wcClear(c);
      then(p, () => {
        st.inFlightClose.resolve(undefined);
        st.inFlightClose = undefined;
        if (st.state === 'erroring') {
          st.storedError = undefined;
          if (st.pendingAbort) {
            st.pendingAbort.d.resolve(undefined);
            st.pendingAbort = undefined;
          }
        }
        st.state = 'closed';
        if (st.writer) st.writer.closed.resolve(undefined);
      }, (e) => {
        st.inFlightClose.reject(e);
        st.inFlightClose = undefined;
        if (st.pendingAbort) {
          st.pendingAbort.d.reject(e);
          st.pendingAbort = undefined;
        }
        wsDealWithRejection(st, e);
      });
    } else {
      st.inFlightWrite = st.writeRequests.shift();
      then(c.writeAlg(value), () => {
        st.inFlightWrite.resolve(undefined);
        st.inFlightWrite = undefined;
        const { size } = c.queue.shift();
        c.queueSize -= size;
        if (c.queueSize < 0) c.queueSize = 0;
        if (!wsCloseQueued(st) && st.state === 'writable') wsUpdateBackpressure(st, c.hwm - c.queueSize <= 0);
        wcAdvance(c);
      }, (e) => {
        if (st.state === 'writable') wcClear(c);
        st.inFlightWrite.reject(e);
        st.inFlightWrite = undefined;
        wsDealWithRejection(st, e);
      });
    }
  }
  function wsSetup(st, start, writeAlg, closeAlg, abortAlg, hwm, size) {
    const c = {
      stream: st, queue: [], queueSize: 0, started: false, hwm, size, writeAlg, closeAlg, abortAlg,
      abortController: new AbortControllerC(), wrapper: undefined,
    };
    st.controller = c;
    c.wrapper = new WritableStreamDefaultController(token, c);
    st.backpressure = hwm <= 0;
    then(resolved(start(c.wrapper)), () => {
      c.started = true;
      wcAdvance(c);
    }, (e) => {
      c.started = true;
      wsDealWithRejection(st, e);
    });
    return c;
  }
  const wsNew = () => ({
    state: 'writable', storedError: undefined, writer: undefined, controller: undefined, inFlightWrite: undefined, closeRequest: undefined,
    inFlightClose: undefined, pendingAbort: undefined, writeRequests: [], backpressure: false, wrapper: undefined,
  });

  let wsOf;
  let isWS;
  const WritableStream$s = slot('WritableStream.s');
  class WritableStream {
    static {
      wsOf = (v) => WritableStream$s.get(v);
      isWS = (v) => WritableStream$s.has(v);
    }
    constructor(sink = undefined, strategy = undefined) {
      if (sink === token) {
        WritableStream$s.set(this, strategy);
        return;
      }
      const strat = strategyOf(strategy, 'WritableStream');
      if (sink !== undefined && sink !== null && !isObject(sink)) {
        throw new TypeErrorC("Failed to construct 'WritableStream': The provided value is not of type 'object'.");
      }
      const d = dictionary(sink, 'WritableStream');
      const abort = callback(d, 'abort', 'WritableStream');
      const close = callback(d, 'close', 'WritableStream');
      const start = callback(d, 'start', 'WritableStream');
      const type = d.type;
      const write = callback(d, 'write', 'WritableStream');
      if (type !== undefined) throw new RangeErrorC('Invalid type is specified');
      const hwm = highWaterMark(strat, 1);
      const st = wsNew();
      st.wrapper = this;
      WritableStream$s.set(this, st);
      wsSetup(
        st,
        (w) => (start ? apply(start, d, [w]) : undefined),
        write ? (chunk) => call(write, d, [chunk, st.controller.wrapper]) : () => resolved(),
        close ? () => call(close, d, []) : () => resolved(),
        abort ? (r) => call(abort, d, [r]) : () => resolved(),
        hwm,
        sizeAlgorithm(strat),
      );
    }
    get locked() {
      if (!isWS(this)) throw invalid('WritableStream', 'locked');
      return WritableStream$s.get(this).writer !== undefined;
    }
    abort(reason = undefined) {
      if (!isWS(this)) return rejected(invalid('WritableStream', 'abort'));
      if (WritableStream$s.get(this).writer) return rejected(new TypeErrorC('Cannot abort a locked stream'));
      return wsAbort(WritableStream$s.get(this), reason);
    }
    close() {
      if (!isWS(this)) return rejected(invalid('WritableStream', 'close'));
      if (WritableStream$s.get(this).writer) return rejected(new TypeErrorC('Cannot close a locked stream'));
      if (wsCloseQueued(WritableStream$s.get(this))) return rejected(new TypeErrorC('Cannot close a stream that is already closing'));
      return wsClose(WritableStream$s.get(this));
    }
    getWriter() {
      if (!isWS(this)) throw invalid('WritableStream', 'getWriter');
      return new WritableStreamDefaultWriter(this);
    }
  }
  const makeWritable = (start, writeAlg, closeAlg, abortAlg, hwm, size) => {
    const st = wsNew();
    st.wrapper = new WritableStream(token, st);
    wsSetup(st, start, writeAlg, closeAlg, abortAlg, hwm, size);
    return st;
  };
  const WritableStreamDefaultController$c = slot('WritableStreamDefaultController.c');
  class WritableStreamDefaultController {
    constructor(t, c) {
      if (t !== token) throw illegal();
      WritableStreamDefaultController$c.set(this, c);
    }
    get signal() {
      return apply(signalOf, WritableStreamDefaultController$c.get(this).abortController, []);
    }
    error(e = undefined) {
      if (WritableStreamDefaultController$c.get(this).stream.state === 'writable') wcErrorIfNeeded(WritableStreamDefaultController$c.get(this), e);
    }
  }
  const WritableStreamDefaultWriter$w = slot('WritableStreamDefaultWriter.w');
  class WritableStreamDefaultWriter {
    static #is = (v) => WritableStreamDefaultWriter$w.has(v);
    constructor(stream) {
      if (!isWS(stream)) throw new TypeErrorC("Failed to construct 'WritableStreamDefaultWriter': parameter 1 is not of type 'WritableStream'.");
      const st = wsOf(stream);
      if (st.writer) throw new TypeErrorC('WritableStream is locked');
      WritableStreamDefaultWriter$w.set(this, newWriter(st));
    }
    get closed() {
      if (!WritableStreamDefaultWriter.#is(this)) return rejected(invalid('WritableStreamDefaultWriter', 'closed'));
      return WritableStreamDefaultWriter$w.get(this).closed.promise;
    }
    get ready() {
      if (!WritableStreamDefaultWriter.#is(this)) return rejected(invalid('WritableStreamDefaultWriter', 'ready'));
      return WritableStreamDefaultWriter$w.get(this).ready.promise;
    }
    get desiredSize() {
      if (!WritableStreamDefaultWriter.#is(this)) throw invalid('WritableStreamDefaultWriter', 'desiredSize');
      const st = WritableStreamDefaultWriter$w.get(this).stream;
      if (!st) throw new TypeErrorC('Writer was released');
      if (st.state === 'errored' || st.state === 'erroring') return null;
      if (st.state === 'closed') return 0;
      return st.controller.hwm - st.controller.queueSize;
    }
    abort(reason = undefined) {
      if (!WritableStreamDefaultWriter.#is(this)) return rejected(invalid('WritableStreamDefaultWriter', 'abort'));
      if (!WritableStreamDefaultWriter$w.get(this).stream) return rejected(new TypeErrorC('Writer was released'));
      return wsAbort(WritableStreamDefaultWriter$w.get(this).stream, reason);
    }
    close() {
      if (!WritableStreamDefaultWriter.#is(this)) return rejected(invalid('WritableStreamDefaultWriter', 'close'));
      const st = WritableStreamDefaultWriter$w.get(this).stream;
      if (!st) return rejected(new TypeErrorC('Writer was released'));
      if (wsCloseQueued(st)) return rejected(new TypeErrorC('Cannot close a stream that is already closing'));
      return wsClose(st);
    }
    releaseLock() {
      if (!WritableStreamDefaultWriter.#is(this)) throw invalid('WritableStreamDefaultWriter', 'releaseLock');
      if (WritableStreamDefaultWriter$w.get(this).stream) writerRelease(WritableStreamDefaultWriter$w.get(this));
    }
    write(chunk = undefined) {
      if (!WritableStreamDefaultWriter.#is(this)) return rejected(invalid('WritableStreamDefaultWriter', 'write'));
      if (!WritableStreamDefaultWriter$w.get(this).stream) return rejected(new TypeErrorC('Writer was released'));
      return writerWrite(WritableStreamDefaultWriter$w.get(this), chunk);
    }
  }

  // ----- TransformStream -------------------------------------------------

  function tsSetBackpressure(ts, backpressure) {
    if (ts.backpressureChange) ts.backpressureChange.resolve(undefined);
    ts.backpressureChange = defer();
    ts.backpressure = backpressure;
  }
  function tsUnblockWrite(ts) {
    if (ts.backpressure) tsSetBackpressure(ts, false);
  }
  const tcClear = (c) => {
    c.transformAlg = undefined;
    c.flushAlg = undefined;
    c.cancelAlg = undefined;
  };
  function tsErrorWritable(ts, e) {
    tcClear(ts.controller);
    wcErrorIfNeeded(ts.writable.controller, e);
    tsUnblockWrite(ts);
  }
  function tsError(ts, e) {
    rcError(ts.readable.controller, e);
    tsErrorWritable(ts, e);
  }
  function tcEnqueue(c, chunk) {
    const ts = c.stream;
    const rc = ts.readable.controller;
    if (!rcCanClose(rc)) throw new TypeErrorC('Readable side is not in a state that permits enqueue');
    try {
      rcEnqueue(rc, chunk);
    } catch (e) {
      tsErrorWritable(ts, e);
      throw ts.readable.storedError;
    }
    const backpressure = !rcShouldPull(rc);
    if (backpressure !== ts.backpressure) tsSetBackpressure(ts, true);
  }
  function tcTerminate(c) {
    const ts = c.stream;
    rcClose(ts.readable.controller);
    tsErrorWritable(ts, new TypeErrorC('TransformStream terminated'));
  }
  function makeTransform(startFn, transformAlg, flushAlg, cancelAlg, wHwm, wSize, rHwm, rSize) {
    const startP = defer();
    const ts = { readable: undefined, writable: undefined, backpressure: undefined, backpressureChange: undefined, controller: undefined, wrapper: undefined };
    const c = { stream: ts, transformAlg, flushAlg, cancelAlg, finish: undefined, wrapper: undefined };
    ts.controller = c;
    const start = () => startP.promise;
    const perform = (chunk) => then(c.transformAlg(chunk), undefined, (r) => {
      tsError(ts, r);
      throw r;
    });
    ts.writable = makeWritable(start, (chunk) => {
      if (ts.backpressure) {
        return then(ts.backpressureChange.promise, () => {
          if (ts.writable.state === 'erroring') throw ts.writable.storedError;
          return perform(chunk);
        });
      }
      return perform(chunk);
    }, () => {
      if (c.finish) return c.finish.promise;
      const rd = ts.readable;
      c.finish = defer();
      const p = c.flushAlg();
      tcClear(c);
      then(p, () => {
        if (rd.state === 'errored') return c.finish.reject(rd.storedError);
        rcClose(rd.controller);
        c.finish.resolve(undefined);
      }, (r) => {
        rcError(rd.controller, r);
        c.finish.reject(r);
      });
      return c.finish.promise;
    }, (reason) => {
      if (c.finish) return c.finish.promise;
      const rd = ts.readable;
      c.finish = defer();
      const p = c.cancelAlg(reason);
      tcClear(c);
      then(p, () => {
        if (rd.state === 'errored') return c.finish.reject(rd.storedError);
        rcError(rd.controller, reason);
        c.finish.resolve(undefined);
      }, (r) => {
        rcError(rd.controller, r);
        c.finish.reject(r);
      });
      return c.finish.promise;
    }, wHwm, wSize);
    ts.readable = makeStream(false, start, () => {
      tsSetBackpressure(ts, false);
      return ts.backpressureChange.promise;
    }, (reason) => {
      if (c.finish) return c.finish.promise;
      const w = ts.writable;
      c.finish = defer();
      const p = c.cancelAlg(reason);
      tcClear(c);
      then(p, () => {
        if (w.state === 'errored') return c.finish.reject(w.storedError);
        wcErrorIfNeeded(w.controller, reason);
        tsUnblockWrite(ts);
        c.finish.resolve(undefined);
      }, (r) => {
        wcErrorIfNeeded(w.controller, r);
        tsUnblockWrite(ts);
        c.finish.reject(r);
      });
      return c.finish.promise;
    }, rHwm, rSize);
    tsSetBackpressure(ts, true);
    c.wrapper = new TransformStreamDefaultController(token, c);
    if (startFn) {
      try {
        startP.resolve(startFn(c.wrapper));
      } catch (e) {
        startP.reject(e);
        throw e;
      }
    } else {
      startP.resolve(undefined);
    }
    return ts;
  }
  const TransformStreamDefaultController$c = slot('TransformStreamDefaultController.c');
  class TransformStreamDefaultController {
    constructor(t, c) {
      if (t !== token) throw illegal();
      TransformStreamDefaultController$c.set(this, c);
    }
    get desiredSize() {
      return rcDesired(TransformStreamDefaultController$c.get(this).stream.readable.controller);
    }
    enqueue(chunk = undefined) {
      tcEnqueue(TransformStreamDefaultController$c.get(this), chunk);
    }
    error(reason = undefined) {
      tsError(TransformStreamDefaultController$c.get(this).stream, reason);
    }
    terminate() {
      tcTerminate(TransformStreamDefaultController$c.get(this));
    }
  }
  let tsOf;
  let isTS;
  const TransformStream$s = slot('TransformStream.s');
  class TransformStream {
    static {
      tsOf = (v) => TransformStream$s.get(v);
      isTS = (v) => TransformStream$s.has(v);
    }
    constructor(transformer = undefined, writableStrategy = undefined, readableStrategy = undefined) {
      // A transferred TransformStream: its two sides, made by `transferIn`.
      if (transformer === token) {
        TransformStream$s.set(this, writableStrategy);
        return;
      }
      const wStrat = strategyOf(writableStrategy, 'TransformStream');
      const rStrat = strategyOf(readableStrategy, 'TransformStream');
      if (transformer !== undefined && transformer !== null && !isObject(transformer)) {
        throw new TypeErrorC("Failed to construct 'TransformStream': The provided value is not of type 'object'.");
      }
      const d = dictionary(transformer, 'TransformStream');
      const cancel = callback(d, 'cancel', 'TransformStream');
      const flush = callback(d, 'flush', 'TransformStream');
      const readableType = d.readableType;
      const start = callback(d, 'start', 'TransformStream');
      const transform = callback(d, 'transform', 'TransformStream');
      const writableType = d.writableType;
      if (readableType !== undefined) throw new RangeErrorC('Invalid readableType specified');
      if (writableType !== undefined) throw new RangeErrorC('Invalid writableType specified');
      const rHwm = highWaterMark(rStrat, 0);
      const wHwm = highWaterMark(wStrat, 1);
      let ts;
      const controller = () => ts.controller.wrapper;
      const startFn = start ? (w) => apply(start, d, [w]) : undefined;
      // The start callback runs inside makeTransform, before `ts` is assigned.
      const transformAlg = transform
        ? (chunk) => call(transform, d, [chunk, controller()])
        : (chunk) => {
          try {
            tcEnqueue(ts.controller, chunk);
            return resolved();
          } catch (e) {
            return rejected(e);
          }
        };
      ts = makeTransform(
        startFn,
        transformAlg,
        flush ? () => call(flush, d, [controller()]) : () => resolved(),
        cancel ? (r) => call(cancel, d, [r]) : () => resolved(),
        wHwm,
        sizeAlgorithm(wStrat),
        rHwm,
        sizeAlgorithm(rStrat),
      );
      TransformStream$s.set(this, ts);
    }
    get readable() {
      return TransformStream$s.get(this).readable.wrapper;
    }
    get writable() {
      return TransformStream$s.get(this).writable.wrapper;
    }
  }

  // ----- Queuing strategies ----------------------------------------------

  const strategyInit = (init, name) => {
    if (!isObject(init)) throw new TypeErrorC(`Failed to construct '${name}': cannot convert to dictionary.`);
    const hwm = init.highWaterMark;
    if (hwm === undefined) throw new TypeErrorC(`Failed to construct '${name}': Failed to read the 'highWaterMark' property from 'QueuingStrategyInit': Required member is undefined.`);
    return NumberC(hwm);
  };
  const byteLengthSize = { size(chunk) { return chunk.byteLength; } }.size;
  defineProperty(byteLengthSize, 'name', { value: 'size' });
  const countSize = { size() { return 1; } }.size;
  const ByteLengthQueuingStrategy$hwm = slot('ByteLengthQueuingStrategy.hwm');
  class ByteLengthQueuingStrategy {
    constructor(init) {
      ByteLengthQueuingStrategy$hwm.set(this, strategyInit(init, 'ByteLengthQueuingStrategy'));
    }
    get highWaterMark() {
      return ByteLengthQueuingStrategy$hwm.get(this);
    }
    get size() {
      if (!ByteLengthQueuingStrategy$hwm.has(this)) throw new TypeErrorC('Illegal invocation');
      return byteLengthSize;
    }
  }
  const CountQueuingStrategy$hwm = slot('CountQueuingStrategy.hwm');
  class CountQueuingStrategy {
    constructor(init) {
      CountQueuingStrategy$hwm.set(this, strategyInit(init, 'CountQueuingStrategy'));
    }
    get highWaterMark() {
      return CountQueuingStrategy$hwm.get(this);
    }
    get size() {
      if (!CountQueuingStrategy$hwm.has(this)) throw new TypeErrorC('Illegal invocation');
      return countSize;
    }
  }

  // ----- Encoding streams ------------------------------------------------

  const TextEncoderStream$t = slot('TextEncoderStream.t');
  class TextEncoderStream {
    constructor() {
      const encoder = new TextEncoderC();
      let pending = '';
      const encodeOut = (c, text) => {
        const bytes = apply(encode, encoder, [text]);
        if (bytes.length > 0) tcEnqueue(c, bytes);
      };
      TextEncoderStream$t.set(this, makeTransform(undefined, (chunk) => {
        try {
          let text = pending + `${chunk}`;
          pending = '';
          const last = text.length > 0 ? text.charCodeAt(text.length - 1) : 0;
          if (last >= 0xd800 && last <= 0xdbff) {
            pending = text[text.length - 1];
            text = text.slice(0, -1);
          }
          if (text !== '') encodeOut(TextEncoderStream$t.get(this).controller, text);
          return resolved();
        } catch (e) {
          return rejected(e);
        }
      }, () => {
        try {
          if (pending !== '') {
            pending = '';
            encodeOut(TextEncoderStream$t.get(this).controller, '�');
          }
          return resolved();
        } catch (e) {
          return rejected(e);
        }
      }, () => resolved(), 1, () => 1, 0, () => 1));
    }
    get encoding() {
      if (!TextEncoderStream$t.has(this)) throw new TypeErrorC('Illegal invocation');
      return 'utf-8';
    }
    get readable() {
      return TextEncoderStream$t.get(this).readable.wrapper;
    }
    get writable() {
      return TextEncoderStream$t.get(this).writable.wrapper;
    }
  }
  const TextDecoderStream$t = slot('TextDecoderStream.t');
  const TextDecoderStream$decoder = slot('TextDecoderStream.decoder');
  class TextDecoderStream {
    constructor(label = undefined, options = undefined) {
      const decoder = new TextDecoderC(label, options);
      TextDecoderStream$decoder.set(this, decoder);
      const out = (text) => {
        if (text !== '') tcEnqueue(TextDecoderStream$t.get(this).controller, text);
      };
      const streaming = { stream: true };
      Object.setPrototypeOf(streaming, null);
      TextDecoderStream$t.set(this, makeTransform(undefined, (chunk) => {
        try {
          // decode() treats a missing input as empty; a stream chunk must be a BufferSource.
          if (chunk === undefined) throw new TypeErrorC('chunk is not an ArrayBuffer or ArrayBufferView');
          out(apply(decode, decoder, [chunk, streaming]));
          return resolved();
        } catch (e) {
          return rejected(e);
        }
      }, () => {
        try {
          out(apply(decode, decoder, []));
          return resolved();
        } catch (e) {
          return rejected(e);
        }
      }, () => resolved(), 1, () => 1, 0, () => 1));
    }
    get encoding() {
      return TextDecoderStream$decoder.get(this).encoding;
    }
    get fatal() {
      return TextDecoderStream$decoder.get(this).fatal;
    }
    get ignoreBOM() {
      return TextDecoderStream$decoder.get(this).ignoreBOM;
    }
    get readable() {
      return TextDecoderStream$t.get(this).readable.wrapper;
    }
    get writable() {
      return TextDecoderStream$t.get(this).writable.wrapper;
    }
  }

  // ----- Compression streams ---------------------------------------------

  const FORMATS = ['deflate', 'deflate-raw', 'gzip', 'brotli'];
  // A transform over a native codec: each chunk must be a BufferSource; the
  // codec's TypeErrors (corrupt or truncated data) error both sides.
  const codecTransform = (kind, format, compress) => {
    const name = `${format}`;
    if (FORMATS.indexOf(name) < 0) {
      throw new TypeErrorC(`Failed to construct '${kind}': The provided value '${name}' is not a valid enum value of type CompressionFormat.`);
    }
    const codec = zlib(name, compress);
    const push = codec.push, finish = codec.finish;
    const enqueue = out => tcEnqueue(t.controller, out);
    const t = makeTransform(undefined, (chunk) => {
      try {
        push(chunk, enqueue);
        return resolved();
      } catch (e) {
        return rejected(e);
      }
    }, () => {
      try {
        const out = finish();
        if (out.length > 0) tcEnqueue(t.controller, out);
        return resolved();
      } catch (e) {
        return rejected(e);
      }
    }, () => resolved(), 1, () => 1, 0, () => 1);
    return t;
  };
  const CompressionStream$t = slot('CompressionStream.t');
  class CompressionStream {
    constructor(format) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'CompressionStream': 1 argument required, but only 0 present.");
      CompressionStream$t.set(this, codecTransform('CompressionStream', format, true));
    }
    get readable() {
      return CompressionStream$t.get(this).readable.wrapper;
    }
    get writable() {
      return CompressionStream$t.get(this).writable.wrapper;
    }
  }
  const DecompressionStream$t = slot('DecompressionStream.t');
  class DecompressionStream {
    constructor(format) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'DecompressionStream': 1 argument required, but only 0 present.");
      DecompressionStream$t.set(this, codecTransform('DecompressionStream', format, false));
    }
    get readable() {
      return DecompressionStream$t.get(this).readable.wrapper;
    }
    get writable() {
      return DecompressionStream$t.get(this).writable.wrapper;
    }
  }

  for (const C of [
    ReadableStream, ReadableStreamDefaultReader, ReadableStreamBYOBReader, ReadableStreamDefaultController, ReadableByteStreamController,
    ReadableStreamBYOBRequest, WritableStream, WritableStreamDefaultWriter, WritableStreamDefaultController, TransformStream,
    TransformStreamDefaultController, ByteLengthQueuingStrategy, CountQueuingStrategy, TextEncoderStream, TextDecoderStream,
    CompressionStream, DecompressionStream,
  ]) {
    idl(C, C.name);
  }
  // Interface objects report the length of their required arguments.
  for (const [C, length] of [[ReadableStreamDefaultReader, 1], [ReadableStreamBYOBReader, 1], [WritableStreamDefaultWriter, 1], [ByteLengthQueuingStrategy, 1], [CountQueuingStrategy, 1], [ReadableStreamDefaultController, 0], [ReadableByteStreamController, 0], [ReadableStreamBYOBRequest, 0], [WritableStreamDefaultController, 0], [TransformStreamDefaultController, 0]]) {
    defineProperty(C, 'length', { value: length, configurable: true });
  }

  // ----- Helpers for streams.rs ------------------------------------------

  // Every chunk of a body stream, concatenated; non-Uint8Array chunks reject.
  // The stream's bytes, as a promise of \`{bytes}\`.
  const collect = (st) => {
    const rd = newReader(st);
    const chunks = [];
    let total = 0;
    const d = defer();
    const step = () => {
      readerRead(rd, {
        chunk(value) {
          if (!isUint8Array(value)) {
            const e = new TypeErrorC('Received non-Uint8Array chunk');
            d.reject(e);
            then(rsCancel(st, e), undefined, noop);
            return;
          }
          chunks.push(value);
          total += value.length;
          microtask(step);
        },
        close() {
          const out = new Uint8ArrayC(total);
          let offset = 0;
          for (let i = 0; i < chunks.length; i++) {
            out.set(chunks[i], offset);
            offset += chunks[i].length;
          }
          // Boxed without a prototype: resolving with the bytes themselves
          // would look up a \`then\` script put on Object.prototype.
          d.resolve({ __proto__: null, bytes: out });
        },
        error: (e) => d.reject(e),
      });
    };
    step();
    return d.promise;
  };
  // ----- Transfer (HTML structured clone, Streams §8) ---------------------
  // A transferred stream is a cross-realm transform over a MessageChannel:
  // the original is piped into (or from) one port, the new stream wraps the
  // other, which goes wherever the message goes (another frame or thread).

  const dataClone = (message) => new DOMExceptionC(message, 'DataCloneError');
  const pack = (port, type, value) => apply(portPost, port, [{ type, value }]);
  const sendError = (port, e) => {
    try {
      pack(port, 'error', e);
    } catch (_) {}
  };
  const packHandlingError = (port, type, value) => {
    try {
      pack(port, type, value);
    } catch (e) {
      sendError(port, e);
      throw e;
    }
  };
  const disentangle = (port) => apply(portClose, port, []);
  // `onMessage(type, value)` for each well-formed message; `onError(e)` when
  // one cannot be deserialized here.
  const listen = (port, onMessage, onError) => {
    apply(addListener, port, ['message', (event) => {
      const data = apply(eventData, event, []);
      if (!isObject(data)) return;
      onMessage(data.type, data.value);
    }]);
    apply(addListener, port, ['messageerror', () => {
      const e = dataClone('A message from the transferred stream could not be deserialized');
      sendError(port, e);
      onError(e);
      disentangle(port);
    }]);
    apply(portStart, port, []);
  };
  // SetUpCrossRealmTransformReadable.
  function crossReadable(port) {
    const st = rsNew();
    st.wrapper = new ReadableStream(token, st);
    let c;
    listen(port, (type, value) => {
      if (type === 'chunk') {
        try {
          rcEnqueue(c, value);
        } catch (_) {}
      } else if (type === 'close') {
        rcClose(c);
        disentangle(port);
      } else if (type === 'error') {
        rcError(c, value);
        disentangle(port);
      }
    }, (e) => rcError(c, e));
    c = rcSetup(st, false, undefined, () => {
      pack(port, 'pull', undefined);
      return resolved();
    }, (reason) => {
      try {
        packHandlingError(port, 'error', reason);
      } catch (e) {
        disentangle(port);
        return rejected(e);
      }
      disentangle(port);
      return resolved();
    }, 0, () => 1);
    return st;
  }
  // SetUpCrossRealmTransformWritable.
  function crossWritable(port) {
    const st = wsNew();
    st.wrapper = new WritableStream(token, st);
    let backpressure = defer();
    const release = () => {
      if (backpressure) backpressure.resolve(undefined);
      backpressure = undefined;
    };
    let c;
    listen(port, (type, value) => {
      if (type === 'pull') {
        release();
      } else if (type === 'error') {
        wcErrorIfNeeded(c, value);
        release();
      }
    }, (e) => wcErrorIfNeeded(c, e));
    c = wsSetup(st, noop, (chunk) => {
      if (!backpressure) backpressure = settledDefer(resolved());
      return then(backpressure.promise, () => {
        backpressure = defer();
        try {
          packHandlingError(port, 'chunk', chunk);
        } catch (e) {
          disentangle(port);
          throw e;
        }
      });
    }, () => {
      pack(port, 'close', undefined);
      disentangle(port);
      return resolved();
    }, (reason) => {
      try {
        packHandlingError(port, 'error', reason);
      } catch (e) {
        disentangle(port);
        return rejected(e);
      }
      disentangle(port);
      return resolved();
    }, 1, () => 1);
    return st;
  }
  // The transfer steps of a ReadableStream (state `st`): the port to send.
  const readableOut = (st) => {
    if (rsLocked(st)) throw dataClone('A ReadableStream could not be cloned because it was locked');
    const channel = new MessageChannelC();
    const writable = crossWritable(apply(port1Of, channel, []));
    handled(rsPipe(st, writable, false, false, false, undefined));
    return apply(port2Of, channel, []);
  };
  const writableOut = (st) => {
    if (st.writer !== undefined) throw dataClone('A WritableStream could not be cloned because it was locked');
    const channel = new MessageChannelC();
    const readable = crossReadable(apply(port1Of, channel, []));
    handled(rsPipe(readable, st, false, false, false, undefined));
    return apply(port2Of, channel, []);
  };
  // 'R', 'W' or 'T' for a transferable stream of this realm, undefined for
  // anything else; DataCloneError for a locked one. No side effects.
  const transferKind = (value) => {
    if (!MessageChannelC) return undefined;
    if (isRS(value)) {
      if (rsLocked(rsOf(value))) throw dataClone('A ReadableStream could not be cloned because it was locked');
      return 'R';
    }
    if (isWS(value)) {
      if (wsOf(value).writer !== undefined) throw dataClone('A WritableStream could not be cloned because it was locked');
      return 'W';
    }
    if (isTS(value)) {
      const ts = tsOf(value);
      if (rsLocked(ts.readable) || ts.writable.writer !== undefined) {
        throw dataClone('A TransformStream could not be cloned because it was locked');
      }
      return 'T';
    }
    return undefined;
  };
  // The transfer steps: the ports (one, or two for a TransformStream) that
  // stand for `value` from now on.
  const transferOut = (value) => {
    if (isRS(value)) return [readableOut(rsOf(value))];
    if (isWS(value)) return [writableOut(wsOf(value))];
    const ts = tsOf(value);
    return [readableOut(ts.readable), writableOut(ts.writable)];
  };
  // The transfer-receiving steps: a stream of this realm over `ports`.
  const transferIn = (kind, ports) => {
    if (kind === 'R') return crossReadable(ports[0]).wrapper;
    if (kind === 'W') return crossWritable(ports[0]).wrapper;
    const ts = { readable: crossReadable(ports[0]), writable: crossWritable(ports[1]) };
    return new TransformStream(token, ts);
  };

  const byteBody = (pull, cancel) => {
    const st = rsNew();
    st.wrapper = new ReadableStream(token, st);
    const c = rcSetup(st, true, undefined, () => pull(c), cancel, 0, () => 1);
    return st.wrapper;
  };
  const textFrom = (stream) => {
    const t = TextDecoderStream$t.get(new TextDecoderStream());
    handled(rsPipe(rsOf(stream), t.writable, false, false, false, undefined));
    return t.readable.wrapper;
  };
  const bytesBody = (bytes) => byteBody((c) => {
    if (bytes.length > 0) bcEnqueue(c, bytes);
    if (rcCanClose(c)) bcClose(c);
    return resolved();
  }, () => resolved());
  return {
    ReadableStream, ReadableStreamDefaultReader, ReadableStreamBYOBReader, ReadableStreamDefaultController, ReadableByteStreamController,
    ReadableStreamBYOBRequest, WritableStream, WritableStreamDefaultWriter, WritableStreamDefaultController, TransformStream,
    TransformStreamDefaultController, ByteLengthQueuingStrategy, CountQueuingStrategy, TextEncoderStream, TextDecoderStream,
    CompressionStream, DecompressionStream,
    errorWritableController: (controller, error) => wcErrorIfNeeded(WritableStreamDefaultController$c.get(controller), error),
    isStream: isRS,
    transferKind,
    transferOut,
    transferIn,
    // Body "unusable" check.
    isLocked: (s) => rsLocked(rsOf(s)),
    isDisturbed: (s) => rsOf(s).disturbed,
    // A body stream made after the body was consumed: locked and disturbed,
    // as the consuming read left the stream it would have used.
    markConsumed(s) {
      const st = rsOf(s);
      if (!rsLocked(st)) newReader(st);
      st.disturbed = true;
    },
    // A byte stream over a native source: read() -> {value, done}, cancel().
    // Once the body is open, one native read runs ahead of the reader: Fetch errors the
    // stream when the network fails, whether or not a read is pending (`reader.closed`).
    fromBody(source, read, cancel) {
      let ahead;
      return byteBody((c) => {
        const p = ahead || apply(read, source, []);
        ahead = undefined;
        return then(p, (r) => {
          if (!rcCanClose(c)) return;
          if (r.done) {
            bcClose(c);
            return;
          }
          bcEnqueue(c, r.value);
          const next = apply(read, source, []);
          ahead = next;
          then(next, noop, (e) => {
            if (ahead !== next) return; // a pull took it and reports its failure
            ahead = undefined;
            rcError(c, e);
          });
        });
      }, (reason) => {
        ahead = undefined;
        return then(apply(cancel, source, [reason]), noop);
      });
    },
    // A byte stream of bytes already in memory (Blob.stream()).
    fromBytes: bytesBody,
    // Blob.textStream() and the Body mixin's textStream(): bytes, or a usable
    // byte stream (which the pipe locks and disturbs at once), piped through a
    // UTF-8 TextDecoderStream with the module's own pipe (no script-visible
    // pipeThrough lookup).
    textFromBytes: (bytes) => textFrom(bytesBody(bytes)),
    textFromStream: textFrom,
    // Response consumption for a body that is a script's stream.
    consume(stream, kind, packageBody) {
      const st = rsOf(stream);
      if (rsLocked(st) || st.disturbed) return rejected(new TypeErrorC('Response body has already been consumed'));
      return then(collect(st), ({ bytes }) => {
        switch (kind) {
          case 'text': return apply(decode, new TextDecoderC(), [bytes]);
          case 'json': return parseJSON(apply(decode, new TextDecoderC(), [bytes]));
          case 'arrayBuffer': return bytes.buffer;
          case 'bytes': return bytes;
          case 'package': return packageBody(bytes);
        }
      });
    },
    cloneBody: (stream, cloneChunk) => rsTee(rsOf(stream), cloneChunk),
    // fetch() with a stream body: the bytes are sent once the stream ends.
    fetchBuffered(input, init, stream) {
      const st = rsOf(stream);
      if (rsLocked(st) || st.disturbed) return rejected(new TypeErrorC('Failed to execute \'fetch\' on \'Window\': The body stream is locked or disturbed'));
      return then(collect(st), ({ bytes }) => {
        const copy = {};
        for (const key of Reflect.ownKeys(init)) copy[key] = init[key];
        copy.body = bytes;
        return fetchFn(input, copy);
      });
    },
  };
})
