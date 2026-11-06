// WebSocket and CloseEvent (HTML "Web sockets"), made on a realm's first use
// of either name. The script evaluates to a function of the engine's helpers
// (websocket.rs) that returns the module object:
// { WebSocket, CloseEvent, deliver, define }.
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
  const S = String;
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
      else close(this.#id, code === undefined ? -1 : code, reason === undefined ? '' : reason);
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

  const deliver = WebSocket.deliver;
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
    deliver,
    define(target, name, value) {
      define(target, name, { value, writable: true, enumerable: false, configurable: true });
    },
  };
})
