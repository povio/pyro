// The AudioWorkletGlobalScope of one BaseAudioContext (audio_worklet.rs evaluates this in the
// context's worklet realm, a bare realm with no window, no DOM and no embedder state), as a function
// of (global, sampleRate, renderQuantumSize, reportException) returning the host object webaudio.js
// drives:
//   setClock(fn)                  the context's current frame (`currentFrame` / `currentTime` between renders)
//   lookup(name)                  the registered processor's normalized parameter descriptors, or undefined
//   construct(name, options, send) -> { ok, handle, deliver, error }
//   process(handle, frame, inputs, outputCounts, names, values) -> { ok, active, outputs, error }
// Nothing here throws into the caller: errors come back as values so webaudio.js can fire
// `processorerror` with the same details a browser reports.
(function (global, sampleRate, renderQuantumSize, reportException) {
  'use strict';
  const definitions = new Map();
  const TypeErrorC = TypeError;
  const F32 = Float32Array;
  const apply = Reflect.apply;
  const defineProperty = Object.defineProperty;
  const freeze = Object.freeze;
  const secret = Symbol('pyro');
  const MOST_POSITIVE = 3.4028234663852886e38;
  const fail = (message) => `Failed to execute 'registerProcessor' on 'AudioWorkletGlobalScope': ${message}`;
  // The global has no DOMException interface of its own here: an Error with its name.
  function DOMExceptionLike(message, name) { const e = new Error(message); e.name = name; return e; }
  const isConstructor = (f) => {
    if (typeof f !== 'function') return false;
    try { Reflect.construct(String, [], f); return true; } catch { return false; }
  };

  // The context's current frame, between renders; while a processor runs, the frame it runs at.
  let clock = () => 0;
  let frameOverride = null;
  const currentFrame = () => (frameOverride !== null ? frameOverride : clock());

  // ---- MessagePort / MessageEvent of the global ------------------------------------------------
  // The processor's end of its node's port: messages cross to and from the node's own MessagePort
  // through `send` (webaudio.js posts them on the entangled port, where they are structured-cloned
  // and delivered as tasks) and `deliver`.
  class MessageEvent {
    constructor(type, init = {}) {
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'MessageEvent': 1 argument required, but only 0 present.");
      defineProperty(this, 'type', { value: `${type}`, enumerable: true });
      defineProperty(this, 'data', { value: init === null || init === undefined ? null : init.data ?? null, enumerable: true });
      defineProperty(this, 'origin', { value: init?.origin === undefined ? '' : `${init.origin}`, enumerable: true });
      defineProperty(this, 'lastEventId', { value: init?.lastEventId === undefined ? '' : `${init.lastEventId}`, enumerable: true });
      defineProperty(this, 'source', { value: null, enumerable: true });
      defineProperty(this, 'ports', { value: freeze([]), enumerable: true });
      defineProperty(this, 'target', { value: null, writable: true, enumerable: true });
      defineProperty(this, 'currentTarget', { value: null, writable: true, enumerable: true });
      defineProperty(this, 'isTrusted', { value: false, enumerable: true });
    }
  }
  defineProperty(MessageEvent.prototype, Symbol.toStringTag, { value: 'MessageEvent', configurable: true });

  const portStates = new WeakMap();
  const portOf = (self) => {
    const state = portStates.get(self);
    if (!state) throw new TypeErrorC('Illegal invocation');
    return state;
  };
  function runListeners(self, state, event) {
    event.target = self;
    event.currentTarget = self;
    for (const entry of state.listeners.slice()) {
      if (entry.type !== event.type || entry.removed) continue;
      if (entry.once) { entry.removed = true; state.listeners.splice(state.listeners.indexOf(entry), 1); }
      try {
        const callback = entry.callback;
        if (typeof callback === 'function') apply(callback, self, [event]);
        else if (callback !== null && typeof callback === 'object' && typeof callback.handleEvent === 'function') apply(callback.handleEvent, callback, [event]);
      } catch (error) {
        // An exception of a listener is reported in this global; it does not stop the processor.
        reportException(error);
      }
    }
    event.currentTarget = null;
  }
  function handlerAttribute(C, type) {
    defineProperty(C.prototype, 'on' + type, {
      get() { return portOf(this).handlers[type]?.value ?? null; },
      set(value) {
        const state = portOf(this);
        const callable = typeof value === 'function' || (value !== null && typeof value === 'object');
        let slot = state.handlers[type];
        if (!callable) {
          if (slot) slot.entry.callback = null;
          if (slot) slot.value = null;
          return;
        }
        if (!slot) {
          const entry = { type, callback: null, once: false, removed: false };
          slot = state.handlers[type] = { value: null, entry };
          state.listeners.push(entry);
        }
        slot.value = value;
        slot.entry.callback = typeof value === 'function' ? value : null;
        // Setting onmessage starts the port.
        if (type === 'message') startPort(this, state);
      },
      enumerable: true,
      configurable: true,
    });
  }
  function startPort(self, state) {
    if (state.started || state.closed) return;
    state.started = true;
    const queued = state.queue.splice(0);
    for (const event of queued) runListeners(self, state, event);
  }
  class MessagePort {
    constructor(token, send, close) {
      if (token !== secret) throw new TypeErrorC("Failed to construct 'MessagePort': Illegal constructor");
      portStates.set(this, { send, close, listeners: [], handlers: {}, queue: [], started: false, closed: false });
    }
    postMessage(message, transfer) {
      const state = portOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'postMessage' on 'MessagePort': 1 argument required, but only 0 present.");
      if (state.closed) return;
      if (transfer === undefined) state.send(message);
      else state.send(message, transfer);
    }
    start() { startPort(this, portOf(this)); }
    close() {
      const state = portOf(this);
      if (state.closed) return;
      state.closed = true;
      state.queue.length = 0;
      state.close();
    }
    addEventListener(type, callback, options) {
      const state = portOf(this);
      if (arguments.length < 2) throw new TypeErrorC("Failed to execute 'addEventListener' on 'EventTarget': 2 arguments required, but only " + arguments.length + ' present.');
      if (callback === null || callback === undefined) return;
      type = `${type}`;
      const once = options !== null && typeof options === 'object' && !!options.once;
      if (state.listeners.some((e) => e.type === type && e.callback === callback && !e.removed)) return;
      state.listeners.push({ type, callback, once, removed: false });
    }
    removeEventListener(type, callback) {
      const state = portOf(this);
      type = `${type}`;
      const at = state.listeners.findIndex((e) => e.type === type && e.callback === callback && !e.removed);
      if (at >= 0) { state.listeners[at].removed = true; state.listeners.splice(at, 1); }
    }
    dispatchEvent(event) {
      const state = portOf(this);
      if (event === null || typeof event !== 'object') throw new TypeErrorC("Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.");
      runListeners(this, state, event);
      return true;
    }
  }
  handlerAttribute(MessagePort, 'message');
  handlerAttribute(MessagePort, 'messageerror');
  for (const C of [MessagePort, MessageEvent]) {
    for (const name of Object.getOwnPropertyNames(C.prototype)) {
      if (name === 'constructor') continue;
      const desc = Object.getOwnPropertyDescriptor(C.prototype, name);
      desc.enumerable = true;
      defineProperty(C.prototype, name, desc);
    }
  }
  defineProperty(MessagePort.prototype, Symbol.toStringTag, { value: 'MessagePort', configurable: true });
  function deliverTo(port, data) {
    const state = portStates.get(port);
    if (state.closed) return;
    const event = new MessageEvent('message', { data });
    if (state.started) runListeners(port, state, event);
    else state.queue.push(event);
  }

  // ---- registerProcessor ---------------------------------------------------------------------------
  const float = (value, what) => {
    const n = +value;
    if (n !== n || n === Infinity || n === -Infinity) throw new TypeErrorC(fail(`The provided float value is non-finite for '${what}'.`));
    return Math.fround(n);
  };
  // AudioParamDescriptor dictionary -> { name, defaultValue, minValue, maxValue, automationRate }.
  function descriptor(value) {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) {
      throw new TypeErrorC(fail("The 'parameterDescriptors' entry is not of type 'AudioParamDescriptor'."));
    }
    const rate = value.automationRate === undefined ? 'a-rate' : `${value.automationRate}`;
    if (rate !== 'a-rate' && rate !== 'k-rate') throw new TypeErrorC(fail(`The provided value '${rate}' is not a valid enum value of type AutomationRate.`));
    const defaultValue = value.defaultValue === undefined ? 0 : float(value.defaultValue, 'defaultValue');
    const maxValue = value.maxValue === undefined ? Math.fround(MOST_POSITIVE) : float(value.maxValue, 'maxValue');
    const minValue = value.minValue === undefined ? -Math.fround(MOST_POSITIVE) : float(value.minValue, 'minValue');
    if (value.name === undefined) throw new TypeErrorC(fail("Required member 'name' of AudioParamDescriptor is undefined."));
    const name = `${value.name}`;
    return { name, defaultValue, minValue, maxValue, automationRate: rate };
  }
  defineProperty(global, 'registerProcessor', {
    value: function registerProcessor(name, processorCtor) {
      if (arguments.length < 2) throw new TypeErrorC(fail(`2 arguments required, but only ${arguments.length} present.`));
      name = String(name);
      if (name === '') throw DOMExceptionLike(fail('The processor name cannot be empty.'), 'NotSupportedError');
      if (definitions.has(name)) throw DOMExceptionLike(fail(`A class with name:'${name}' is already registered.`), 'NotSupportedError');
      if (typeof processorCtor !== 'function') throw new TypeErrorC(fail("parameter 2 is not of type 'Function'."));
      if (!isConstructor(processorCtor)) throw new TypeErrorC(fail('The provided class definition is not a constructor.'));
      const prototype = processorCtor.prototype;
      if (prototype === null || (typeof prototype !== 'object' && typeof prototype !== 'function')) {
        throw new TypeErrorC(fail("The 'prototype' object on the class does not exist."));
      }
      const descriptors = processorCtor.parameterDescriptors;
      const parameters = [];
      if (descriptors !== undefined) {
        if (descriptors === null || typeof descriptors[Symbol.iterator] !== 'function') {
          throw new TypeErrorC(fail("The 'parameterDescriptors' property is not iterable."));
        }
        const seen = new Set();
        for (const entry of descriptors) {
          const d = descriptor(entry);
          if (seen.has(d.name)) throw DOMExceptionLike(fail(`Duplicate parameter name '${d.name}'.`), 'NotSupportedError');
          if (!(d.minValue <= d.defaultValue && d.defaultValue <= d.maxValue)) {
            throw DOMExceptionLike(fail(`The defaultValue of '${d.name}' is outside [minValue, maxValue].`), 'InvalidStateError');
          }
          seen.add(d.name);
          parameters.push(d);
        }
      }
      definitions.set(name, { processorCtor, parameters });
    },
    writable: true, configurable: true,
  });

  // ---- AudioWorkletProcessor -----------------------------------------------------------------------
  // The processor being constructed for a node (set while the node's constructor runs): the first
  // `new AudioWorkletProcessor()` / `super()` takes it; a second one, or one outside a node's
  // construction, is an Illegal constructor.
  let pending = null;
  const processorPorts = new WeakMap();
  class AudioWorkletProcessor {
    constructor() {
      if (pending === null) throw new TypeErrorC("Failed to construct 'AudioWorkletProcessor': Illegal constructor");
      processorPorts.set(this, pending.port);
      pending = null;
    }
    get port() {
      const port = processorPorts.get(this);
      if (port === undefined) throw new TypeErrorC('Illegal invocation');
      return port;
    }
  }
  defineProperty(AudioWorkletProcessor.prototype, 'port', { enumerable: true });
  defineProperty(AudioWorkletProcessor.prototype, Symbol.toStringTag, { value: 'AudioWorkletProcessor', configurable: true });
  defineProperty(global, 'AudioWorkletProcessor', { value: AudioWorkletProcessor, writable: true, configurable: true });
  defineProperty(global, 'MessagePort', { value: MessagePort, writable: true, configurable: true });
  defineProperty(global, 'MessageEvent', { value: MessageEvent, writable: true, configurable: true });
  const getter = (name, get) => defineProperty(global, name, { get, configurable: true, enumerable: true });
  getter('sampleRate', () => sampleRate);
  getter('currentFrame', () => currentFrame());
  getter('currentTime', () => currentFrame() / sampleRate);
  getter('renderQuantumSize', () => renderQuantumSize);

  // ---- The host webaudio.js drives -------------------------------------------------------------------
  return {
    setClock(fn) { clock = fn; },
    lookup(name) {
      const definition = definitions.get(`${name}`);
      return definition === undefined ? undefined : definition.parameters;
    },
    // Constructs the processor of a node: `options` is the node's serialized AudioWorkletNodeOptions
    // (a clone made by the caller), `send(data[, transfer])` posts a message to the node's port.
    construct(name, options, send, close) {
      const definition = definitions.get(`${name}`);
      const port = new MessagePort(secret, send, close);
      const deliver = (data) => deliverTo(port, data);
      const copy = {};
      for (const key of Object.keys(options)) defineProperty(copy, key, { value: options[key], writable: true, enumerable: true, configurable: true });
      pending = { port };
      try {
        const processor = Reflect.construct(definition.processorCtor, [copy]);
        return { ok: true, handle: { processor }, deliver, error: undefined };
      } catch (error) {
        return { ok: false, handle: undefined, deliver, error };
      } finally {
        pending = null;
      }
    },
    // One render quantum of a processor: `inputs` are arrays of channels (arrays of Float32Arrays of
    // the caller's realm, empty for an input with nothing active), `outputCounts` the channel count
    // of each output, `names` / `values` the parameter arrays (length 1 when constant over the quantum).
    process(handle, frame, inputs, outputCounts, names, values) {
      const previous = frameOverride;
      frameOverride = frame;
      try {
        const processor = handle.processor;
        const inputArrays = [];
        for (let i = 0; i < inputs.length; i++) {
          const channels = [];
          for (let c = 0; c < inputs[i].length; c++) channels.push(new F32(inputs[i][c]));
          inputArrays.push(freeze(channels));
        }
        const outputArrays = [];
        for (let i = 0; i < outputCounts.length; i++) {
          const channels = [];
          for (let c = 0; c < outputCounts[i]; c++) channels.push(new F32(renderQuantumSize));
          outputArrays.push(freeze(channels));
        }
        const parameters = {};
        for (let i = 0; i < names.length; i++) {
          defineProperty(parameters, names[i], { value: new F32(values[i]), writable: true, enumerable: true, configurable: true });
        }
        // `process` is read for every call, as a getter or a property assigned after construction.
        const callback = processor.process;
        if (typeof callback !== 'function') throw new TypeErrorC("The 'process' property of the AudioWorkletProcessor is not callable.");
        const result = apply(callback, processor, [freeze(inputArrays), freeze(outputArrays), parameters]);
        return { ok: true, active: !!result, outputs: outputArrays, error: undefined };
      } catch (error) {
        return { ok: false, active: false, outputs: undefined, error };
      } finally {
        frameOverride = previous;
      }
    },
  };
})
