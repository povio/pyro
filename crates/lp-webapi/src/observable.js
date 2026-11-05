// WICG Observable (https://wicg.github.io/observable/), made on a realm's
// first use. Returns the module object: { Observable, Subscriber, when }.
(() => {
  'use strict';
  const INTERNAL = Symbol('internal');

  function reportException(e) {
    try {
      const ev = new ErrorEvent('error', { error: e, message: String(e && e.message !== undefined ? e.message : e), cancelable: true });
      globalThis.dispatchEvent(ev);
    } catch (_) {
      setTimeout(() => { throw e; }, 0);
    }
  }

  function isObject(v) { return v !== null && (typeof v === 'object' || typeof v === 'function'); }

  class Subscriber {
    #next; #error; #complete;
    #active = true;
    #controller = new AbortController();
    #teardowns = [];
    constructor(token, observer) {
      if (token !== INTERNAL) throw new TypeError('Illegal constructor');
      this.#next = observer.next; this.#error = observer.error; this.#complete = observer.complete;
    }
    get active() { return this.#active; }
    get signal() { return this.#controller.signal; }
    next(value) {
      if (!this.#active) return;
      if (this.#next) { try { this.#next(value); } catch (e) { reportException(e); } }
    }
    error(error) {
      if (!this.#active) { reportException(error); return; }
      const cb = this.#error;
      this.#close(error);
      if (cb) { try { cb(error); } catch (e) { reportException(e); } } else reportException(error);
    }
    complete() {
      if (!this.#active) return;
      const cb = this.#complete;
      this.#close(undefined);
      if (cb) { try { cb(); } catch (e) { reportException(e); } }
    }
    addTeardown(teardown) {
      if (typeof teardown !== 'function') throw new TypeError('teardown must be a function');
      if (this.#active) this.#teardowns.push(teardown);
      else { try { teardown(); } catch (e) { reportException(e); } }
    }
    #close(reason) {
      if (!this.#active) return;
      this.#active = false;
      this.#controller.abort(reason);
      const teardowns = this.#teardowns; this.#teardowns = [];
      for (let i = teardowns.length - 1; i >= 0; i--) { try { teardowns[i](); } catch (e) { reportException(e); } }
    }
    [INTERNAL](reason) { this.#close(reason); }
  }
  Object.defineProperty(Subscriber.prototype, Symbol.toStringTag, { value: 'Subscriber', configurable: true });

  function toObserver(observer) {
    if (typeof observer === 'function') return { next: observer, error: null, complete: null };
    if (observer === undefined || observer === null) return { next: null, error: null, complete: null };
    if (!isObject(observer)) throw new TypeError('observer must be an object or a function');
    const next = observer.next, error = observer.error, complete = observer.complete;
    for (const [k, f] of [['next', next], ['error', error], ['complete', complete]]) {
      if (f !== undefined && f !== null && typeof f !== 'function') throw new TypeError(`observer.${k} is not a function`);
    }
    return { next: next ? next.bind(observer) : null, error: error ? error.bind(observer) : null, complete: complete ? complete.bind(observer) : null };
  }

  class Observable {
    #callback;
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError("Failed to construct 'Observable': parameter 1 is not of type 'Function'");
      this.#callback = callback;
    }
    static [INTERNAL](callback) { return new Observable(callback); }
    subscribe(observer = undefined, options = undefined) {
      if (options !== undefined && !isObject(options)) throw new TypeError('options must be a dictionary');
      const obs = toObserver(observer);
      const subscriber = new Subscriber(INTERNAL, obs);
      const signal = options ? options.signal : undefined;
      if (signal !== undefined && signal !== null) {
        if (!(signal instanceof AbortSignal)) throw new TypeError('signal must be an AbortSignal');
        if (signal.aborted) { subscriber[INTERNAL](signal.reason); return; }
        signal.addEventListener('abort', () => subscriber[INTERNAL](signal.reason), { once: true, signal: subscriber.signal });
      }
      try { this.#callback(subscriber); } catch (e) { subscriber.error(e); }
    }
    static from(value) {
      if (value instanceof Observable) return value;
      if (!isObject(value)) throw new TypeError('Observable.from(): value is not an object');
      const asyncIter = value[Symbol.asyncIterator];
      if (asyncIter !== undefined && asyncIter !== null) {
        if (typeof asyncIter !== 'function') throw new TypeError('Symbol.asyncIterator is not a function');
        return Observable[INTERNAL](subscriber => {
          let iterator;
          try { iterator = asyncIter.call(value); } catch (e) { subscriber.error(e); return; }
          if (!isObject(iterator)) { subscriber.error(new TypeError('async iterator is not an object')); return; }
          subscriber.addTeardown(() => { try { if (typeof iterator.return === 'function') iterator.return(); } catch (_) {} });
          const step = () => {
            if (!subscriber.active) return;
            let p;
            try { p = iterator.next(); } catch (e) { subscriber.error(e); return; }
            Promise.resolve(p).then(r => {
              if (!subscriber.active) return;
              if (!isObject(r)) { subscriber.error(new TypeError('iterator result is not an object')); return; }
              if (r.done) subscriber.complete(); else { subscriber.next(r.value); step(); }
            }, e => subscriber.error(e));
          };
          step();
        });
      }
      const iter = value[Symbol.iterator];
      if (iter !== undefined && iter !== null) {
        if (typeof iter !== 'function') throw new TypeError('Symbol.iterator is not a function');
        return Observable[INTERNAL](subscriber => {
          let iterator;
          try {
            const method = value[Symbol.iterator];
            if (typeof method !== 'function') throw new TypeError('Symbol.iterator is not a function');
            iterator = method.call(value);
          } catch (e) { subscriber.error(e); return; }
          if (!isObject(iterator)) { subscriber.error(new TypeError('iterator is not an object')); return; }
          subscriber.addTeardown(() => { try { if (typeof iterator.return === 'function') iterator.return(); } catch (_) {} });
          for (;;) {
            if (!subscriber.active) return;
            let r;
            try { r = iterator.next(); } catch (e) { subscriber.error(e); return; }
            if (!isObject(r)) { subscriber.error(new TypeError('iterator result is not an object')); return; }
            if (r.done) { subscriber.complete(); return; }
            subscriber.next(r.value);
          }
        });
      }
      if (typeof value.then === 'function') {
        return Observable[INTERNAL](subscriber => {
          Promise.resolve(value).then(v => { subscriber.next(v); subscriber.complete(); }, e => subscriber.error(e));
        });
      }
      throw new TypeError('Observable.from(): value is not observable');
    }
    takeUntil(notifier) {
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let n;
        try { n = Observable.from(notifier); } catch (e) { subscriber.error(e); return; }
        n.subscribe({ next: () => subscriber.complete(), error: () => subscriber.complete() }, { signal: subscriber.signal });
        if (!subscriber.active) return;
        source.subscribe({ next: v => subscriber.next(v), error: e => subscriber.error(e), complete: () => subscriber.complete() }, { signal: subscriber.signal });
      });
    }
    map(mapper) {
      if (typeof mapper !== 'function') throw new TypeError('mapper must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let idx = 0;
        source.subscribe({
          next: v => { let m; try { m = mapper(v, idx++); } catch (e) { subscriber.error(e); return; } subscriber.next(m); },
          error: e => subscriber.error(e), complete: () => subscriber.complete(),
        }, { signal: subscriber.signal });
      });
    }
    filter(predicate) {
      if (typeof predicate !== 'function') throw new TypeError('predicate must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let idx = 0;
        source.subscribe({
          next: v => { let ok; try { ok = predicate(v, idx++); } catch (e) { subscriber.error(e); return; } if (ok) subscriber.next(v); },
          error: e => subscriber.error(e), complete: () => subscriber.complete(),
        }, { signal: subscriber.signal });
      });
    }
    take(amount) {
      amount = toCount(amount);
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let remaining = amount;
        if (remaining === 0) { subscriber.complete(); return; }
        source.subscribe({
          next: v => { if (remaining > 0) { remaining--; subscriber.next(v); } if (remaining === 0) subscriber.complete(); },
          error: e => subscriber.error(e), complete: () => subscriber.complete(),
        }, { signal: subscriber.signal });
      });
    }
    drop(amount) {
      amount = toCount(amount);
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let remaining = amount;
        source.subscribe({
          next: v => { if (remaining > 0) { remaining--; return; } subscriber.next(v); },
          error: e => subscriber.error(e), complete: () => subscriber.complete(),
        }, { signal: subscriber.signal });
      });
    }
    flatMap(mapper) {
      if (typeof mapper !== 'function') throw new TypeError('mapper must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let idx = 0, queue = [], activeInner = false, outerDone = false;
        const flatten = value => {
          let inner;
          try { inner = Observable.from(mapper(value, idx++)); } catch (e) { subscriber.error(e); return; }
          activeInner = true;
          inner.subscribe({
            next: v => subscriber.next(v),
            error: e => subscriber.error(e),
            complete: () => {
              activeInner = false;
              if (queue.length) flatten(queue.shift());
              else if (outerDone) subscriber.complete();
            },
          }, { signal: subscriber.signal });
        };
        source.subscribe({
          next: v => { if (activeInner) queue.push(v); else flatten(v); },
          error: e => subscriber.error(e),
          complete: () => { outerDone = true; if (!activeInner && !queue.length) subscriber.complete(); },
        }, { signal: subscriber.signal });
      });
    }
    switchMap(mapper) {
      if (typeof mapper !== 'function') throw new TypeError('mapper must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        let idx = 0, innerController = null, outerDone = false;
        source.subscribe({
          next: v => {
            if (innerController) innerController.abort();
            let inner;
            try { inner = Observable.from(mapper(v, idx++)); } catch (e) { subscriber.error(e); return; }
            const ac = new AbortController();
            innerController = ac;
            const signal = AbortSignal.any([ac.signal, subscriber.signal]);
            inner.subscribe({
              next: x => subscriber.next(x),
              error: e => subscriber.error(e),
              complete: () => { if (innerController === ac) { innerController = null; if (outerDone) subscriber.complete(); } },
            }, { signal });
          },
          error: e => subscriber.error(e),
          complete: () => { outerDone = true; if (!innerController) subscriber.complete(); },
        }, { signal: subscriber.signal });
      });
    }
    inspect(inspector = undefined) {
      let callbacks;
      if (typeof inspector === 'function') callbacks = { next: inspector };
      else if (inspector === undefined || inspector === null) callbacks = {};
      else if (isObject(inspector)) {
        callbacks = {};
        for (const k of ['subscribe', 'next', 'error', 'complete', 'abort']) {
          const f = inspector[k];
          if (f !== undefined && f !== null) { if (typeof f !== 'function') throw new TypeError(`${k} is not a function`); callbacks[k] = f.bind(inspector); }
        }
      } else throw new TypeError('inspector must be an object or a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        if (callbacks.subscribe) { try { callbacks.subscribe(); } catch (e) { subscriber.error(e); return; } }
        if (callbacks.abort) {
          subscriber.signal.addEventListener('abort', () => { try { callbacks.abort(subscriber.signal.reason); } catch (e) { reportException(e); } }, { once: true });
        }
        source.subscribe({
          next: v => { if (callbacks.next) { try { callbacks.next(v); } catch (e) { subscriber.error(e); return; } } subscriber.next(v); },
          error: e => { if (callbacks.error) { try { callbacks.error(e); } catch (e2) { subscriber.error(e2); return; } } subscriber.error(e); },
          complete: () => { if (callbacks.complete) { try { callbacks.complete(); } catch (e) { subscriber.error(e); return; } } subscriber.complete(); },
        }, { signal: subscriber.signal });
      });
    }
    catch(handler) {
      if (typeof handler !== 'function') throw new TypeError('handler must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        source.subscribe({
          next: v => subscriber.next(v),
          error: e => {
            let inner;
            try { inner = Observable.from(handler(e)); } catch (e2) { subscriber.error(e2); return; }
            inner.subscribe({ next: v => subscriber.next(v), error: e2 => subscriber.error(e2), complete: () => subscriber.complete() }, { signal: subscriber.signal });
          },
          complete: () => subscriber.complete(),
        }, { signal: subscriber.signal });
      });
    }
    finally(callback) {
      if (typeof callback !== 'function') throw new TypeError('callback must be a function');
      const source = this;
      return Observable[INTERNAL](subscriber => {
        subscriber.addTeardown(callback);
        source.subscribe({ next: v => subscriber.next(v), error: e => subscriber.error(e), complete: () => subscriber.complete() }, { signal: subscriber.signal });
      });
    }
    toArray(options = undefined) {
      return promised(this, options, (resolve, reject, signal) => {
        const values = [];
        return { next: v => values.push(v), error: reject, complete: () => resolve(values) };
      });
    }
    forEach(callback, options = undefined) {
      if (typeof callback !== 'function') return Promise.reject(new TypeError('callback must be a function'));
      return promised(this, options, (resolve, reject, controller) => {
        let idx = 0;
        return { next: v => { try { callback(v, idx++); } catch (e) { reject(e); controller.abort(e); } }, error: reject, complete: () => resolve(undefined) };
      });
    }
    every(predicate, options = undefined) {
      if (typeof predicate !== 'function') return Promise.reject(new TypeError('predicate must be a function'));
      return promised(this, options, (resolve, reject, controller) => {
        let idx = 0;
        return {
          next: v => { let ok; try { ok = predicate(v, idx++); } catch (e) { reject(e); controller.abort(e); return; } if (!ok) { resolve(false); controller.abort(); } },
          error: reject, complete: () => resolve(true),
        };
      });
    }
    first(options = undefined) {
      return promised(this, options, (resolve, reject, controller) => ({
        next: v => { resolve(v); controller.abort(); },
        error: reject,
        complete: () => reject(new RangeError('no values in Observable')),
      }));
    }
    last(options = undefined) {
      return promised(this, options, (resolve, reject) => {
        let has = false, last;
        return { next: v => { has = true; last = v; }, error: reject, complete: () => has ? resolve(last) : reject(new RangeError('no values in Observable')) };
      });
    }
    find(predicate, options = undefined) {
      if (typeof predicate !== 'function') return Promise.reject(new TypeError('predicate must be a function'));
      return promised(this, options, (resolve, reject, controller) => {
        let idx = 0;
        return {
          next: v => { let ok; try { ok = predicate(v, idx++); } catch (e) { reject(e); controller.abort(e); return; } if (ok) { resolve(v); controller.abort(); } },
          error: reject, complete: () => resolve(undefined),
        };
      });
    }
    some(predicate, options = undefined) {
      if (typeof predicate !== 'function') return Promise.reject(new TypeError('predicate must be a function'));
      return promised(this, options, (resolve, reject, controller) => {
        let idx = 0;
        return {
          next: v => { let ok; try { ok = predicate(v, idx++); } catch (e) { reject(e); controller.abort(e); return; } if (ok) { resolve(true); controller.abort(); } },
          error: reject, complete: () => resolve(false),
        };
      });
    }
    reduce(reducer, ...rest) {
      if (typeof reducer !== 'function') return Promise.reject(new TypeError('reducer must be a function'));
      const hasInitial = rest.length > 0;
      const options = rest[1];
      return promised(this, options, (resolve, reject, controller) => {
        let idx = 0, acc = rest[0], seeded = hasInitial;
        return {
          next: v => {
            if (!seeded) { acc = v; seeded = true; idx++; return; }
            try { acc = reducer(acc, v, idx++); } catch (e) { reject(e); controller.abort(e); }
          },
          error: reject,
          complete: () => seeded ? resolve(acc) : reject(new TypeError('Reduce of empty Observable with no initial value')),
        };
      });
    }
  }
  Object.defineProperty(Observable.prototype, Symbol.toStringTag, { value: 'Observable', configurable: true });

  function toCount(v) {
    const n = Number(v);
    if (Number.isNaN(n)) return 0;
    if (n < 0) return 0;
    return Math.min(n, Number.MAX_SAFE_INTEGER);
  }

  // The promise-returning operators share the subscription plumbing.
  function promised(source, options, build) {
    if (options !== undefined && options !== null && !isObject(options)) return Promise.reject(new TypeError('options must be a dictionary'));
    return new Promise((resolve, reject) => {
      const signal = options ? options.signal : undefined;
      if (signal !== undefined && signal !== null) {
        if (!(signal instanceof AbortSignal)) { reject(new TypeError('signal must be an AbortSignal')); return; }
        if (signal.aborted) { reject(signal.reason); return; }
      }
      const controller = new AbortController();
      const combined = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
      if (signal) signal.addEventListener('abort', () => reject(signal.reason), { once: true, signal: controller.signal });
      const observer = build(resolve, reject, controller);
      source.subscribe(observer, { signal: combined });
    });
  }

  function when(target, type, options) {
    type = String(type);
    const capture = !!(options && options.capture);
    const passive = options && options.passive !== undefined ? !!options.passive : undefined;
    return Observable[INTERNAL](subscriber => {
      const init = { capture, signal: subscriber.signal };
      if (passive !== undefined) init.passive = passive;
      target.addEventListener(type, e => subscriber.next(e), init);
    });
  }

  return { Observable, Subscriber, when };
})()
