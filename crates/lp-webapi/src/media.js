// TimeRanges, MediaError and TextTrackList for one realm, made on first use
// (media.rs). There is no media pipeline: every TimeRanges is empty and no
// text track is ever added.
(function () {
  'use strict';
  const secret = Symbol('lightningpanda');
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const EventTargetC = EventTarget;
  const addListener = EventTargetC.prototype.addEventListener;
  const removeListener = EventTargetC.prototype.removeEventListener;
  const apply = Reflect.apply;

  function illegal() { return new TypeErrorC('Illegal constructor'); }
  function webidl(C, tag) {
    for (const name of getOwnPropertyNames(C.prototype)) {
      if (name === 'constructor') continue;
      const desc = getOwnPropertyDescriptor(C.prototype, name);
      desc.enumerable = true;
      defineProperty(C.prototype, name, desc);
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  const ranges = new WeakMap();
  function rangesOf(self) {
    const r = ranges.get(self);
    if (!r) throw new TypeErrorC('Illegal invocation');
    return r;
  }
  function index(r, i, what) {
    const n = +i >>> 0;
    if (n >= r.length) {
      throw new DOMExceptionC(`Failed to execute '${what}' on 'TimeRanges': The index provided (${n}) is greater than or equal to the maximum bound (${r.length}).`, 'IndexSizeError');
    }
    return n;
  }
  class TimeRanges {
    constructor(token, list) {
      if (token !== secret) throw illegal();
      ranges.set(this, list);
    }
    get length() { return rangesOf(this).length; }
    start(i) { const r = rangesOf(this); return r[index(r, i, 'start')][0]; }
    end(i) { const r = rangesOf(this); return r[index(r, i, 'end')][1]; }
  }
  webidl(TimeRanges, 'TimeRanges');

  const errors = new WeakMap();
  function errorOf(self) {
    const e = errors.get(self);
    if (!e) throw new TypeErrorC('Illegal invocation');
    return e;
  }
  class MediaError {
    constructor(token, code, message) {
      if (token !== secret) throw illegal();
      errors.set(this, { code, message });
    }
    get code() { return errorOf(this).code; }
    get message() { return errorOf(this).message; }
  }
  webidl(MediaError, 'MediaError');
  const CODES = { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 };
  for (const name of Object.keys(CODES)) {
    for (const target of [MediaError, MediaError.prototype]) {
      defineProperty(target, name, { value: CODES[name], enumerable: true });
    }
  }

  // No track is ever added: an empty list whose event handlers work.
  const handlers = new WeakMap();
  class TextTrackList extends EventTargetC {
    constructor(token) {
      if (token !== secret) throw illegal();
      super();
      handlers.set(this, { __proto__: null });
    }
    get length() { if (!handlers.has(this)) throw new TypeErrorC('Illegal invocation'); return 0; }
    getTrackById(id) { if (!handlers.has(this)) throw new TypeErrorC('Illegal invocation'); String(id); return null; }
  }
  for (const type of ['change', 'addtrack', 'removetrack']) {
    defineProperty(TextTrackList.prototype, 'on' + type, {
      get() {
        const h = handlers.get(this);
        if (!h) throw new TypeErrorC('Illegal invocation');
        return h[type] ? h[type].value : null;
      },
      set(value) {
        const h = handlers.get(this);
        if (!h) throw new TypeErrorC('Illegal invocation');
        if (h[type]) apply(removeListener, this, [type, h[type].listener]);
        h[type] = null;
        if (typeof value === 'function' || (value !== null && typeof value === 'object')) {
          const listener = function (event) {
            return typeof value === 'function' ? apply(value, this, [event]) : undefined;
          };
          h[type] = { value, listener };
          apply(addListener, this, [type, listener]);
        }
      },
      enumerable: true,
      configurable: true,
    });
  }
  webidl(TextTrackList, 'TextTrackList');

  return {
    TimeRanges, MediaError, TextTrackList,
    timeRanges() { return new TimeRanges(secret, []); },
    mediaError(code, message) { return new MediaError(secret, code, message); },
    textTrackList() { return new TextTrackList(secret); },
  };
})
