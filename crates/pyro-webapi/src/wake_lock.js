// Screen Wake Lock (https://w3c.github.io/screen-wake-lock/) for one window realm: `WakeLock`,
// `WakeLockSentinel` and `navigator.wakeLock`, made on first use. A headless browser has no screen to keep
// awake, so a sentinel is the spec's state machine alone: the document's set of active locks, `released`, the
// `release` event, and the loss of every lock when the document stops being visible or fully active.
// `native` reads what the engine owns:
//   active()            the document is fully active
//   hidden()            document.hidden (the visibility state is "hidden")
//   allowed()           the document's permissions policy allows `screen-wake-lock` (a refusal is also reported)
//   permission()        'granted' | 'denied' | 'prompt' of the `screen-wake-lock` permission
//   dispatchTrusted(target, event)   delivers `event` at `target` with isTrusted true
(function (native) {
  'use strict';
  const O = Object;
  const defineProperty = O.defineProperty;
  const PromiseC = Promise;
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const EventC = Event;
  const EventTargetC = EventTarget;
  const addListener = EventTargetC.prototype.addEventListener;
  // The Window's own method, taken now so a page replacing it later cannot hide the document's loss of visibility.
  const windowAddListener = globalThis.addEventListener;
  const token = Symbol('pyro');
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const notAllowed = (message) => new DOMExceptionC(message, 'NotAllowedError');

  // WebIDL attributes and operations are enumerable members of the prototype.
  const slotOf = (map, self, name) => {
    const d = map.get(self);
    if (d === undefined) throw new TypeErrorC(`Failed to read the '${name}' property from 'WakeLockSentinel': Illegal invocation`);
    return d;
  };
  const enumerable = (C, name) => {
    const d = O.getOwnPropertyDescriptor(C.prototype, name);
    d.enumerable = true;
    defineProperty(C.prototype, name, d);
  };

  // The document's "[[ActiveLocks]]" for "screen", in the order they were acquired.
  const active = new Set();
  const states = new WeakMap();

  // "Release a wake lock": the lock leaves the document's list, `released` becomes true and `release` is fired
  // at the sentinel (a trusted Event that neither bubbles nor can be cancelled) before anything else happens.
  const release = (sentinel) => {
    const st = states.get(sentinel);
    if (st.released) return;
    st.released = true;
    active.delete(sentinel);
    try {
      native.dispatchTrusted(sentinel, new EventC('release'));
    } catch (e) { /* the document is gone: nobody is left to hear it */ }
  };

  // "Handling document loss of visibility" (and of full activity): a document that is hidden, or is leaving,
  // holds no wake lock. Registered with the first lock, in the capture phase at the Window so a page listener
  // on the document that appended a frame or ran `release()` itself sees the locks already gone.
  let watching = false;
  const watch = () => {
    if (watching) return;
    watching = true;
    windowAddListener.call(globalThis, 'visibilitychange', () => {
      if (!native.hidden() && native.active()) return;
      for (const sentinel of [...active]) release(sentinel);
    }, true);
  };

  class WakeLockSentinel extends EventTargetC {
    constructor(t, type) {
      if (t !== token) throw illegal('WakeLockSentinel');
      super();
      defineProperty(this, Symbol.toStringTag, { value: 'WakeLockSentinel', configurable: true });
      states.set(this, { type, released: false, handler: null, installed: false });
    }
    // Promise<undefined> release(): idempotent, and the event has fired when the promise resolves.
    release() {
      const st = states.get(this);
      if (st === undefined) {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'release' on 'WakeLockSentinel': Illegal invocation"));
      }
      release(this);
      return PromiseC.resolve(undefined);
    }
  }
  for (const name of ['released', 'type']) {
    const d = O.getOwnPropertyDescriptor({ get [name]() { return slotOf(states, this, name)[name]; } }, name);
    d.enumerable = true;
    defineProperty(WakeLockSentinel.prototype, name, d);
  }
  // onrelease: the first non-null assignment registers the one listener, as an event handler attribute does.
  defineProperty(WakeLockSentinel.prototype, 'onrelease', {
    get() {
      return slotOf(states, this, 'onrelease').handler;
    },
    set(v) {
      const st = slotOf(states, this, 'onrelease');
      st.handler = typeof v === 'function' || (v !== null && typeof v === 'object') ? v : null;
      if (st.handler !== null && !st.installed) {
        st.installed = true;
        addListener.call(this, 'release', (e) => {
          const h = st.handler;
          if (typeof h === 'function') h.call(this, e);
        });
      }
    },
    enumerable: true,
    configurable: true,
  });
  enumerable(WakeLockSentinel, 'release');
  tag(WakeLockSentinel, 'WakeLockSentinel');

  class WakeLock {
    constructor(t) {
      if (t !== token) throw illegal('WakeLock');
    }
    // Promise<WakeLockSentinel> request(optional WakeLockType type = "screen")
    request(type = 'screen') {
      const where = "Failed to execute 'request' on 'WakeLock'";
      if (!wakeLocks.has(this)) {
        return PromiseC.reject(new TypeErrorC(`${where}: Illegal invocation`));
      }
      // The enumeration conversion comes first: anything but "screen" is a TypeError.
      let name;
      try {
        name = `${type}`;
      } catch (e) {
        return PromiseC.reject(e);
      }
      if (name !== 'screen') {
        return PromiseC.reject(new TypeErrorC(`${where}: The provided value '${name}' is not a valid enum value of type WakeLockType.`));
      }
      // 1-2. The document must be fully active (a navigated-away or removed frame's is not); the engine
      // cannot answer for a realm whose document is gone, which is the same refusal.
      let refusal = null;
      try {
        if (!native.active()) refusal = 'The document is not fully active.';
        // 3. Its permissions policy must allow `screen-wake-lock`.
        else if (!native.allowed()) refusal = 'Access to Screen Wake Lock features is disallowed by permissions policy.';
        // 4. A hidden document may not take a lock.
        else if (native.hidden()) refusal = 'The requesting page is not visible.';
        // 5. The permission: a refusal aborts. Chrome never prompts for a screen lock, so only "denied" refuses.
        else if (native.permission() === 'denied') refusal = 'Wake Lock permission request denied.';
      } catch (e) {
        refusal = 'The document is not fully active.';
      }
      if (refusal !== null) return PromiseC.reject(notAllowed(`${where}: ${refusal}`));
      watch();
      const sentinel = new WakeLockSentinel(token, 'screen');
      active.add(sentinel);
      return PromiseC.resolve(sentinel);
    }
  }
  enumerable(WakeLock, 'request');
  tag(WakeLock, 'WakeLock');
  for (const C of [WakeLock, WakeLockSentinel]) defineProperty(C, 'length', { value: 0, configurable: true });

  const wakeLocks = new WeakSet();
  const wakeLock = new WakeLock(token);
  wakeLocks.add(wakeLock);

  return { wakeLock, WakeLock, WakeLockSentinel };
})
