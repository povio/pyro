// CSS View Transitions 1 and 2 (`document.startViewTransition`, element-scoped
// `Element.startViewTransition`): the lifecycle only, made on a realm's first
// call (view_transition.rs). Nothing is captured or painted: the update
// callback runs at the next animation frame, as after the old state's capture,
// and the transition finishes at the frame after the update, as one whose
// animations take no time. `ready`, `updateCallbackDone` and `finished` settle
// as the specification says for that, and a new transition on the same scope
// (or `skipTransition()`) skips the running one.
(function (captureValid) {
  'use strict';
  const global = globalThis;
  const rafFn = global.requestAnimationFrame, setTimeoutFn = global.setTimeout;
  const clearTimeoutFn = global.clearTimeout;
  const raf = (fn) => rafFn.call(global, fn);
  const setTimeout_ = (fn, ms) => setTimeoutFn.call(global, fn, ms);
  const PromiseCtor = Promise;
  const promiseThen = Promise.prototype.then;
  const isFunction = (v) => typeof v === 'function';
  const states = new WeakMap();
  // The running transition of each scope (a document or an element).
  const active = new WeakMap();

  function deferred() {
    let resolve, reject;
    const promise = new PromiseCtor((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }
  const handled = (promise) => { promiseThen.call(promise, undefined, () => {}); };
  const later = (fn) => {
    // The scope's frame: a realm without frames (a destroyed window's) still
    // settles its transitions.
    try { raf(fn); } catch (_) { setTimeout_(fn, 0); }
  };
  const abortError = (message) => new DOMException(message, 'AbortError');
  function finish(st) {
    if (st.phase !== 'animating' || !st.animationDone || st.waiting) return;
    st.phase = 'done';
    if (active.get(st.scope) === st.self) active.delete(st.scope);
    st.finished.resolve();
  }
  function capture(st) {
    if (captureValid(st.root)) return true;
    skip(st, new DOMException('Cannot capture view transition state', 'InvalidStateError'));
    return false;
  }

  const ViewTransition = function ViewTransition() {
    throw new TypeError('Illegal constructor');
  };
  const state = (self, name) => {
    const st = states.get(self);
    if (st === undefined) throw new TypeError(`Illegal invocation: ViewTransition.${name}`);
    return st;
  };
  Object.defineProperties(ViewTransition.prototype, {
    updateCallbackDone: { get() { return state(this, 'updateCallbackDone').updateCallbackDone.promise; }, enumerable: true, configurable: true },
    ready: { get() { return state(this, 'ready').ready.promise; }, enumerable: true, configurable: true },
    finished: { get() { return state(this, 'finished').finished.promise; }, enumerable: true, configurable: true },
    types: { get() { return state(this, 'types').types; }, enumerable: true, configurable: true },
    transitionRoot: { get() { return state(this, 'transitionRoot').root; }, enumerable: true, configurable: true },
    waitUntil: {
      value: function waitUntil(promise) {
        const st = state(this, 'waitUntil');
        if (!arguments.length) throw new TypeError('waitUntil requires a promise');
        if (st.phase === 'done') return;
        if (st.waiting >= 1024) throw new DOMException('Too many pending promises', 'QuotaExceededError');
        ++st.waiting;
        const settled = () => { --st.waiting; finish(st); };
        promiseThen.call(PromiseCtor.resolve(promise), settled, settled);
      }, writable: true, enumerable: true, configurable: true,
    },
    skipTransition: {
      value: function skipTransition() {
        const st = state(this, 'skipTransition');
        if (st.phase !== 'done') skip(st, abortError('Transition was skipped'));
      },
      writable: true, enumerable: true, configurable: true,
    },
    [Symbol.toStringTag]: { value: 'ViewTransition', configurable: true },
  });

  // "Skip the view transition": `ready` rejects with `reason` unless it has
  // settled, the update callback still runs, `finished` follows it.
  function skip(st, reason) {
    if (st.phase === 'done') return;
    const before = st.phase === 'pending-capture';
    st.phase = 'done';
    if (st.timer !== null) { clearTimeoutFn.call(global, st.timer); st.timer = null; }
    if (active.get(st.scope) === st.self) active.delete(st.scope);
    if (!st.readySettled) { st.readySettled = true; st.ready.reject(reason); }
    if (before) setTimeout_(() => callUpdate(st), 0);
    promiseThen.call(st.updateCallbackDone.promise, () => st.finished.resolve(), (e) => st.finished.reject(e));
  }

  function callUpdate(st) {
    if (st.called) return;
    st.called = true;
    if (st.phase !== 'done') st.timer = setTimeout_(() => skip(st, new DOMException('Update callback timed out', 'TimeoutError')), 4000);
    let result;
    try {
      result = isFunction(st.update) ? PromiseCtor.resolve(st.update.call(undefined)) : PromiseCtor.resolve();
    } catch (e) {
      result = PromiseCtor.reject(e);
    }
    promiseThen.call(result, () => {
      if (st.timer !== null) { clearTimeoutFn.call(global, st.timer); st.timer = null; }
      st.updateCallbackDone.resolve();
      if (st.phase === 'done' || !capture(st)) return;
      st.phase = 'animating';
      st.readySettled = true;
      st.ready.resolve();
      later(() => {
        st.animationDone = true;
        finish(st);
      });
    }, (e) => {
      st.updateCallbackDone.reject(e);
      if (st.phase !== 'done') skip(st, e);
    });
  }

  function start(scope, root, arg, hidden) {
    let update = null;
    let types = [];
    if (isFunction(arg)) {
      update = arg;
    } else if (arg !== undefined && arg !== null) {
      if (typeof arg !== 'object') throw new TypeError("Failed to execute 'startViewTransition': The provided value is not of type '(StartViewTransitionOptions or ViewTransitionUpdateCallback)'.");
      const u = arg.update;
      if (u !== undefined && u !== null) {
        if (!isFunction(u)) throw new TypeError("Failed to execute 'startViewTransition': The 'update' property is not a function.");
        update = u;
      }
      const t = arg.types;
      if (t !== undefined && t !== null) types = Array.from(t, String);
    }
    const self = Object.create(ViewTransition.prototype);
    const st = {
      self, scope, root, update, phase: 'pending-capture', called: false, readySettled: false,
      types: new Set(types), timer: null, waiting: 0, animationDone: false,
      updateCallbackDone: deferred(), ready: deferred(), finished: deferred(),
    };
    handled(st.ready.promise);
    states.set(self, st);
    const prior = active.get(scope);
    if (prior !== undefined) skip(states.get(prior), abortError('Old view transition aborted by new view transition.'));
    active.set(scope, self);
    if (hidden) {
      skip(st, new DOMException('Document is hidden', 'InvalidStateError'));
      return self;
    }
    later(() => {
      if (st.phase !== 'pending-capture' || !capture(st)) return;
      st.phase = 'update-callback-called';
      callUpdate(st);
    });
    return self;
  }

  return { start, ViewTransition, active: scope => active.get(scope) || null };
})
