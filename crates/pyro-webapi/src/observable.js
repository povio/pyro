// WICG Observable (https://wicg.github.io/observable/), made on a realm's
// first use. The script evaluates to a function of the engine's helpers
// (observable.rs) that returns the module object:
// { Observable, Subscriber, when, runAbortAlgorithm }.
(native => {
  'use strict';
  const INTERNAL = Symbol('internal');
  const { reportException, fullyActive, isAbortSignal, addAbortHook, removeAbortHook } = native;
  const apply = Reflect.apply;
  const P = Promise;
  const promiseThen = P.prototype.then;
  const promiseResolve = v => apply(P.resolve, P, [v]);
  const AC = AbortController;
  const controllerAbort = AC.prototype.abort;
  const anySignal = AbortSignal.any;
  const addEventListener = EventTarget.prototype.addEventListener;
  const WR = WeakRef;
  const deref = WR.prototype.deref;
  const getter = (proto, name) => {
    const d = Object.getOwnPropertyDescriptor(proto, name);
    return d && d.get ? o => apply(d.get, o, []) : o => o[name];
  };
  const aborted = getter(AbortSignal.prototype, 'aborted');
  const abortReason = getter(AbortSignal.prototype, 'reason');
  const signalOf = getter(AC.prototype, 'signal');
  const TWO_64 = 18446744073709551616;

  function isObject(v) { return v !== null && (typeof v === 'object' || typeof v === 'function'); }
  function remove(list, item) { const i = list.indexOf(item); if (i >= 0) list.splice(i, 1); }

  // DOM "abort algorithms" of this module. The engine runs each one when its
  // signal aborts, in the order they were added among the signal's other
  // abort steps and before the `abort` event fires; an exception one throws
  // leaves through the `abort()` call that started the abort. Steps that
  // became moot (their subscription ended, their promise settled) are
  // removed, so a long-lived signal does not keep what they reach.
  const algorithms = new WeakMap();
  let lastAlgorithm = 0;
  function addAbortAlgorithm(signal, steps) {
    if (aborted(signal)) return 0;
    let map = algorithms.get(signal);
    if (!map) algorithms.set(signal, map = new Map());
    const key = ++lastAlgorithm;
    map.set(key, { steps, hook: addAbortHook(signal, key) });
    return key;
  }
  function removeAbortAlgorithm(signal, key) {
    const map = algorithms.get(signal);
    const algorithm = map && map.get(key);
    if (!algorithm) return;
    map.delete(key);
    removeAbortHook(signal, algorithm.hook);
  }
  function runAbortAlgorithm(signal, key) {
    const map = algorithms.get(signal);
    const algorithm = map && map.get(key);
    if (!algorithm) return;
    map.delete(key);
    algorithm.steps();
  }

  // Web IDL "invoke a callback function" with "report" (the callback this
  // value is undefined).
  function invokeReport(callback, args) {
    try { apply(callback, undefined, args); } catch (e) { reportException(e); }
  }

  // An "internal observer". The default error steps are the "default error
  // algorithm": report the exception.
  const noop = () => {};
  function internalObserver(next, error, complete) {
    return { next: next || noop, error: error || reportException, complete: complete || noop };
  }

  function callbackMember(dictionary, name) {
    const f = dictionary[name];
    if (f === undefined) return null;
    if (typeof f !== 'function') throw new TypeError(`The provided value for '${name}' is not a function.`);
    return f;
  }
  function callbackArgument(f, what) {
    if (typeof f !== 'function') throw new TypeError(`${what} is not a function.`);
    return f;
  }
  function requireArguments(args, count, what) {
    if (args.length < count) throw new TypeError(`${what}: ${count} argument${count > 1 ? 's' : ''} required, but only ${args.length} present.`);
  }

  // "Process observer" of an ObserverUnion: a callback is the next handler,
  // a SubscriptionObserver gives up to three (members are read in
  // lexicographical order).
  function processObserver(observer) {
    if (typeof observer === 'function') return internalObserver(v => invokeReport(observer, [v]));
    if (observer === undefined || observer === null) return internalObserver();
    if (!isObject(observer)) throw new TypeError("The provided value is not of type '(ObservableSubscriptionCallback or SubscriptionObserver)'.");
    const complete = callbackMember(observer, 'complete');
    const error = callbackMember(observer, 'error');
    const next = callbackMember(observer, 'next');
    return internalObserver(
      next && (v => invokeReport(next, [v])),
      error && (e => invokeReport(error, [e])),
      complete && (() => invokeReport(complete, [])));
  }

  // SubscribeOptions' signal, undefined when it does not exist.
  function signalOption(options) {
    if (options === undefined || options === null) return undefined;
    if (!isObject(options)) throw new TypeError("The provided value is not of type 'SubscribeOptions'.");
    const signal = options.signal;
    if (signal === undefined) return undefined;
    if (!isAbortSignal(signal)) throw new TypeError("Failed to read the 'signal' property from 'SubscribeOptions': Failed to convert value to 'AbortSignal'.");
    return signal;
  }

  // Web IDL `unsigned long long` (no [Clamp], no [EnforceRange]).
  function toUnsignedLongLong(v) {
    let n = +v;
    if (n !== n || n === Infinity || n === -Infinity) return 0;
    n = Math.trunc(n) % TWO_64;
    if (n < 0) n += TWO_64;
    return n + 0;
  }

  let ops; // Subscriber internals, for this module only.
  class Subscriber {
    #observers = [];
    #teardowns = [];
    #controller = new AC();
    #active = true;
    #unsubscribers = []; // [signal, abort algorithm] of each observer's signal
    constructor(token) {
      if (token !== INTERNAL) throw new TypeError('Illegal constructor');
    }
    get active() { return this.#active; }
    get signal() { return signalOf(this.#controller); }
    next(value) {
      if (!(#active in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'next' on 'Subscriber'");
      this.#next(value);
    }
    error(error) {
      if (!(#active in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'error' on 'Subscriber'");
      this.#error(error);
    }
    complete() {
      if (!(#active in this)) throw new TypeError('Illegal invocation');
      this.#complete();
    }
    addTeardown(teardown) {
      if (!(#active in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'addTeardown' on 'Subscriber'");
      this.#addTeardown(callbackArgument(teardown, "Failed to execute 'addTeardown' on 'Subscriber': parameter 1"));
    }
    #next(value) {
      if (!this.#active || !fullyActive()) return;
      // A copy: a next step may subscribe (or unsubscribe) another observer.
      const observers = this.#observers.slice();
      for (let i = 0; i < observers.length; i++) observers[i].next(value);
    }
    #error(error) {
      if (!this.#active) { reportException(error); return; }
      if (!fullyActive()) return;
      const failure = this.#close(error);
      const observers = this.#observers.slice();
      for (let i = 0; i < observers.length; i++) observers[i].error(error);
      if (failure) throw failure.exception;
    }
    #complete() {
      if (!this.#active || !fullyActive()) return;
      const failure = this.#close(undefined);
      const observers = this.#observers.slice();
      for (let i = 0; i < observers.length; i++) observers[i].complete();
      if (failure) throw failure.exception;
    }
    #addTeardown(teardown) {
      if (!fullyActive()) return;
      if (this.#active) this.#teardowns.push(teardown);
      else invokeReport(teardown, []);
    }
    // "Close a subscription". Returns { exception } when an abort algorithm
    // of the subscription controller's signal threw (an iterator's
    // return()): the caller finishes its own steps and rethrows it.
    #close(reason) {
      if (!this.#active) return null;
      this.#active = false;
      let failure = null;
      try { apply(controllerAbort, this.#controller, [reason]); } catch (exception) { failure = { exception }; }
      const teardowns = this.#teardowns;
      for (let i = teardowns.length - 1; i >= 0; i--) {
        // Each teardown can detach the document.
        if (!fullyActive()) break;
        invokeReport(teardowns[i], []);
      }
      this.#teardowns = [];
      // Nothing is left to unsubscribe from.
      const unsubscribers = this.#unsubscribers;
      this.#unsubscribers = [];
      for (let i = 0; i < unsubscribers.length; i++) removeAbortAlgorithm(unsubscribers[i][0], unsubscribers[i][1]);
      return failure;
    }
    static {
      ops = {
        active: s => s.#active,
        observers: s => s.#observers,
        unsubscribers: s => s.#unsubscribers,
        signal: s => signalOf(s.#controller),
        next: (s, value) => s.#next(value),
        error: (s, error) => s.#error(error),
        complete: s => s.#complete(),
        addTeardown: (s, teardown) => s.#addTeardown(teardown),
        close: (s, reason) => { const failure = s.#close(reason); if (failure) throw failure.exception; },
      };
    }
  }
  Object.defineProperty(Subscriber.prototype, Symbol.toStringTag, { value: 'Subscriber', configurable: true });

  // ECMAScript iterator records: { iterator, next, done }.
  function getMethod(object, key) {
    const f = object[key];
    if (f === undefined || f === null) return undefined;
    if (typeof f !== 'function') throw new TypeError(`${String(key)} is not a function`);
    return f;
  }
  function getIteratorFromMethod(object, method) {
    const iterator = apply(method, object, []);
    if (!isObject(iterator)) throw new TypeError('The iterator is not an Object');
    return { iterator, next: iterator.next, done: false };
  }
  function getIterator(object, async) {
    if (async) {
      const method = getMethod(object, Symbol.asyncIterator);
      if (method !== undefined) return getIteratorFromMethod(object, method);
      const syncMethod = getMethod(object, Symbol.iterator);
      if (syncMethod === undefined) throw new TypeError('The object is not async iterable');
      return asyncFromSyncIterator(getIteratorFromMethod(object, syncMethod));
    }
    const method = getMethod(object, Symbol.iterator);
    if (method === undefined) throw new TypeError('The object is not iterable');
    return getIteratorFromMethod(object, method);
  }
  function iteratorNext(record) {
    let result;
    try { result = apply(record.next, record.iterator, []); } catch (e) { record.done = true; throw e; }
    if (!isObject(result)) { record.done = true; throw new TypeError('The iterator result is not an Object'); }
    return result;
  }
  const DONE = Symbol('done');
  function iteratorStepValue(record) {
    const result = iteratorNext(record);
    try {
      if (result.done) { record.done = true; return DONE; }
      return result.value;
    } catch (e) { record.done = true; throw e; }
  }
  // IteratorClose(record, NormalCompletion): an exhausted iterator is not closed.
  function iteratorClose(record) {
    if (record.done) return;
    const method = getMethod(record.iterator, 'return');
    if (method === undefined) return;
    const result = apply(method, record.iterator, []);
    if (!isObject(result)) throw new TypeError('The return() method of the iterator did not return an Object');
  }
  // AsyncIteratorClose(record, NormalCompletion(reason)), not awaited: a
  // failure is a rejected promise nobody handles.
  function asyncIteratorClose(record, reason) {
    if (record.done) return;
    let promise;
    try {
      const method = getMethod(record.iterator, 'return');
      if (method === undefined) return;
      promise = promiseResolvedWith(apply(method, record.iterator, [reason]));
    } catch (e) { promise = promiseRejectedWith(e); }
    apply(promiseThen, promise, [result => {
      if (!isObject(result)) throw new TypeError('The return() method of the async iterator did not return an Object');
    }]);
  }
  // CreateAsyncFromSyncIterator.
  function asyncFromSyncIterator(sync) {
    const closeAfterRejection = () => { try { iteratorClose(sync); } catch (_) {} };
    const continuation = (result, closeOnRejection) => {
      let done, wrapper;
      try {
        done = !!result.done;
        const value = result.value;
        try { wrapper = promiseResolve(value); } catch (e) { if (!done && closeOnRejection) closeAfterRejection(); throw e; }
      } catch (e) { return promiseRejectedWith(e); }
      return apply(promiseThen, wrapper, [
        value => ({ value, done }),
        !done && closeOnRejection ? e => { closeAfterRejection(); throw e; } : undefined,
      ]);
    };
    const iterator = {
      next() {
        let result;
        try { result = iteratorNext(sync); } catch (e) { return promiseRejectedWith(e); }
        return continuation(result, true);
      },
      return(value) {
        let result;
        try {
          const method = getMethod(sync.iterator, 'return');
          if (method === undefined) return promiseResolvedWith({ value, done: true });
          result = apply(method, sync.iterator, [value]);
          if (!isObject(result)) throw new TypeError('The return() method of the iterator did not return an Object');
        } catch (e) { return promiseRejectedWith(e); }
        return continuation(result, false);
      },
    };
    return { iterator, next: iterator.next, done: false };
  }

  // Web IDL "a promise resolved with" / "rejected with" and "a new promise".
  function promiseResolvedWith(value) { return new P(resolve => resolve(value)); }
  function promiseRejectedWith(reason) { return new P((_, reject) => reject(reason)); }
  function newPromise() {
    let resolve, reject;
    const promise = new P((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }

  // The async iterable conversion's nextAlgorithm.
  function asyncIterableNext(subscriber, record) {
    if (aborted(ops.signal(subscriber))) return;
    let nextPromise;
    try { nextPromise = promiseResolvedWith(iteratorNext(record)); } catch (e) { nextPromise = promiseRejectedWith(e); }
    apply(promiseThen, nextPromise, [result => {
      if (!isObject(result)) {
        record.done = true;
        ops.error(subscriber, new TypeError('The async iterator result is not an Object'));
        return;
      }
      let done, value;
      try { done = !!result.done; } catch (e) { record.done = true; ops.error(subscriber, e); return; }
      if (done) { record.done = true; ops.complete(subscriber); return; }
      try { value = result.value; } catch (e) { record.done = true; ops.error(subscriber, e); return; }
      ops.next(subscriber, value);
      asyncIterableNext(subscriber, record);
    }, reason => { record.done = true; ops.error(subscriber, reason); }]);
  }

  // "Convert to an Observable".
  function toObservable(value) {
    if (!isObject(value)) throw new TypeError('The provided value cannot be converted to an Observable.');
    if (isObservable(value)) return value;
    if (getMethod(value, Symbol.asyncIterator) !== undefined) {
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        if (aborted(signal)) return;
        let record;
        try { record = getIterator(value, true); } catch (e) { ops.error(subscriber, e); return; }
        if (aborted(signal)) return;
        addAbortAlgorithm(signal, () => asyncIteratorClose(record, abortReason(signal)));
        asyncIterableNext(subscriber, record);
      });
    }
    if (getMethod(value, Symbol.iterator) !== undefined) {
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        if (aborted(signal)) return;
        let record;
        try { record = getIterator(value, false); } catch (e) { ops.error(subscriber, e); return; }
        if (aborted(signal)) return;
        addAbortAlgorithm(signal, () => iteratorClose(record));
        for (;;) {
          let next;
          try { next = iteratorStepValue(record); } catch (e) { ops.error(subscriber, e); break; }
          if (next === DONE) { ops.complete(subscriber); return; }
          ops.next(subscriber, next);
          if (aborted(signal)) break;
        }
      });
    }
    if (value instanceof P) {
      return makeObservable(subscriber => {
        apply(promiseThen, value, [
          v => { ops.next(subscriber, v); ops.complete(subscriber); },
          reason => { ops.error(subscriber, reason); },
        ]);
      });
    }
    throw new TypeError('The provided value cannot be converted to an Observable.');
  }

  // The observer of an operator that passes on what its source emits, but
  // for the given next or complete steps.
  function mirror(subscriber, next, complete) {
    return internalObserver(
      next || (value => ops.next(subscriber, value)),
      error => ops.error(subscriber, error),
      complete || (() => ops.complete(subscriber)));
  }

  // A promise-returning operation: an exception rejects the promise.
  function promising(steps) {
    try { return steps(); } catch (e) { return promiseRejectedWith(e); }
  }
  // The promise of an operator, rejected when `signal` aborts: `aborted` is
  // set when it already has.
  function abortable(signal) {
    const p = newPromise();
    if (signal === undefined) return p;
    if (aborted(signal)) { p.reject(abortReason(signal)); p.aborted = true; return p; }
    const { resolve, reject } = p;
    const algorithm = addAbortAlgorithm(signal, () => reject(abortReason(signal)));
    p.resolve = value => { removeAbortAlgorithm(signal, algorithm); resolve(value); };
    p.reject = reason => { removeAbortAlgorithm(signal, algorithm); reject(reason); };
    return p;
  }
  // The dependent signal of the operators that can end their subscription.
  function internalSignal(controller, signal) {
    const signals = signal === undefined ? [signalOf(controller)] : [signalOf(controller), signal];
    return apply(anySignal, AbortSignal, [signals]);
  }

  let makeObservable, subscribeTo, isObservable;
  class Observable {
    #callback;
    #native = false; // the subscribe callback is steps of this module
    #weak = null; // the "weak subscriber"
    constructor(callback) {
      requireArguments(arguments, 1, "Failed to construct 'Observable'");
      this.#callback = callbackArgument(callback, "Failed to construct 'Observable': parameter 1");
    }
    static {
      makeObservable = steps => { const o = new Observable(steps); o.#native = true; return o; };
      subscribeTo = (o, observer, signal) => o.#subscribe(observer, signal);
      isObservable = v => #callback in v;
    }
    // "Subscribe to an Observable" given an internal observer.
    #subscribe(observer, signal) {
      if (!fullyActive()) return;
      let subscriber = this.#weak === null ? undefined : apply(deref, this.#weak, []);
      const joins = subscriber !== undefined && ops.active(subscriber);
      if (!joins) {
        subscriber = new Subscriber(INTERNAL);
        this.#weak = new WR(subscriber);
      }
      const observers = ops.observers(subscriber);
      observers.push(observer);
      if (signal !== undefined) {
        if (aborted(signal)) {
          if (joins) remove(observers, observer);
          else ops.close(subscriber, abortReason(signal));
        } else {
          const algorithm = addAbortAlgorithm(signal, () => {
            if (!ops.active(subscriber)) return;
            remove(observers, observer);
            if (observers.length === 0) ops.close(subscriber, abortReason(signal));
          });
          ops.unsubscribers(subscriber).push([signal, algorithm]);
        }
      }
      if (joins) return;
      const callback = this.#callback;
      if (this.#native) { callback(subscriber); return; }
      try { apply(callback, undefined, [subscriber]); } catch (e) { ops.error(subscriber, e); }
    }
    subscribe(observer = undefined, options = undefined) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      const internal = processObserver(observer);
      this.#subscribe(internal, signalOption(options));
    }
    static from(value) {
      requireArguments(arguments, 1, "Failed to execute 'from' on 'Observable'");
      return toObservable(value);
    }
    takeUntil(value) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'takeUntil' on 'Observable'");
      const notifier = toObservable(value);
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        const stop = () => ops.complete(subscriber);
        subscribeTo(notifier, internalObserver(stop, stop), signal);
        if (!ops.active(subscriber)) return;
        subscribeTo(this, mirror(subscriber), signal);
      });
    }
    map(mapper) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'map' on 'Observable'");
      callbackArgument(mapper, "Failed to execute 'map' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        let idx = 0;
        subscribeTo(this, mirror(subscriber, value => {
          let mapped;
          try { mapped = apply(mapper, undefined, [value, idx]); } catch (e) { ops.error(subscriber, e); return; }
          idx++;
          ops.next(subscriber, mapped);
        }), ops.signal(subscriber));
      });
    }
    filter(predicate) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'filter' on 'Observable'");
      callbackArgument(predicate, "Failed to execute 'filter' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        let idx = 0;
        subscribeTo(this, mirror(subscriber, value => {
          let matches;
          try { matches = apply(predicate, undefined, [value, idx]); } catch (e) { ops.error(subscriber, e); return; }
          idx++;
          if (matches) ops.next(subscriber, value);
        }), ops.signal(subscriber));
      });
    }
    take(amount) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'take' on 'Observable'");
      amount = toUnsignedLongLong(amount);
      return makeObservable(subscriber => {
        let remaining = amount;
        if (remaining === 0) { ops.complete(subscriber); return; }
        subscribeTo(this, mirror(subscriber, value => {
          ops.next(subscriber, value);
          remaining--;
          if (remaining === 0) ops.complete(subscriber);
        }), ops.signal(subscriber));
      });
    }
    drop(amount) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'drop' on 'Observable'");
      amount = toUnsignedLongLong(amount);
      return makeObservable(subscriber => {
        let remaining = amount;
        subscribeTo(this, mirror(subscriber, value => {
          if (remaining > 0) { remaining--; return; }
          ops.next(subscriber, value);
        }), ops.signal(subscriber));
      });
    }
    flatMap(mapper) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'flatMap' on 'Observable'");
      callbackArgument(mapper, "Failed to execute 'flatMap' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        const queue = [];
        let idx = 0, outerSubscriptionHasCompleted = false, activeInnerSubscription = false;
        // The "flatmap process next value steps".
        const process = value => {
          let inner;
          try {
            const mapped = apply(mapper, undefined, [value, idx]);
            idx++;
            inner = toObservable(mapped);
          } catch (e) { ops.error(subscriber, e); return; }
          subscribeTo(inner, mirror(subscriber, null, () => {
            if (queue.length > 0) { process(queue.shift()); return; }
            activeInnerSubscription = false;
            if (outerSubscriptionHasCompleted) ops.complete(subscriber);
          }), signal);
        };
        subscribeTo(this, internalObserver(
          value => {
            if (activeInnerSubscription) { queue.push(value); return; }
            activeInnerSubscription = true;
            process(value);
          },
          error => ops.error(subscriber, error),
          () => {
            outerSubscriptionHasCompleted = true;
            if (!activeInnerSubscription && queue.length === 0) ops.complete(subscriber);
          }), signal);
      });
    }
    switchMap(mapper) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'switchMap' on 'Observable'");
      callbackArgument(mapper, "Failed to execute 'switchMap' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        let idx = 0, outerSubscriptionHasCompleted = false, activeInnerAbortController = null;
        // The "switchmap process next value steps".
        const process = value => {
          let inner;
          try {
            const mapped = apply(mapper, undefined, [value, idx]);
            idx++;
            inner = toObservable(mapped);
          } catch (e) { ops.error(subscriber, e); return; }
          subscribeTo(inner, mirror(subscriber, null, () => {
            if (outerSubscriptionHasCompleted) ops.complete(subscriber);
            else activeInnerAbortController = null;
          }), internalSignal(activeInnerAbortController, signal));
        };
        subscribeTo(this, internalObserver(
          value => {
            if (activeInnerAbortController !== null) apply(controllerAbort, activeInnerAbortController, []);
            activeInnerAbortController = new AC();
            process(value);
          },
          error => ops.error(subscriber, error),
          () => {
            outerSubscriptionHasCompleted = true;
            if (activeInnerAbortController === null) ops.complete(subscriber);
          }), signal);
      });
    }
    inspect(inspectorUnion = undefined) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      let subscribeCallback = null, nextCallback = null, errorCallback = null, completeCallback = null, abortCallback = null;
      if (typeof inspectorUnion === 'function') nextCallback = inspectorUnion;
      else if (isObject(inspectorUnion)) {
        abortCallback = callbackMember(inspectorUnion, 'abort');
        completeCallback = callbackMember(inspectorUnion, 'complete');
        errorCallback = callbackMember(inspectorUnion, 'error');
        nextCallback = callbackMember(inspectorUnion, 'next');
        subscribeCallback = callbackMember(inspectorUnion, 'subscribe');
      } else if (inspectorUnion !== undefined && inspectorUnion !== null) {
        throw new TypeError("The provided value is not of type '(ObservableInspector or ObservableSubscriptionCallback)'.");
      }
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        if (subscribeCallback !== null) {
          try { apply(subscribeCallback, undefined, []); } catch (e) { ops.error(subscriber, e); return; }
        }
        // The abort callback is for consumer-initiated unsubscription only:
        // it is removed before the producer ends the subscription.
        let abortAlgorithm = 0;
        if (abortCallback !== null) {
          abortAlgorithm = addAbortAlgorithm(signal, () => invokeReport(abortCallback, [abortReason(signal)]));
        }
        const removeAbortCallback = () => { if (abortAlgorithm) removeAbortAlgorithm(signal, abortAlgorithm); };
        subscribeTo(this, internalObserver(
          value => {
            if (nextCallback !== null) {
              try { apply(nextCallback, undefined, [value]); } catch (e) {
                removeAbortCallback();
                ops.error(subscriber, e);
                return;
              }
            }
            ops.next(subscriber, value);
          },
          error => {
            removeAbortCallback();
            if (errorCallback !== null) {
              try { apply(errorCallback, undefined, [error]); } catch (e) { ops.error(subscriber, e); return; }
            }
            ops.error(subscriber, error);
          },
          () => {
            removeAbortCallback();
            if (completeCallback !== null) {
              try { apply(completeCallback, undefined, []); } catch (e) { ops.error(subscriber, e); return; }
            }
            ops.complete(subscriber);
          }), signal);
      });
    }
    catch(callback) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'catch' on 'Observable'");
      callbackArgument(callback, "Failed to execute 'catch' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        const signal = ops.signal(subscriber);
        subscribeTo(this, internalObserver(
          value => ops.next(subscriber, value),
          error => {
            let inner;
            try { inner = toObservable(apply(callback, undefined, [error])); } catch (e) { ops.error(subscriber, e); return; }
            subscribeTo(inner, mirror(subscriber), signal);
          },
          () => ops.complete(subscriber)), signal);
      });
    }
    finally(callback) {
      if (!(#callback in this)) throw new TypeError('Illegal invocation');
      requireArguments(arguments, 1, "Failed to execute 'finally' on 'Observable'");
      callbackArgument(callback, "Failed to execute 'finally' on 'Observable': parameter 1");
      return makeObservable(subscriber => {
        ops.addTeardown(subscriber, callback);
        subscribeTo(this, mirror(subscriber), ops.signal(subscriber));
      });
    }
    toArray(options = undefined) {
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        const signal = signalOption(options);
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        const values = [];
        subscribeTo(this, internalObserver(value => { values.push(value); }, p.reject, () => p.resolve(values)), signal);
        return p.promise;
      });
    }
    forEach(callback, options = undefined) {
      const args = arguments;
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        requireArguments(args, 1, "Failed to execute 'forEach' on 'Observable'");
        callbackArgument(callback, "Failed to execute 'forEach' on 'Observable': parameter 1");
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(options));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let idx = 0;
        subscribeTo(this, internalObserver(value => {
          try { apply(callback, undefined, [value, idx]); } catch (e) {
            p.reject(e);
            apply(controllerAbort, controller, [e]);
          }
          idx++;
        }, p.reject, () => p.resolve(undefined)), signal);
        return p.promise;
      });
    }
    every(predicate, options = undefined) {
      const args = arguments;
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        requireArguments(args, 1, "Failed to execute 'every' on 'Observable'");
        callbackArgument(predicate, "Failed to execute 'every' on 'Observable': parameter 1");
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(options));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let idx = 0;
        subscribeTo(this, internalObserver(value => {
          let passed;
          try { passed = apply(predicate, undefined, [value, idx]); } catch (e) {
            p.reject(e);
            apply(controllerAbort, controller, [e]);
            return;
          }
          idx++;
          if (!passed) { p.resolve(false); apply(controllerAbort, controller, []); }
        }, p.reject, () => p.resolve(true)), signal);
        return p.promise;
      });
    }
    first(options = undefined) {
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(options));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        subscribeTo(this, internalObserver(
          value => { p.resolve(value); apply(controllerAbort, controller, []); },
          p.reject,
          () => p.reject(new RangeError('No values in Observable'))), signal);
        return p.promise;
      });
    }
    last(options = undefined) {
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        const signal = signalOption(options);
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let lastValue, hasLastValue = false;
        subscribeTo(this, internalObserver(
          value => { hasLastValue = true; lastValue = value; },
          p.reject,
          () => { if (hasLastValue) p.resolve(lastValue); else p.reject(new RangeError('No values in Observable')); }), signal);
        return p.promise;
      });
    }
    find(predicate, options = undefined) {
      const args = arguments;
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        requireArguments(args, 1, "Failed to execute 'find' on 'Observable'");
        callbackArgument(predicate, "Failed to execute 'find' on 'Observable': parameter 1");
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(options));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let idx = 0;
        subscribeTo(this, internalObserver(value => {
          let passed;
          try { passed = apply(predicate, undefined, [value, idx]); } catch (e) {
            p.reject(e);
            apply(controllerAbort, controller, [e]);
            return;
          }
          idx++;
          if (passed) { p.resolve(value); apply(controllerAbort, controller, []); }
        }, p.reject, () => p.resolve(undefined)), signal);
        return p.promise;
      });
    }
    some(predicate, options = undefined) {
      const args = arguments;
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        requireArguments(args, 1, "Failed to execute 'some' on 'Observable'");
        callbackArgument(predicate, "Failed to execute 'some' on 'Observable': parameter 1");
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(options));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let idx = 0;
        subscribeTo(this, internalObserver(value => {
          let passed;
          try { passed = apply(predicate, undefined, [value, idx]); } catch (e) {
            p.reject(e);
            apply(controllerAbort, controller, [e]);
            return;
          }
          idx++;
          if (passed) { p.resolve(true); apply(controllerAbort, controller, []); }
        }, p.reject, () => p.resolve(false)), signal);
        return p.promise;
      });
    }
    reduce(reducer, ...rest) {
      const args = arguments;
      return promising(() => {
        if (!(#callback in this)) throw new TypeError('Illegal invocation');
        requireArguments(args, 1, "Failed to execute 'reduce' on 'Observable'");
        callbackArgument(reducer, "Failed to execute 'reduce' on 'Observable': parameter 1");
        const controller = new AC();
        const signal = internalSignal(controller, signalOption(rest[1]));
        const p = abortable(signal);
        if (p.aborted) return p.promise;
        let idx = 0, accumulator = rest[0], initialized = rest.length > 0;
        subscribeTo(this, internalObserver(value => {
          if (!initialized) { accumulator = value; initialized = true; idx++; return; }
          let result;
          try { result = apply(reducer, undefined, [accumulator, value, idx]); } catch (e) {
            p.reject(e);
            apply(controllerAbort, controller, [e]);
            return;
          }
          idx++;
          accumulator = result;
        }, p.reject, () => {
          if (initialized) p.resolve(accumulator);
          else p.reject(new TypeError('Reduce of an empty Observable with no initial value'));
        }), signal);
        return p.promise;
      });
    }
  }
  Object.defineProperty(Observable.prototype, Symbol.toStringTag, { value: 'Observable', configurable: true });

  // `EventTarget.when(type, options)`.
  function when(target, type, options) {
    if (!fullyActive()) return undefined;
    type = `${type}`;
    let capture = false, passive;
    if (options !== undefined && options !== null) {
      if (!isObject(options)) throw new TypeError("The provided value is not of type 'ObservableEventListenerOptions'.");
      capture = !!options.capture;
      if (options.passive !== undefined) passive = !!options.passive;
    }
    return makeObservable(subscriber => {
      const signal = ops.signal(subscriber);
      if (aborted(signal)) return;
      const init = { capture, signal };
      if (passive !== undefined) init.passive = passive;
      apply(addEventListener, target, [type, event => { ops.next(subscriber, event); }, init]);
    });
  }

  return { Observable, Subscriber, when, runAbortAlgorithm };
})
