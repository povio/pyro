// WebSocket, CloseEvent, WebSocketStream and WebSocketError are made on a
// realm's first use of one of these interfaces. The script evaluates to a function of the engine's helpers
// (websocket.rs) that returns the module object:
// { WebSocket, CloseEvent, WebSocketStream, WebSocketError, deliver, define }.
//
// The connection lives on the network thread; the engine calls `deliver`
// from a task of this realm's event loop for each of its events, in order.
(native => {
  'use strict';
  const { connect, send, close, abort, closeReason, dispatch, getHandler, setHandler } = native;
  const apply = Reflect.apply;
  const define = Object.defineProperty;
  const getOwn = Object.getOwnPropertyDescriptor;
  const ownKeys = Reflect.ownKeys;
  const iterator = Symbol.iterator;
  const toStringTag = Symbol.toStringTag;
  const ET = EventTarget;
  const Ev = Event;
  const ME = MessageEvent;
  const B = Blob;
  const DOMEx = DOMException;
  const TE = TypeError;
  const Str = String;
  // WebIDL DOMString/USVString conversion: a Symbol throws.
  function S(v) {
    if (typeof v === 'symbol') throw new TE('Cannot convert a Symbol value to a string');
    return Str(v);
  }
  const N = Number;
  const floor = Math.floor;

  const CONNECTING = 0, OPEN = 1, CLOSING = 2, CLOSED = 3;

  function isObject(v) { return v !== null && (typeof v === 'object' || typeof v === 'function'); }

  // WebIDL sequence<DOMString> from an iterable.
  function strings(value) {
    const method = value[iterator];
    if (typeof method !== 'function') throw new TE("Failed to construct 'WebSocket': The provided value cannot be converted to a sequence.");
    const out = [];
    const it = apply(method, value, []);
    for (;;) {
      const step = it.next();
      if (!isObject(step)) throw new TE('Iterator result is not an object');
      if (step.done) return out;
      out[out.length] = S(step.value);
    }
  }

  // `(sequence<DOMString> or DOMString or WebSocketInit) protocols`.
  function protocolList(protocols) {
    if (protocols === undefined) return [];
    if (isObject(protocols)) {
      if (typeof protocols[iterator] === 'function') return strings(protocols);
      const member = protocols.protocols;
      if (member === undefined) return [];
      if (isObject(member)) return strings(member);
      return [S(member)];
    }
    return [S(protocols)];
  }

  // WebIDL `[Clamp] unsigned short`.
  function clampShort(value) {
    let x = N(value);
    if (x !== x) return 0;
    if (x <= 0) return 0;
    if (x >= 65535) return 65535;
    const f = floor(x);
    const diff = x - f;
    if (diff > 0.5) return f + 1;
    if (diff < 0.5) return f;
    return f % 2 === 0 ? f : f + 1;
  }

  // WebIDL `unsigned short` (modulo).
  function toShort(value) {
    let x = N(value);
    if (x !== x || x === Infinity || x === -Infinity) return 0;
    x = (x < 0 ? -floor(-x) : floor(x)) % 65536;
    return x < 0 ? x + 65536 : x;
  }

  class WebSocket extends ET {
    #id = 0;
    #url = '';
    #state = CONNECTING;
    #protocol = '';
    #extensions = '';
    #binaryType = 'blob';
    #buffered = 0;
    #origin = '';

    static #brand(o, what) {
      if (!isObject(o) || !(#id in o)) throw new TE(`Illegal invocation: ${what}`);
    }

    constructor(url, protocols = undefined) {
      if (arguments.length < 1) throw new TE("Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
      // WebIDL converts the arguments in order: url, then protocols.
      const href = S(url);
      const list = protocolList(protocols);
      super();
      const result = connect(this, href, list);
      this.#id = result[0];
      this.#url = result[1];
      this.#origin = result[2];
    }

    get url() { WebSocket.#brand(this, 'url'); return this.#url; }
    get readyState() { WebSocket.#brand(this, 'readyState'); return this.#state; }
    get bufferedAmount() { WebSocket.#brand(this, 'bufferedAmount'); return this.#buffered; }
    get extensions() { WebSocket.#brand(this, 'extensions'); return this.#extensions; }
    get protocol() { WebSocket.#brand(this, 'protocol'); return this.#protocol; }
    get binaryType() { WebSocket.#brand(this, 'binaryType'); return this.#binaryType; }
    set binaryType(value) {
      WebSocket.#brand(this, 'binaryType');
      value = S(value);
      // An enumeration attribute ignores values outside the enumeration.
      if (value === 'blob' || value === 'arraybuffer') this.#binaryType = value;
    }
    get onopen() { WebSocket.#brand(this, 'onopen'); return getHandler(this, 'open'); }
    set onopen(v) { WebSocket.#brand(this, 'onopen'); setHandler(this, 'open', v); }
    get onerror() { WebSocket.#brand(this, 'onerror'); return getHandler(this, 'error'); }
    set onerror(v) { WebSocket.#brand(this, 'onerror'); setHandler(this, 'error', v); }
    get onclose() { WebSocket.#brand(this, 'onclose'); return getHandler(this, 'close'); }
    set onclose(v) { WebSocket.#brand(this, 'onclose'); setHandler(this, 'close', v); }
    get onmessage() { WebSocket.#brand(this, 'onmessage'); return getHandler(this, 'message'); }
    set onmessage(v) { WebSocket.#brand(this, 'onmessage'); setHandler(this, 'message', v); }

    close(code = undefined, reason = undefined) {
      WebSocket.#brand(this, 'close');
      if (code !== undefined) {
        code = clampShort(code);
        if (code !== 1000 && (code < 3000 || code > 4999)) {
          throw new DOMEx(`Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. ${code} is neither.`, 'InvalidAccessError');
        }
      }
      if (reason !== undefined) reason = closeReason(S(reason));
      const state = this.#state;
      if (state === CLOSING || state === CLOSED) return;
      this.#state = CLOSING;
      // Closing a connection that is not established fails it.
      if (state === CONNECTING) abort(this.#id);
      else {
        // A reason without a code is sent with 1000 (no code, no body).
        if (reason === undefined) reason = '';
        if (code === undefined) code = reason === '' ? -1 : 1000;
        close(this.#id, code, reason);
      }
    }

    send(data) {
      WebSocket.#brand(this, 'send');
      if (arguments.length < 1) throw new TE("Failed to execute 'send' on 'WebSocket': 1 argument required, but only 0 present.");
      const state = this.#state;
      if (state === CONNECTING) throw new DOMEx("Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.", 'InvalidStateError');
      // After close() the bytes are counted but not sent.
      this.#buffered += send(this.#id, data, state === OPEN);
    }

    static deliver(ws, kind, a, b, c, d) {
      switch (kind) {
        case 'open': {
          if (ws.#state !== CONNECTING) return;
          ws.#state = OPEN;
          ws.#protocol = a;
          ws.#extensions = b;
          fire(ws, new Ev('open'));
          return;
        }
        case 'message': {
          if (ws.#state !== OPEN) return;
          let data = a;
          if (typeof data !== 'string' && ws.#binaryType === 'blob') data = new B([data]);
          fire(ws, new ME('message', { data, origin: ws.#origin }));
          return;
        }
        case 'sent': {
          const left = ws.#buffered - a;
          ws.#buffered = left > 0 ? left : 0;
          return;
        }
        case 'close': {
          if (ws.#state === CLOSED) return;
          ws.#state = CLOSED;
          if (d) fire(ws, new Ev('error'));
          fire(ws, new CloseEvent('close', { wasClean: a, code: b, reason: c }));
          return;
        }
      }
    }
  }

  // A trusted event, dispatched by the user agent.
  function fire(target, event) {
    dispatch(target, event);
  }

  class CloseEvent extends Ev {
    #wasClean = false;
    #code = 0;
    #reason = '';
    constructor(type, eventInitDict = undefined) {
      if (arguments.length < 1) throw new TE("Failed to construct 'CloseEvent': 1 argument required, but only 0 present.");
      if (eventInitDict !== undefined && eventInitDict !== null && !isObject(eventInitDict)) {
        throw new TE("Failed to construct 'CloseEvent': The provided value is not of type 'CloseEventInit'.");
      }
      super(type, eventInitDict);
      if (isObject(eventInitDict)) {
        // Dictionary members in lexicographic order.
        const code = eventInitDict.code;
        if (code !== undefined) this.#code = toShort(code);
        const reason = eventInitDict.reason;
        if (reason !== undefined) this.#reason = S(reason);
        this.#wasClean = !!eventInitDict.wasClean;
      }
    }
    get wasClean() { if (!isObject(this) || !(#code in this)) throw new TE('Illegal invocation'); return this.#wasClean; }
    get code() { if (!isObject(this) || !(#code in this)) throw new TE('Illegal invocation'); return this.#code; }
    get reason() { if (!isObject(this) || !(#code in this)) throw new TE('Illegal invocation'); return this.#reason; }
  }

  // WebSocketStream uses the same transport as WebSocket, but its readable
  // source requests one message at a time and its sink waits for the socket
  // to acknowledge each write. Neither queue is drained by polling.
  const { streamSlot, errorSlot, streamConstructors, pull, writeStream } = native;
  let streamPromise;

  function deferred() {
    // A classic socket never needs the stream promise implementation.
    const PromiseC = streamPromise ??= Promise;
    let resolve, reject;
    const promise = new PromiseC((ok, fail) => { resolve = ok; reject = fail; });
    return { promise, resolve, reject };
  }

  // Store a settled outcome even when no one has requested the attribute.
  // An unobserved opened/closed attribute must not manufacture an unhandled
  // rejected Promise. Once read, the attribute always returns that Promise.
  function promiseProperty() {
    let result, status, value;
    function settle(kind, v) {
      if (status) return;
      status = kind;
      if (result) result[kind](v);
      else value = v;
    }
    return {
      get promise() {
        if (!result) {
          result = deferred();
          if (status) {
            result[status](value);
            value = undefined;
          }
        }
        return result.promise;
      },
      resolve(v) { settle('resolve', v); },
      reject(e) { settle('reject', e); },
    };
  }

  function dictionary(value) {
    if (value != null && !isObject(value)) throw new TE('Expected a dictionary');
    return value;
  }

  function closeInfo(value) {
    const options = dictionary(value);
    const rawCode = options?.closeCode;
    // The unary plus is ToNumber, including when an object yields BigInt.
    let code = rawCode === undefined ? null : clampShort(+rawCode);
    const rawReason = options?.reason;
    const text = rawReason === undefined ? '' : S(rawReason);
    if (code !== null && code !== 1000 && (code < 3000 || code > 4999))
      throw new DOMEx('The close code must be 1000 or between 3000 and 4999.', 'InvalidAccessError');
    const reason = closeReason(text);
    if (code === null && reason !== '') code = 1000;
    return { closeCode: code, reason };
  }

  class WebSocketError extends DOMEx {
    constructor(message = '', init = undefined) {
      const text = S(message);
      const info = closeInfo(init);
      super(text, 'WebSocketError');
      errorSlot.set(this, info);
    }
    get closeCode() {
      const info = errorSlot.get(this);
      if (!info) throw new TE('Illegal invocation');
      return info.closeCode;
    }
    get reason() {
      const info = errorSlot.get(this);
      if (!info) throw new TE('Illegal invocation');
      return info.reason;
    }
  }

  function connectionError(message, code, reason) {
    // Received codes (including abnormal closure) need not be codes that
    // script is allowed to send.
    const error = new WebSocketError(message);
    errorSlot.set(error, { closeCode: code, reason });
    return error;
  }

  function streamOf(value) {
    const st = streamSlot.get(value);
    if (!st) throw new TE('Illegal invocation');
    return st;
  }

  function startClose(st, info) {
    if (st.phase === CLOSING || st.phase === CLOSED) return;
    const connecting = st.phase === CONNECTING;
    st.phase = CLOSING;
    if (connecting) abort(st.id);
    else close(st.id, info?.closeCode ?? -1, info?.reason ?? '');
  }

  function abortedHandshake(st) {
    if (st.phase !== CONNECTING) return;
    st.phase = CLOSED;
    const error = new DOMEx('The WebSocket handshake was aborted.', 'AbortError');
    st.opened.reject(error);
    st.closed.reject(error);
  }

  class WebSocketStream {
    constructor(url, options = undefined) {
      if (!arguments.length) throw new TE('WebSocketStream requires a URL');
      const href = S(url);
      const dict = dictionary(options);
      const protocols = dict?.protocols;
      if (protocols !== undefined && !isObject(protocols)) throw new TE('Expected a sequence of protocols');
      const list = [];
      if (protocols !== undefined) {
        for (const protocol of protocols) list.push(S(protocol));
      }
      const signal = dict?.signal;
      const st = {
        id: 0, url: '', phase: CONNECTING, opened: promiseProperty(), closed: promiseProperty(),
        reader: null, writer: null, read: null, write: null, readerCancelled: false, writerClosing: false,
      };
      streamSlot.set(this, st);
      const result = connect(this, href, list, true, signal);
      st.id = result[0];
      st.url = result[1];
      if (st.id === 0) abortedHandshake(st);
      else if (st.id < 0) streamDelivery(st, 'close', false, 1006, '', true);
    }

    get url() { return streamOf(this).url; }
    get opened() { return streamOf(this).opened.promise; }
    get closed() { return streamOf(this).closed.promise; }
    close(options = undefined) {
      const st = streamOf(this);
      startClose(st, closeInfo(options));
    }
  }

  function openedStreams(st, protocol, extensions) {
    const [Readable, Writable] = streamConstructors();
    const readable = new Readable({
      start(controller) { st.reader = controller; },
      pull() {
        if (st.phase !== OPEN) return;
        st.read = deferred();
        pull(st.id);
        return st.read.promise;
      },
      cancel(reason) {
        st.readerCancelled = true;
        startClose(st, errorSlot.get(reason));
      },
    });
    const writable = new Writable({
      start(controller) { st.writer = controller; },
      write(chunk) {
        if (st.phase !== OPEN) throw new DOMEx('The WebSocketStream is closing.', 'InvalidStateError');
        const write = deferred();
        st.write = write;
        try { writeStream(st.id, chunk); }
        catch (error) { st.write = null; throw error; }
        return write.promise;
      },
      close() {
        st.writerClosing = true;
        startClose(st, null);
        return st.closed.promise;
      },
      abort(reason) { startClose(st, errorSlot.get(reason)); },
    });
    st.opened.resolve({ readable, writable, protocol, extensions });
  }

  function streamDelivery(st, kind, a, b, c, d) {
    if (st.phase === CLOSED) return;
    if (kind === 'open') {
      if (st.phase !== CONNECTING) return;
      st.phase = OPEN;
      openedStreams(st, a, b);
    } else if (kind === 'abort') {
      abortedHandshake(st);
    } else if (kind === 'message') {
      if (st.phase === OPEN && !st.readerCancelled)
        st.reader.enqueue(a);
      st.read?.resolve();
      st.read = null;
    } else if (kind === 'sent') {
      st.write?.resolve();
      st.write = null;
    } else if (kind === 'close') {
      const wasOpened = st.reader !== null;
      st.phase = CLOSED;
      if (!wasOpened)
        st.opened.reject(connectionError('WebSocket closed before the handshake completed.', null, ''));
      const clean = wasOpened && a && !d && !st.write;
      const error = clean ? null : connectionError('The WebSocket connection was not cleanly closed.', b, c);
      const invalid = new DOMEx('The WebSocketStream is closed.', 'InvalidStateError');
      if (st.reader && !st.readerCancelled) {
        if (clean) st.reader.close();
        else st.reader.error(error);
      }
      if (st.writer && !st.writerClosing) st.writer.error(error ?? invalid);
      st.write?.reject(invalid);
      st.write = null;
      st.read?.resolve();
      st.read = null;
      st.reader = st.writer = null;
      if (clean) st.closed.resolve({ closeCode: b, reason: c });
      else st.closed.reject(error);
    }
  }

  const classicDelivery = WebSocket.deliver;
  function deliver(socket, kind, a, b, c, d) {
    const st = streamSlot.get(socket);
    if (st) streamDelivery(st, kind, a, b, c, d);
    else classicDelivery(socket, kind, a, b, c, d);
  }
  delete WebSocket.deliver;

  // WebIDL shapes: enumerable members, constants on both the interface and
  // its prototype, @@toStringTag.
  function idl(C, name) {
    const proto = C.prototype;
    for (const key of ownKeys(proto)) {
      if (key === 'constructor') continue;
      const d = getOwn(proto, key);
      d.enumerable = true;
      define(proto, key, d);
    }
    define(proto, toStringTag, { value: name, configurable: true });
  }
  idl(WebSocketStream, 'WebSocketStream');
  idl(WebSocketError, 'WebSocketError');
  idl(WebSocket, 'WebSocket');
  idl(CloseEvent, 'CloseEvent');
  const constants = [['CONNECTING', CONNECTING], ['OPEN', OPEN], ['CLOSING', CLOSING], ['CLOSED', CLOSED]];
  for (let i = 0; i < constants.length; i++) {
    const d = { value: constants[i][1], enumerable: true };
    define(WebSocket, constants[i][0], d);
    define(WebSocket.prototype, constants[i][0], d);
  }

  return {
    WebSocket,
    CloseEvent,
    WebSocketStream,
    WebSocketError,
    deliver,
    define(target, name, value) {
      define(target, name, { value, writable: true, enumerable: false, configurable: true });
    },
  };
})
