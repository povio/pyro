// CSS View Transitions 1 and 2 (`document.startViewTransition`, element-scoped
// `Element.startViewTransition`): per-scope lifecycle and native capture.
(function (native) {
  'use strict';
  const {captureValid, transitionAnimations} = native;
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
  const pending = new Map();
  let pumpQueued = false;

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
    if (st.phase !== 'animating' || !st.animationDone) return;
    if (st.runs) { st.runs.release(); st.runs = null; }
    if (st.admitted) { captureValid(st.root, false, true, st.scope); st.admitted = false; }
    if (st.waiting) return;
    st.phase = 'done';
    if (active.get(st.scope) === st.self) active.delete(st.scope);
    st.finished.resolve();
  }
  function capture(st) {
    if (captureValid(st.root, st.phase === 'pending-capture', false, st.scope)) return true;
    skip(st, new DOMException('Cannot capture view transition state', 'InvalidStateError'));
    return false;
  }

  // Web IDL accessor and operation functions: `get <name>` getters, and
  // Promise-typed attributes that reject rather than throw on a bad `this`.
  const getter = (name, read) => Object.getOwnPropertyDescriptor({ get [name]() { return read(this); } }, name).get;
  const accessor = (get) => ({ get, enumerable: true, configurable: true });
  const method = (value) => ({ value, writable: true, enumerable: true, configurable: true });
  const requireArgument = (count, interfaceName, name) => {
    if (!count) throw new TypeError(`Failed to execute '${name}' on '${interfaceName}': 1 argument required, but only 0 present.`);
  };

  // `ViewTransitionTypeSet` (setlike<DOMString>): an interface object whose
  // instances keep their members in a backing Set.
  const typeSets = new WeakMap();
  const SetCtor = Set;
  const setProto = Set.prototype;
  const setAdd = setProto.add, setDelete = setProto.delete, setHas = setProto.has, setClear = setProto.clear;
  const setValues = setProto.values, setEntries = setProto.entries, setForEach = setProto.forEach;
  const setSize = Object.getOwnPropertyDescriptor(setProto, 'size').get;
  const ViewTransitionTypeSet = function ViewTransitionTypeSet() {
    throw new TypeError('Illegal constructor');
  };
  const members = (self, name) => {
    const set = typeSets.get(self);
    if (set === undefined) throw new TypeError(`Illegal invocation: ViewTransitionTypeSet.${name}`);
    return set;
  };
  const typeSetMethods = {
    entries() { return setEntries.call(members(this, 'entries')); },
    keys() { return setValues.call(members(this, 'keys')); },
    values() { return setValues.call(members(this, 'values')); },
    forEach(callback) {
      const set = members(this, 'forEach');
      requireArgument(arguments.length, 'ViewTransitionTypeSet', 'forEach');
      if (!isFunction(callback)) throw new TypeError("Failed to execute 'forEach' on 'ViewTransitionTypeSet': The callback provided as parameter 1 is not a function.");
      const thisArg = arguments[1], self = this;
      setForEach.call(set, (value) => { callback.call(thisArg, value, value, self); });
    },
    has(value) {
      const set = members(this, 'has');
      requireArgument(arguments.length, 'ViewTransitionTypeSet', 'has');
      return setHas.call(set, String(value));
    },
    add(value) {
      const set = members(this, 'add');
      requireArgument(arguments.length, 'ViewTransitionTypeSet', 'add');
      setAdd.call(set, String(value));
      return this;
    },
    delete(value) {
      const set = members(this, 'delete');
      requireArgument(arguments.length, 'ViewTransitionTypeSet', 'delete');
      return setDelete.call(set, String(value));
    },
    clear() { setClear.call(members(this, 'clear')); },
  };
  Object.defineProperties(ViewTransitionTypeSet.prototype, {
    entries: method(typeSetMethods.entries),
    keys: method(typeSetMethods.keys),
    values: method(typeSetMethods.values),
    forEach: method(typeSetMethods.forEach),
    has: method(typeSetMethods.has),
    add: method(typeSetMethods.add),
    delete: method(typeSetMethods.delete),
    clear: method(typeSetMethods.clear),
    size: accessor(getter('size', (self) => setSize.call(members(self, 'size')))),
    [Symbol.iterator]: { value: typeSetMethods.values, writable: true, configurable: true },
    [Symbol.toStringTag]: { value: 'ViewTransitionTypeSet', configurable: true },
  });
  Object.defineProperty(ViewTransitionTypeSet, 'prototype', { writable: false });
  function typeSet(types) {
    const self = Object.create(ViewTransitionTypeSet.prototype);
    typeSets.set(self, new SetCtor(types));
    return self;
  }

  const ViewTransition = function ViewTransition() {
    throw new TypeError('Illegal constructor');
  };
  const state = (self, name) => {
    const st = states.get(self);
    if (st === undefined) throw new TypeError(`Illegal invocation: ViewTransition.${name}`);
    return st;
  };
  const promiseGetter = (name) => getter(name, (self) => {
    const st = states.get(self);
    return st === undefined ? PromiseCtor.reject(new TypeError(`Illegal invocation: ViewTransition.${name}`)) : st[name].promise;
  });
  Object.defineProperties(ViewTransition.prototype, {
    updateCallbackDone: accessor(promiseGetter('updateCallbackDone')),
    ready: accessor(promiseGetter('ready')),
    finished: accessor(promiseGetter('finished')),
    types: accessor(getter('types', (self) => state(self, 'types').types)),
    transitionRoot: accessor(getter('transitionRoot', (self) => state(self, 'transitionRoot').root)),
    waitUntil: method({
      waitUntil(promise) {
        const st = state(this, 'waitUntil');
        requireArgument(arguments.length, 'ViewTransition', 'waitUntil');
        if (st.phase === 'done') return;
        if (st.waiting >= 1024) throw new DOMException('Too many pending promises', 'QuotaExceededError');
        ++st.waiting;
        const settled = () => { --st.waiting; finish(st); };
        promiseThen.call(PromiseCtor.resolve(promise), settled, settled);
      },
    }.waitUntil),
    skipTransition: method({
      skipTransition() {
        const st = state(this, 'skipTransition');
        if (st.phase !== 'done') skip(st, abortError('Transition was skipped'));
      },
    }.skipTransition),
    [Symbol.toStringTag]: { value: 'ViewTransition', configurable: true },
  });
  Object.defineProperty(ViewTransition, 'prototype', { writable: false });

  // "Skip the view transition": `ready` rejects with `reason` unless it has
  // settled, the update callback still runs, `finished` follows it.
  function skip(st, reason) {
    if (st.phase === 'done') return;
    const before = st.phase === 'pending-capture';
    if (st.runs) { st.runs.release(); st.runs = null; }
    if (before) pending.delete(st.scope);
    if (st.admitted) { captureValid(st.root, false, true, st.scope); st.admitted = false; }
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
      types: typeSet(types), timer: null, waiting: 0, animationDone: false,
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
    st.admitted = captureValid(root, false, false, scope, true);
    if (!st.admitted) {
      skip(st, new DOMException('View transition capture limit reached', 'InvalidStateError'));
      return self;
    }
    pending.set(scope, st);
    if (!pumpQueued) {
      pumpQueued = true;
      later(() => {
        const batch = Array.from(pending.values());
        pending.clear();
        pumpQueued = false;
        for (const next of batch) {
          if (next.phase !== 'pending-capture' || !capture(next)) continue;
          next.phase = 'update-callback-called';
          callUpdate(next);
        }
      });
    }
    return self;
  }

  return { start, ViewTransition, ViewTransitionTypeSet, active: scope => active.get(scope) || null };
})
