// PerformanceObserver and PerformanceObserverEntryList for one realm, made
// on its first use (Performance Timeline). `performance` is the realm's
// Performance object (a Window's or a worker's); `supported` its
// supportedEntryTypes; `native`:
//   watch(queue | null): queue(type, entry?) runs as entries are added to
//     the realm's timeline: a mark or measure with its entry object, or the
//     type ('paint', 'navigation') whose entries just became available.
//     Registered while some observer observes, so a realm without one pays
//     nothing per mark.
//     Entries of the types in `perf/` (layout-shift, event, ...) arrive as
//     queue(type, entry, inSoftNavigation) from the native timeline.
//   observing(types): (comma-joined) the entry types the realm's observers observe now.
//   buffered(type, includeSoftNavigation): the native buffer of a type.
//   dropped(type): entries of a native type the full buffer dropped.
//   reportException(error): HTML "report an exception" (a callback threw).
//   droppedResources(): resource entries the buffer dropped so far.
//   ambient(), setAmbient(id): the task attribution of the code running (Soft Navigations); optional.
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
  // Types whose entries live in the page's `perf/` timeline buffers; the
  // others are read through performance.getEntriesByType.
  const BASE = ['mark', 'measure', 'navigation', 'paint', 'resource'];
  const isNative = (t) => !BASE.includes(t);
  const DEFAULT_THRESHOLD = 104;
  const tag = (C, name) => defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
  // Web IDL operations: enumerable prototype methods whose length counts the
  // required arguments only.
  const operations = (C, lengths) => {
    const p = C.prototype;
    for (const k of O.getOwnPropertyNames(p)) {
      if (k === 'constructor') continue;
      defineProperty(p, k, { enumerable: true });
      if (k in lengths) defineProperty(p[k], 'length', { value: lengths[k] });
    }
  };
  const illegal = (name) => new TypeErrorC(`Failed to construct '${name}': Illegal constructor`);
  const observeError = (rest) => new TypeErrorC(`Failed to execute 'observe' on 'PerformanceObserver': ${rest}`);

  // Sorted by startTime; equal times keep the order they were queued in.
  const sorted = (entries) => entries.map((e, i) => [e, i]).sort((a, b) => (a[0].startTime - b[0].startTime) || (a[1] - b[1])).map((p) => p[0]);

  const listData = new WeakMap();
  // A list's entries; a TypeError for any other `this`.
  const entriesOf = (list) => {
    const entries = listData.get(list);
    if (entries === undefined) throw new TypeErrorC('Illegal invocation');
    return entries;
  };
  class PerformanceObserverEntryList {
    constructor(t, entries) {
      if (t !== token) throw illegal('PerformanceObserverEntryList');
      listData.set(this, sorted(entries));
    }
    getEntries() {
      return entriesOf(this).slice();
    }
    getEntriesByType(type) {
      const entries = entriesOf(this);
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'getEntriesByType' on 'PerformanceObserverEntryList': 1 argument required, but only 0 present.");
      }
      type = String(type);
      return entries.filter((e) => e.entryType === type);
    }
    getEntriesByName(name, type) {
      const entries = entriesOf(this);
      if (arguments.length === 0) {
        throw new TypeErrorC("Failed to execute 'getEntriesByName' on 'PerformanceObserverEntryList': 1 argument required, but only 0 present.");
      }
      name = String(name);
      const t = type === undefined ? undefined : String(type);
      return entries.filter((e) => e.name === name && (t === undefined || e.entryType === t));
    }
  }
  tag(PerformanceObserverEntryList, 'PerformanceObserverEntryList');
  operations(PerformanceObserverEntryList, { getEntriesByName: 1 });
  // No constructor in its IDL: length 0.
  defineProperty(PerformanceObserverEntryList, 'length', { value: 0 });

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
        let dropped = 0;
        for (const t of d.types) dropped += t === 'resource' ? native.droppedResources() : isNative(t) ? native.dropped(t) : 0;
        options.droppedEntriesCount = dropped;
        d.requiresDropped = false;
      }
      // The callback descends from the code that began observing, when that was attributed.
      const outer = d.attribution !== 0 ? native.ambient() : 0;
      if (d.attribution !== 0) native.setAmbient(d.attribution);
      try {
        d.callback.call(observer, list, observer, options);
      } catch (e) {
        native.reportException(e);
      }
      if (d.attribution !== 0) native.setAmbient(outer);
    }
  };
  const schedule = () => {
    if (!scheduled) {
      scheduled = true;
      setTimeoutC(deliver, 0);
    }
  };
  // Event Timing: an observer's durationThreshold (default 104, at least 16,
  // a multiple of 8) filters `event` entries.
  const passes = (d, type, entry) => type !== 'event' || entry.duration >= d.threshold;
  const queue = (type, entry, soft) => {
    let entries = null;
    for (const observer of registered) {
      const d = data.get(observer);
      if (!d.types.has(type)) continue;
      if (soft === true && !d.soft) continue;
      if (entries === null) entries = entry !== undefined ? [entry] : getEntriesByType.call(performance, type);
      let queued = false;
      for (const e of entries) {
        if (!passes(d, type, e)) continue;
        d.buffer.push(e);
        queued = true;
      }
      if (queued) schedule();
    }
  };
  const update = () => {
    native.watch(registered.size ? queue : null);
    const types = new Set();
    for (const observer of registered) for (const t of data.get(observer).types) types.add(t);
    native.observing(ArrayFrom(types).join(','));
  };

  class PerformanceObserver {
    constructor(callback) {
      if (new.target === undefined) {
        throw new TypeErrorC("Failed to construct 'PerformanceObserver': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
      }
      if (typeof callback !== 'function') {
        throw new TypeErrorC("Failed to construct 'PerformanceObserver': parameter 1 is not of type 'Function'.");
      }
      data.set(this, { callback, mode: null, types: new Set(), buffer: [], requiresDropped: false, threshold: DEFAULT_THRESHOLD, soft: false, attribution: 0 });
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
      if (native.ambient !== undefined) {
        const attribution = native.ambient();
        if (attribution !== 0) d.attribution = attribution;
      }
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
        if (t === 'event' && options.durationThreshold !== undefined) {
          // Event Timing: at least 16, rounded to a multiple of 8.
          const threshold = Number(options.durationThreshold);
          d.threshold = Math.max(16, Math.round((Number.isNaN(threshold) ? 0 : threshold) / 8) * 8);
        }
        if (options.includeSoftNavigationObservations !== undefined) d.soft = Boolean(options.includeSoftNavigationObservations);
        if (options.buffered) {
          const entries = isNative(t) ? native.buffered(t, d.soft) : getEntriesByType.call(performance, t);
          let queued = false;
          for (const e of entries) {
            if (!passes(d, t, e)) continue;
            d.buffer.push(e);
            queued = true;
          }
          if (queued) schedule();
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
      d.threshold = DEFAULT_THRESHOLD;
      d.soft = false;
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
  // observe(optional PerformanceObserverInit options = {}).
  operations(PerformanceObserver, { observe: 0 });

  // A worker global holds both names as lazy accessors on its prototype. The
  // first read of either settles the other too (own data properties, as
  // WebIDL interface objects), once per realm and only while the global has
  // no own property of that name: a value the page defined or a deletion
  // made before is kept.
  const hasOwn = O.hasOwn;
  let settledOther = false;
  const otherToSettle = (global, name) => {
    if (settledOther) return undefined;
    settledOther = true;
    const other = name === 'PerformanceObserver' ? 'PerformanceObserverEntryList' : 'PerformanceObserver';
    return hasOwn(global, other) ? undefined : other;
  };

  return { PerformanceObserver, PerformanceObserverEntryList, otherToSettle };
})
