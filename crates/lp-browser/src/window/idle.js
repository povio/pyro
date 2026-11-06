// requestIdleCallback's IdleDeadline (w3c/requestidlecallback), made on a
// window's first idle callback or first read of `IdleDeadline`. `now` is the
// window's performance.now() clock; `make(deadline, didTimeout)` makes the
// argument of one callback.
(function (now) {
  "use strict";
  const token = Symbol("IdleDeadline");
  class IdleDeadline {
    #deadline;
    #didTimeout;
    constructor(key, deadline, didTimeout) {
      if (key !== token) throw new TypeError("Illegal constructor");
      this.#deadline = deadline;
      this.#didTimeout = didTimeout;
    }
    timeRemaining() {
      const remaining = this.#deadline - now();
      return remaining > 0 ? remaining : 0;
    }
    get didTimeout() {
      return this.#didTimeout;
    }
  }
  const proto = IdleDeadline.prototype;
  // WebIDL: operations and attributes are enumerable; no constructor.
  for (const key of ["timeRemaining", "didTimeout"]) {
    const desc = Object.getOwnPropertyDescriptor(proto, key);
    desc.enumerable = true;
    Object.defineProperty(proto, key, desc);
  }
  Object.defineProperty(proto, Symbol.toStringTag, { value: "IdleDeadline", configurable: true });
  Object.defineProperty(IdleDeadline, "length", { value: 0 });
  return {
    IdleDeadline,
    make: (deadline, didTimeout) => new IdleDeadline(token, deadline, didTimeout),
  };
})
