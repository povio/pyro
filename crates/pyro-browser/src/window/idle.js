// requestIdleCallback's IdleDeadline (w3c/requestidlecallback), made on a
// window's first idle callback or first read of `IdleDeadline`. `now` is the
// window's performance.now() clock; `make(start, length, didTimeout)` makes the
// argument of one callback for an idle period of `length` ms that began at `start`.
(function (now) {
  "use strict";
  const token = Symbol("IdleDeadline");
  class IdleDeadline {
    #start;
    #length;
    #didTimeout;
    constructor(key, start, length, didTimeout) {
      if (key !== token) throw new TypeError("Illegal constructor");
      this.#start = start;
      this.#length = length;
      this.#didTimeout = didTimeout;
    }
    timeRemaining() {
      // length - elapsed, not (start + length) - now: the sum rounds, and a
      // result a few ulps above the 50 ms cap breaks `timeRemaining() <= 50`.
      const elapsed = now() - this.#start;
      const remaining = this.#length - (elapsed > 0 ? elapsed : 0);
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
    make: (start, length, didTimeout) => new IdleDeadline(token, start, length, didTimeout),
  };
})
