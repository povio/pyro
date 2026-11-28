// HTML server-sent events (https://html.spec.whatwg.org/#server-sent-events),
// made on a realm's first use of `EventSource`. The script evaluates to a
// function of the engine's transport (event_source.rs) that returns the
// module object: { EventSource }.
(native => {
  'use strict';
  const { resolve, connect, cancel, message } = native;
  const apply = Reflect.apply;
  const ET = EventTarget;
  const dispatch = ET.prototype.dispatchEvent;
  const addListener = ET.prototype.addEventListener;
  const Ev = Event;
  const DE = DOMException;
  // Captured now: a page's fake timers installed later do not stall reconnection.
  const setTimer = setTimeout;
  const clearTimer = clearTimeout;
  const CONNECTING = 0, OPEN = 1, CLOSED = 2;
  const DIGITS = /^[0-9]+$/;

  class EventSource extends ET {
    #url;
    #withCredentials;
    #readyState = CONNECTING;
    #request = 0;
    #timer = undefined;
    #reconnection = 3000;
    #lastEventId = '';
    #origin = '';
    // Parser state: the unterminated line, whether a CR ended the last chunk,
    // and the event being built.
    #line = '';
    #afterCR = false;
    #data = '';
    #type = '';
    #idBuffer = '';
    #handlers = { open: null, message: null, error: null };

    constructor(url, init = undefined) {
      super();
      if (arguments.length < 1) throw new TypeError("Failed to construct 'EventSource': 1 argument required, but only 0 present.");
      const resolved = resolve(`${url}`);
      if (resolved === null) {
        throw new DE(`Failed to construct 'EventSource': Cannot open an EventSource to '${url}'. The URL is invalid.`, 'SyntaxError');
      }
      if (init !== undefined && init !== null && typeof init !== 'object' && typeof init !== 'function') {
        throw new TypeError("Failed to construct 'EventSource': The provided value is not of type 'EventSourceInit'.");
      }
      this.#url = resolved;
      this.#withCredentials = init != null && !!init.withCredentials;
      this.#connect();
    }

    get url() { return this.#url; }
    get withCredentials() { return this.#withCredentials; }
    get readyState() { return this.#readyState; }

    close() {
      if (this.#readyState === CLOSED) return;
      this.#readyState = CLOSED;
      if (this.#timer !== undefined) { clearTimer(this.#timer); this.#timer = undefined; }
      if (this.#request) { cancel(this.#request); this.#request = 0; }
    }

    get onopen() { return this.#handlers.open; }
    set onopen(v) { this.#setHandler('open', v); }
    get onmessage() { return this.#handlers.message; }
    set onmessage(v) { this.#setHandler('message', v); }
    get onerror() { return this.#handlers.error; }
    set onerror(v) { this.#setHandler('error', v); }

    // An event handler IDL attribute: one listener, added on the first
    // non-null value, that calls the current handler.
    #listening = { open: false, message: false, error: false };
    #setHandler(type, value) {
      const handler = typeof value === 'function' || (value !== null && typeof value === 'object') ? value : null;
      this.#handlers[type] = handler;
      if (handler !== null && !this.#listening[type]) {
        this.#listening[type] = true;
        apply(addListener, this, [type, event => {
          const h = this.#handlers[type];
          if (typeof h === 'function') return apply(h, this, [event]);
        }]);
      }
    }

    #fire(event) { apply(dispatch, this, [event]); }

    #connect() {
      this.#line = '';
      this.#afterCR = false;
      this.#data = '';
      this.#type = '';
      // The incomplete event a dropped stream ended in is discarded with its `id:` (the buffer
      // goes back to the last dispatched id), as Chrome does.
      this.#idBuffer = this.#lastEventId;
      this.#request = connect(this.#url, this.#withCredentials, this.#lastEventId, (kind, a, b, c) => this.#report(kind, a, b, c));
    }

    #report(kind, a, b, c) {
      if (this.#readyState === CLOSED) return;
      switch (kind) {
        case 'open': {
          const status = a, type = b;
          if (status !== 200 || type !== 'text/event-stream') {
            this.#fail();
            return;
          }
          this.#origin = c;
          this.#readyState = OPEN;
          this.#fire(new Ev('open'));
          return;
        }
        case 'text':
          if (this.#readyState === OPEN) this.#parse(a);
          return;
        case 'end':
        case 'error':
          this.#request = 0;
          if (kind === 'error' && a === true) this.#fail();
          else this.#reestablish();
          return;
      }
    }

    // "Fail the connection".
    #fail() {
      if (this.#request) { cancel(this.#request); this.#request = 0; }
      this.#readyState = CLOSED;
      this.#fire(new Ev('error'));
    }

    // "Reestablish the connection": error, then a new request after the
    // reconnection time, unless closed meanwhile.
    #reestablish() {
      this.#readyState = CONNECTING;
      this.#fire(new Ev('error'));
      if (this.#readyState !== CONNECTING) return;
      this.#timer = setTimer(() => {
        this.#timer = undefined;
        if (this.#readyState === CONNECTING) this.#connect();
      }, this.#reconnection);
    }

    #parse(chunk) {
      let i = 0;
      if (this.#afterCR && chunk.charCodeAt(0) === 10) i = 1; // CRLF split across chunks
      this.#afterCR = false;
      let start = i;
      for (; i < chunk.length; i++) {
        const c = chunk.charCodeAt(i);
        if (c !== 10 && c !== 13) continue;
        const line = this.#line + chunk.slice(start, i);
        this.#line = '';
        if (c === 13) {
          if (i + 1 < chunk.length) { if (chunk.charCodeAt(i + 1) === 10) i++; }
          else this.#afterCR = true;
        }
        start = i + 1;
        this.#processLine(line);
        if (this.#readyState !== OPEN) return; // closed by a listener
      }
      this.#line += chunk.slice(start);
    }

    #processLine(line) {
      if (line === '') { this.#dispatchEvent(); return; }
      if (line.charCodeAt(0) === 58) return; // ':' comment
      const colon = line.indexOf(':');
      let field = line, value = '';
      if (colon >= 0) {
        field = line.slice(0, colon);
        value = line.slice(colon + 1);
        if (value.charCodeAt(0) === 32) value = value.slice(1);
      }
      switch (field) {
        case 'event': this.#type = value; break;
        case 'data': this.#data += value + '\n'; break;
        case 'id': if (value.indexOf('\0') < 0) this.#idBuffer = value; break;
        case 'retry': if (DIGITS.test(value)) this.#reconnection = +value; break;
      }
    }

    #dispatchEvent() {
      this.#lastEventId = this.#idBuffer;
      if (this.#data === '') { this.#type = ''; return; }
      const data = this.#data.slice(0, -1);
      const type = this.#type || 'message';
      this.#data = '';
      this.#type = '';
      message(this, type, data, this.#origin, this.#lastEventId);
    }
  }

  for (const [name, value] of [['CONNECTING', CONNECTING], ['OPEN', OPEN], ['CLOSED', CLOSED]]) {
    Object.defineProperty(EventSource, name, { value, enumerable: true });
    Object.defineProperty(EventSource.prototype, name, { value, enumerable: true });
  }
  // Web IDL members are enumerable.
  for (const name of ['url', 'withCredentials', 'readyState', 'onopen', 'onmessage', 'onerror', 'close']) {
    const desc = Object.getOwnPropertyDescriptor(EventSource.prototype, name);
    desc.enumerable = true;
    Object.defineProperty(EventSource.prototype, name, desc);
  }
  Object.defineProperty(EventSource.prototype, Symbol.toStringTag, { value: 'EventSource', configurable: true });
  return { EventSource };
});
