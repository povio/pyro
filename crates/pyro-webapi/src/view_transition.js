// CSS View Transitions 1 and 2 (`document.startViewTransition`, element-scoped
// `Element.startViewTransition`): per-scope lifecycle and native capture.
(function (native) {
  'use strict';
  const {captureValid, transitionAnimations} = native;
  const {DOMException, intrinsics: p} = native;
  const {apply, global, Promise: PromiseCtor, WeakMap, Map, TypeError} = p;
  const raf = fn => apply(p.raf, global, [fn]);
  const setTimeout_ = (fn, ms) => apply(p.timer, global, [fn, ms]);
  const cancel = id => apply(p.cancel, global, [id]);
  const resolve = value => apply(p.promiseResolve, PromiseCtor, [value]);
  const then = (promise, ok, fail) => apply(p.promiseThen, promise, [ok, fail]);
  const get = (map, key) => apply(p.get, map, [key]);
  const set = (map, key, value) => apply(p.set, map, [key, value]);
  const remove = (map, key) => apply(p.remove, map, [key]);
  const isFunction = (v) => typeof v === 'function';
  const states = new WeakMap();
  // The running transition of each scope (a document or an element).
  const active = new WeakMap();
  let pending = new Map();
  const skipped = new WeakMap();
  let pumpQueued = false;

  function deferred() {
    let resolve, reject;
    const promise = new PromiseCtor((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }
  const handled = (promise) => { then(promise, undefined, () => {}); };
  const later = (fn) => {
    // The scope's frame: a realm without frames (a destroyed window's) still
    // settles its transitions.
    try { raf(fn); } catch (_) { setTimeout_(fn, 0); }
  };
  const abortError = (message) => new DOMException(message, 'AbortError');
  function finish(st) {
    if (st.phase !== 'animating' || !st.animationDone) return;
    if (st.runs) { st.runs.release(); st.runs = null; }
    if (st.admitted) { captureValid(st.root, false, true, st.scope); st.admitted = false; }
    if (st.waiting) return;
    st.phase = 'done';
    if (get(active, st.scope) === st.self) remove(active, st.scope);
    st.finished.resolve();
  }
  function capture(st) {
    if (captureValid(st.root, st.phase === 'pending-capture', false, st.scope)) return true;
    skip(st, new DOMException('Cannot capture view transition state', 'InvalidStateError'));
    return false;
  }

  const ViewTransition = function ViewTransition() {
    throw new TypeError('Illegal constructor');
  };
  const state = (self, name) => {
    const st = get(states, self);
    if (st === undefined) throw new TypeError(`Illegal invocation: ViewTransition.${name}`);
    return st;
  };
  p.defineProperties(ViewTransition.prototype, {
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
        then(resolve(promise), settled, settled);
      }, writable: true, enumerable: true, configurable: true,
    },
    skipTransition: {
      value: function skipTransition() {
        const st = state(this, 'skipTransition');
        if (st.phase !== 'done') skip(st, abortError('Transition was skipped'));
      },
      writable: true, enumerable: true, configurable: true,
    },
    [p.tag]: { value: 'ViewTransition', configurable: true },
  });

  // "Skip the view transition": `ready` rejects with `reason` unless it has
  // settled, the update callback still runs, `finished` follows it.
  function skip(st, reason) {
    if (st.phase === 'done') return;
    const before = st.phase === 'pending-capture';
    if (st.runs) { st.runs.release(); st.runs = null; }
    if (before) apply(p.mapDelete, pending, [st.scope]);
    if (st.admitted) { captureValid(st.root, false, true, st.scope); st.admitted = false; }
    st.phase = 'done';
    if (st.timer !== null) { cancel(st.timer); st.timer = null; }
    if (get(active, st.scope) === st.self) remove(active, st.scope);
    if (!st.readySettled) { st.readySettled = true; st.ready.reject(reason); }
    if (before) {
      set(skipped, st.scope, (get(skipped, st.scope) || 0) + 1);
      setTimeout_(() => {
        try { callUpdate(st); } finally {
          const left = get(skipped, st.scope) - 1;
          if (left) set(skipped, st.scope, left); else remove(skipped, st.scope);
        }
      }, 0);
    }
    then(st.updateCallbackDone.promise, () => st.finished.resolve(), (e) => st.finished.reject(e));
  }

  function callUpdate(st) {
    if (st.called) return;
    st.called = true;
    if (st.phase !== 'done') st.timer = setTimeout_(() => skip(st, new DOMException('Update callback timed out', 'TimeoutError')), 4000);
    let result;
    try {
      result = resolve(isFunction(st.update) ? apply(st.update, undefined, []) : undefined);
    } catch (e) {
      result = apply(p.promiseReject, PromiseCtor, [e]);
    }
    then(result, () => {
      if (st.timer !== null) { cancel(st.timer); st.timer = null; }
      st.updateCallbackDone.resolve();
      if (st.phase === 'done' || !capture(st)) return;
      try { st.runs = transitionAnimations(st.root); } catch (_) { st.runs = null; }
      if (!st.runs) { skip(st, new DOMException('Cannot animate transition boxes', 'InvalidStateError')); return; }
      if (!native.sample(st.root, st.runs.sample())) { skip(st, abortError('Cannot sample transition boxes')); return; }
      st.phase = 'animating';
      st.readySettled = true;
      st.ready.resolve();
      let frames = 0;
      later(function checkAnimations() {
        if (st.phase !== 'animating') return;
        const entries = transitionAnimations(st.root, true);
        if (entries === null || ++frames > 4096) { skip(st, abortError('Transition animation limit reached')); return; }
        st.runs.sync(entries);
        if (!native.sample(st.root, st.runs.sample())) { skip(st, abortError('Cannot sample transition boxes')); return; }
        if (st.runs.active()) { later(checkAnimations); return; }
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
    const types = new p.Set();
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
      if (t !== undefined && t !== null) for (const type of t) apply(p.add, types, [p.String(type)]);
    }
    const self = p.create(ViewTransition.prototype);
    const st = {
      self, scope, root, update, phase: 'pending-capture', called: false, readySettled: false,
      types, timer: null, waiting: 0, animationDone: false,
      updateCallbackDone: deferred(), ready: deferred(), finished: deferred(),
    };
    handled(st.ready.promise);
    set(states, self, st);
    const prior = get(active, scope);
    if (prior !== undefined) skip(get(states, prior), abortError('Old view transition aborted by new view transition.'));
    set(active, scope, self);
    if (hidden) {
      skip(st, new DOMException('Document is hidden', 'InvalidStateError'));
      return self;
    }
    st.admitted = captureValid(root, false, false, scope, true);
    if (!st.admitted) {
      skip(st, new DOMException('View transition capture limit reached', 'InvalidStateError'));
      return self;
    }
    enqueue(st);
    return self;
  }

  function enqueue(st) {
    apply(p.mapSet, pending, [st.scope, st]);
    if (!pumpQueued) {
      pumpQueued = true;
      later(() => {
        const batch = pending;
        pending = new Map();
        pumpQueued = false;
        apply(p.mapEach, batch, [next => {
          if (next.phase !== 'pending-capture') return;
          if (get(skipped, next.scope)) { enqueue(next); return; }
          if (!capture(next)) return;
          next.phase = 'update-callback-called';
          callUpdate(next);
        }]);
      });
    }
  }

  return { start, ViewTransition, active: scope => get(active, scope) || null };
})
