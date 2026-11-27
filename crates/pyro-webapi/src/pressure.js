// PressureObserver and PressureRecord for one realm (Compute Pressure, https://w3c.github.io/compute-pressure/).
// pressure.rs holds the sources (a virtual one set through CDP, or the host's CPU load) and gives this module:
//   native.active()               the realm's document is fully active
//   native.allowed()              its permissions policy allows `compute-pressure`
//   native.supported(source)      the source can be observed
//   native.sample(source)         its state now, or undefined while it has no reading
//   native.pollMillis(source)     how often observers look at it
//   native.minIntervalMillis(src) the least time between two records of an observer (the platform's own rate)
// Observers look at their sources from one timer per realm while any observes; a record is queued for an observer when
// its source's state changed (or, with a sampleInterval, at every interval), no sooner than its sampleInterval after
// the observer's previous record, and is delivered to the callback in the same task.
(function (native) {
  'use strict';
  const O = Object;
  const PromiseC = Promise;
  const DOMExceptionC = DOMException;
  const TypeErrorC = TypeError;
  const ArrayC = Array;
  const MapC = Map;
  const SetC = Set;
  const WeakMapC = WeakMap;
  const ReflectC = Reflect;
  const setTimeoutC = globalThis.setTimeout;
  const clearTimeoutC = globalThis.clearTimeout;
  const perf = globalThis.performance;
  const nowOf = perf.now;
  const now = () => ReflectC.apply(nowOf, perf, []);
  const isWindow = typeof Window === 'function' && globalThis instanceof Window;

  const SOURCES = O.freeze(['cpu']);
  const MAX_INTERVAL = 4294967295;

  const sourceName = (value, what) => {
    const name = `${value}`;
    if (!SOURCES.includes(name)) {
      throw new TypeErrorC(`Failed to execute '${what}' on 'PressureObserver': The provided value '${name}' is not a valid enum value of type PressureSource.`);
    }
    return name;
  };
  // PressureObserverOptions: [EnforceRange] unsigned long sampleInterval = 0.
  const sampleInterval = (options) => {
    if (options !== undefined && options !== null && typeof options !== 'object' && typeof options !== 'function') {
      throw new TypeErrorC("Failed to execute 'observe' on 'PressureObserver': The provided value is not of type 'PressureObserverOptions'.");
    }
    const raw = options == null ? undefined : options.sampleInterval;
    if (raw === undefined) return 0;
    const n = +raw;
    if (!Number.isFinite(n)) throw new TypeErrorC("Failed to execute 'observe' on 'PressureObserver': Value is not of type 'unsigned long'.");
    const t = Math.trunc(n);
    if (t < 0 || t > MAX_INTERVAL) throw new TypeErrorC("Failed to execute 'observe' on 'PressureObserver': Value is outside the 'unsigned long' value range.");
    return t + 0;
  };

  // ---- PressureRecord
  const recordData = new WeakMapC();
  class PressureRecord {
    constructor() {
      throw new TypeErrorC("Failed to construct 'PressureRecord': Illegal constructor");
    }
    get source() { return recordOf(this, 'source').source; }
    get state() { return recordOf(this, 'state').state; }
    get time() { return recordOf(this, 'time').time; }
    toJSON() {
      const r = recordOf(this, 'toJSON');
      return { source: r.source, state: r.state, time: r.time };
    }
  }
  const recordOf = (self, what) => {
    const r = recordData.get(self);
    if (r === undefined) throw new TypeErrorC(`Failed to execute '${what}' on 'PressureRecord': Illegal invocation`);
    return r;
  };
  const makeRecord = (source, state, time) => {
    const record = O.create(PressureRecord.prototype);
    recordData.set(record, { source, state, time });
    return record;
  };

  // ---- the realm's observers
  // The observers that observe something, and the one timer that looks at their sources.
  const active = new SetC();
  let timer = null;

  // Whether pressure data may be delivered to this realm's observers now (Compute Pressure "can expose pressure
  // data"): a window's document is visible and has focus (a frame: its page does); a worker has no document of its own.
  const mayDeliver = () => {
    if (!isWindow) return true;
    try {
      if (document.visibilityState !== 'visible') return false;
      let focused;
      try { focused = top.document.hasFocus(); } catch (_) { focused = document.hasFocus(); }
      return focused;
    } catch (_) {
      return false;
    }
  };

  const schedule = (delay) => {
    if (timer !== null || active.size === 0) return;
    try {
      timer = setTimeoutC(tick, delay);
    } catch (_) {
      // The document is no longer fully active: nothing will be delivered.
      stopAll();
    }
  };
  const stopAll = () => {
    if (timer !== null) { try { clearTimeoutC(timer); } catch (_) { /* gone with its document */ } timer = null; }
    for (const state of active) state.sources.clear();
    active.clear();
  };

  const tick = () => {
    timer = null;
    if (!native.active()) { stopAll(); return; }
    const t = now();
    const deliverable = mayDeliver();
    const samples = new MapC();
    let delay = 1000;
    const due = [];
    for (const state of active) {
      for (const [source, s] of state.sources) {
        if (!samples.has(source)) samples.set(source, native.sample(source));
        delay = Math.min(delay, native.pollMillis(source));
        const sample = samples.get(source);
        if (sample === undefined || !deliverable) continue;
        if (t - s.lastTime < Math.max(s.interval, native.minIntervalMillis(source))) continue;
        // "Has change in data": a state other than the last record's, or any with a sampleInterval.
        if (s.last === sample && s.interval === 0) continue;
        const record = makeRecord(source, sample, t);
        s.last = sample;
        s.lastTime = t;
        state.queue.push(record);
        if (!due.includes(state)) due.push(state);
      }
    }
    for (const state of due) {
      if (state.queue.length === 0) continue;
      const records = state.queue.splice(0, state.queue.length);
      try {
        ReflectC.apply(state.callback, state.observer, [records, state.observer]);
      } catch (e) {
        // "Report the exception": an uncaught error of its own task.
        setTimeoutC(() => { throw e; }, 0);
      }
    }
    schedule(delay);
  };

  // ---- PressureObserver
  const states = new WeakMapC();
  const stateOf = (self, what) => {
    const s = states.get(self);
    if (s === undefined) throw new TypeErrorC(`Failed to execute '${what}' on 'PressureObserver': Illegal invocation`);
    return s;
  };
  const abortPending = (state, source) => {
    for (const p of state.pending.filter((p) => source === undefined || p.source === source)) {
      p.done = true;
      p.reject(new DOMExceptionC('The observation was aborted.', 'AbortError'));
    }
    state.pending = state.pending.filter((p) => !p.done);
  };
  const stop = (state, source) => {
    state.sources.delete(source);
    state.queue = state.queue.filter((r) => r.source !== source);
    if (state.sources.size === 0) active.delete(state);
  };

  let sourcesArray = null;
  class PressureObserver {
    constructor(callback) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'PressureObserver': 1 argument required, but only 0 present.");
      if (typeof callback !== 'function') throw new TypeErrorC("Failed to construct 'PressureObserver': The callback provided as parameter 1 is not a function.");
      states.set(this, { observer: this, callback, queue: [], sources: new MapC(), pending: [] });
    }
    observe(source, options = {}) {
      let state, name, interval;
      try {
        state = stateOf(this, 'observe');
        name = sourceName(source, 'observe');
        interval = sampleInterval(options);
      } catch (e) {
        return PromiseC.reject(e);
      }
      return new PromiseC((resolve, reject) => {
        const pending = { source: name, resolve, reject, done: false };
        state.pending.push(pending);
        // Settles after the caller's synchronous code, so an unobserve() or disconnect() right after still aborts it.
        PromiseC.resolve().then(() => {
          if (pending.done) return;
          pending.done = true;
          state.pending = state.pending.filter((p) => p !== pending);
          if (!native.active()) return reject(new DOMExceptionC('The document is not fully active.', 'NotSupportedError'));
          if (!native.allowed()) return reject(new DOMExceptionC('Compute Pressure is blocked by the permissions policy.', 'NotAllowedError'));
          if (!native.supported(name)) return reject(new DOMExceptionC(`The ${name} pressure source is not supported on this platform.`, 'NotSupportedError'));
          // Observing again replaces the options and starts a new series of records (the last record is forgotten).
          state.sources.set(name, { interval, last: undefined, lastTime: -Infinity });
          active.add(state);
          schedule(0);
          resolve(undefined);
        });
      });
    }
    unobserve(source) {
      const state = stateOf(this, 'unobserve');
      const name = sourceName(source, 'unobserve');
      abortPending(state, name);
      stop(state, name);
    }
    disconnect() {
      const state = stateOf(this, 'disconnect');
      abortPending(state, undefined);
      for (const name of [...state.sources.keys()]) stop(state, name);
      state.queue = [];
    }
    takeRecords() {
      const state = stateOf(this, 'takeRecords');
      return state.queue.splice(0, state.queue.length);
    }
    static get knownSources() {
      return sourcesArray ??= O.freeze(ArrayC.from(SOURCES));
    }
  }

  const finish = (C, name) => {
    O.defineProperty(C.prototype, Symbol.toStringTag, { value: name, configurable: true });
    for (const k of O.getOwnPropertyNames(C.prototype)) {
      if (k === 'constructor') continue;
      const d = O.getOwnPropertyDescriptor(C.prototype, k);
      d.enumerable = true;
      O.defineProperty(C.prototype, k, d);
    }
    for (const k of O.getOwnPropertyNames(C)) {
      if (k === 'length' || k === 'name' || k === 'prototype') continue;
      const d = O.getOwnPropertyDescriptor(C, k);
      d.enumerable = true;
      O.defineProperty(C, k, d);
    }
  };
  finish(PressureObserver, 'PressureObserver');
  finish(PressureRecord, 'PressureRecord');
  return { PressureObserver, PressureRecord };
});
