// PerformanceObserver and PerformanceObserverEntryList for one realm, made
// on its first use (Performance Timeline). `performance` is the realm's
// Performance object (a Window's or a worker's); `supported` its
// supportedEntryTypes; `native`:
//   watch(queue | null): queue(type, entry?) runs as entries are added to
//     the realm's timeline: a mark or measure with its entry object, or the
//     type ('paint', 'navigation') whose entries just became available.
//     Registered while some observer observes, so a realm without one pays
//     nothing per mark.
//   reportException(error): HTML "report an exception" (a callback threw).
//   droppedResources(): resource entries the buffer dropped so far.
// A worker's timeline has no `paint` or `navigation` entries.
// Callbacks run in a task after the entries were queued, as in Chrome.
(function (performance, native, supported) {
  'use strict';
  const token = Symbol('pyro');
  const O = Object;
  const freeze = O.freeze;
  const defineProperty = O.defineProperty;
  const TypeErrorC = TypeError;
  const DOMExceptionC = DOMException;
  const ArrayFrom = Array.from;
  const setTimeoutC = setTimeout;
  const getEntriesByType = performance.getEntriesByType;
  const SUPPORTED = freeze(supported);
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const observeError = (rest) => new TypeErrorC(`Failed to execute 'observe' on 'PerformanceObserver': ${rest}`);

  // Sorted by startTime; equal times keep the order they were queued in.
  const sorted = (entries) => entries.map((e, i) => [e, i]).sort((a, b) => (a[0].startTime - b[0].startTime) || (a[1] - b[1])).map((p) => p[0]);

  const listData = new WeakMap();
  class PerformanceObserverEntryList {
    constructor(t, entries) {
      if (t !== token) throw illegal('PerformanceObserverEntryList');
      listData.set(this, sorted(entries));
    }
    getEntries() {
      return listData.get(this).slice();
    }
    getEntriesByType(type) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'getEntriesByType' on 'PerformanceObserverEntryList': 1 argument required, but only 0 present.");
      }
      type = String(type);
      return listData.get(this).filter((e) => e.entryType === type);
    }
    getEntriesByName(name, type) {
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'getEntriesByName' on 'PerformanceObserverEntryList': 1 argument required, but only 0 present.");
      }
      name = String(name);
      const t = type === undefined ? undefined : String(type);
      return listData.get(this).filter((e) => e.name === name && (t === undefined || e.entryType === t));
    }
  }
  tag(PerformanceObserverEntryList, 'PerformanceObserverEntryList');

  // Observers with at least one observed type, in registration order.
  const registered = new Set();
  const data = new WeakMap();
  let scheduled = false;

  const deliver = () => {
    scheduled = false;
    for (const observer of ArrayFrom(registered)) {
      const d = data.get(observer);
      if (d.buffer.length === 0) continue;
      const list = new PerformanceObserverEntryList(token, d.buffer);
      d.buffer = [];
      const options = {};
      if (d.requiresDropped) {
        // Only the resource timing buffer drops entries.
        options.droppedEntriesCount = d.types.has('resource') ? native.droppedResources() : 0;
        d.requiresDropped = false;
      }
      try {
        d.callback.call(observer, list, observer, options);
      } catch (e) {
        native.reportException(e);
      }
    }
  };
  const schedule = () => {
    if (!scheduled) {
      scheduled = true;
      setTimeoutC(deliver, 0);
    }
  };
  const queue = (type, entry) => {
    let entries = null;
    for (const observer of registered) {
      const d = data.get(observer);
      if (!d.types.has(type)) continue;
      if (entries === null) entries = entry !== undefined ? [entry] : getEntriesByType.call(performance, type);
      for (const e of entries) d.buffer.push(e);
      if (entries.length) schedule();
    }
  };
  const update = () => native.watch(registered.size ? queue : null);

  class PerformanceObserver {
    constructor(callback) {
      if (new.target === undefined) {
        throw new TypeErrorC("Failed to construct 'PerformanceObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      }
      if (typeof callback !== 'function') {
        throw new TypeErrorC("Failed to construct 'PerformanceObserver': parameter 1 is not of type 'Function'.");
      }
      data.set(this, { callback, mode: null, types: new Set(), buffer: [], requiresDropped: false });
    }
    static get supportedEntryTypes() {
      return SUPPORTED;
    }
    observe(options) {
      const d = data.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      if (options === undefined || options === null) options = {};
      else if (typeof options !== 'object' && typeof options !== 'function') {
        throw observeError("The provided value is not of type 'PerformanceObserverInit'.");
      }
      const entryTypes = options.entryTypes;
      const type = options.type;
      if (entryTypes === undefined && type === undefined) {
        throw observeError("An observe() call must include either entryTypes or type arguments.");
      }
      if (entryTypes !== undefined && type !== undefined) {
        throw observeError("An observe() call must not include both entryTypes and type arguments.");
      }
      const mode = entryTypes !== undefined ? 'multiple' : 'single';
      if (d.mode !== null && d.mode !== mode) {
        throw new DOMExceptionC(
          mode === 'multiple'
            ? "Failed to execute 'observe' on 'PerformanceObserver': This PerformanceObserver has performed observe({type:...}, therefore it cannot perform observe({entryTypes:...})"
            : "Failed to execute 'observe' on 'PerformanceObserver': This observer has performed observe({entryTypes:...}, therefore it cannot perform observe({type:...})",
          'InvalidModificationError');
      }
      d.requiresDropped = true;
      if (mode === 'multiple') {
        if (entryTypes === null || typeof entryTypes !== 'object' || typeof entryTypes[Symbol.iterator] !== 'function') {
          throw observeError("Failed to read the 'entryTypes' property from 'PerformanceObserverInit': The provided value cannot be converted to a sequence.");
        }
        const types = ArrayFrom(entryTypes, String).filter((t) => SUPPORTED.includes(t));
        if (types.length === 0) return;
        d.mode = mode;
        d.types = new Set(types);
      } else {
        const t = String(type);
        if (!SUPPORTED.includes(t)) return;
        d.mode = mode;
        d.types.add(t);
        if (options.buffered) {
          const entries = getEntriesByType.call(performance, t);
          for (const e of entries) d.buffer.push(e);
          if (entries.length) schedule();
        }
      }
      registered.add(this);
      update();
    }
    disconnect() {
      const d = data.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      registered.delete(this);
      d.buffer = [];
      d.types = new Set();
      d.mode = null;
      update();
    }
    takeRecords() {
      const d = data.get(this);
      if (d === undefined) throw new TypeErrorC('Illegal invocation');
      const records = d.buffer;
      d.buffer = [];
      return records;
    }
  }
  tag(PerformanceObserver, 'PerformanceObserver');

  return { PerformanceObserver, PerformanceObserverEntryList };
})
