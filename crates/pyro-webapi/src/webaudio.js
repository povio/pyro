// Web Audio for one realm, made on first use (webaudio.rs). The script
// evaluates to a function of the engine's natives (decode, activated, a
// timer that does not keep the page busy)
// returning the module object.
//
// The graph renders in 128-frame quanta, pulled from the destination: each
// node's output for a quantum is computed once (fan-out shares it), inputs
// are summed after up/down-mixing to the input's computed channel count, and
// AudioParams compute their automation (and connected inputs) per sample
// (a-rate) or per quantum (k-rate). An OfflineAudioContext renders when
// startRendering() runs; a real-time AudioContext has no output device, so
// it only runs its clock and ends its sources on time.
(natives => {
  'use strict';
  const { decode, activated, timer, clearTimer: clearClock } = natives;
  const apply = Reflect.apply;
  const ET = EventTarget;
  const dispatch = ET.prototype.dispatchEvent;
  const addListener = ET.prototype.addEventListener;
  const removeListener = ET.prototype.removeEventListener;
  const Ev = Event;
  const DE = DOMException;
  const TypeErrorC = TypeError;
  const RangeErrorC = RangeError;
  const PromiseC = Promise;
  const WeakRefC = WeakRef;
  const F32 = Float32Array;
  const ArrayBufferC = ArrayBuffer;
  const detachedOf = (Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'detached') || {}).get;
  const transferOf = ArrayBuffer.prototype.transfer;
  const structuredCloneC = typeof structuredClone === 'function' ? structuredClone : null;
  const defineProperty = Object.defineProperty;
  const getOwnPropertyNames = Object.getOwnPropertyNames;
  const getOwnPropertyDescriptor = Object.getOwnPropertyDescriptor;
  const queueMicrotaskC = queueMicrotask;
  const reportErrorC = typeof reportError === 'function' ? reportError : () => {};
  const { sin, cos, asin, acos, fround, exp, pow, floor, ceil, min, max, PI, abs } = Math;
  // Captured now: page fake timers installed later do not stall audio.
  const setTimer = setTimeout;
  const perf = performance;
  const perfNow = perf.now;
  const now = () => apply(perfNow, perf, []);
  const secret = Symbol('pyro');
  const QUANTUM = 128;
  const MAX_CHANNELS = 32;
  const MIN_RATE = 3000, MAX_RATE = 768000;

  function illegal() { return new TypeErrorC('Illegal constructor'); }
  function invocation() { return new TypeErrorC('Illegal invocation'); }
  function webidl(C, tag) {
    for (const name of getOwnPropertyNames(C.prototype)) {
      if (name === 'constructor') continue;
      const desc = getOwnPropertyDescriptor(C.prototype, name);
      desc.enumerable = true;
      defineProperty(C.prototype, name, desc);
    }
    defineProperty(C.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }
  function fail(what, iface, message) { return `Failed to ${what} '${iface}': ${message}`; }
  function finite(v, what, iface, kind = 'float') {
    const n = +v;
    if (n !== n || n === Infinity || n === -Infinity) {
      throw new TypeErrorC(fail(what, iface, `The provided ${kind} value is non-finite.`));
    }
    return n;
  }
  function ulong(v) { return (+v) >>> 0; }
  // An event handler IDL attribute on the prototype of C.
  function handler(C, type) {
    const key = Symbol(type);
    defineProperty(C.prototype, 'on' + type, {
      get() { return this[key] ? this[key].value : null; },
      set(value) {
        if (this[key]) apply(removeListener, this, [type, this[key].listener]);
        this[key] = null;
        if (typeof value === 'function' || (value !== null && typeof value === 'object')) {
          const listener = function (event) {
            return typeof value === 'function' ? apply(value, this, [event]) : undefined;
          };
          this[key] = { value, listener };
          apply(addListener, this, [type, listener]);
        }
      },
      enumerable: true,
      configurable: true,
    });
  }
  function fire(target, type) { apply(dispatch, target, [new Ev(type)]); }

  // ---- AudioBuffer ----------------------------------------------------------
  const buffers = new WeakMap();
  let made = null;
  function bufOf(self) {
    const b = buffers.get(self);
    if (!b) throw invocation();
    return b;
  }
  function checkBuffer(what, iface, channels, length, rate) {
    if (channels < 1 || channels > MAX_CHANNELS) {
      throw new DE(fail(what, iface, `The number of channels provided (${channels}) is outside the range [1, ${MAX_CHANNELS}].`), 'NotSupportedError');
    }
    if (length < 1) {
      throw new DE(fail(what, iface, `The number of frames provided (${length}) is less than or equal to the minimum bound (0).`), 'NotSupportedError');
    }
    if (!(rate >= MIN_RATE && rate <= MAX_RATE)) {
      throw new DE(fail(what, iface, `The sample rate provided (${rate}) is outside the range [${MIN_RATE}, ${MAX_RATE}].`), 'NotSupportedError');
    }
  }
  class AudioBuffer {
    constructor(options) {
      if (options === secret && made) {
        buffers.set(this, made);
        made = null;
        return;
      }
      if (arguments.length < 1) throw new TypeErrorC("Failed to construct 'AudioBuffer': 1 argument required, but only 0 present.");
      if (options === null || typeof options !== 'object') {
        throw new TypeErrorC("Failed to construct 'AudioBuffer': The provided value is not of type 'AudioBufferOptions'.");
      }
      if (options.length === undefined) throw new TypeErrorC("Failed to construct 'AudioBuffer': Failed to read the 'length' property from 'AudioBufferOptions': Required member is undefined.");
      if (options.sampleRate === undefined) throw new TypeErrorC("Failed to construct 'AudioBuffer': Failed to read the 'sampleRate' property from 'AudioBufferOptions': Required member is undefined.");
      const length = ulong(options.length);
      const channels = options.numberOfChannels === undefined ? 1 : ulong(options.numberOfChannels);
      const rate = finite(options.sampleRate, 'construct', 'AudioBuffer');
      checkBuffer('construct', 'AudioBuffer', channels, length, rate);
      const data = [];
      for (let i = 0; i < channels; i++) data.push(new F32(length));
      buffers.set(this, { rate, length, data });
    }
    get sampleRate() { return bufOf(this).rate; }
    get length() { return bufOf(this).length; }
    get duration() { const b = bufOf(this); return b.length / b.rate; }
    get numberOfChannels() { return bufOf(this).data.length; }
    getChannelData(channel) {
      const b = bufOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'getChannelData' on 'AudioBuffer': 1 argument required, but only 0 present.");
      const c = ulong(channel);
      if (c >= b.data.length) {
        throw new DE(fail('execute', 'getChannelData', `channel index (${c}) exceeds number of channels (${b.data.length})`).replace("'getChannelData'", "'getChannelData' on 'AudioBuffer'"), 'IndexSizeError');
      }
      return b.data[c];
    }
    copyFromChannel(destination, channelNumber, bufferOffset = 0) {
      const b = bufOf(this);
      if (!(destination instanceof F32)) throw new TypeErrorC("Failed to execute 'copyFromChannel' on 'AudioBuffer': parameter 1 is not of type 'Float32Array'.");
      const c = ulong(channelNumber);
      if (c >= b.data.length) {
        throw new DE(`Failed to execute 'copyFromChannel' on 'AudioBuffer': The channelNumber provided (${c}) is outside the range [0, ${b.data.length - 1}].`, 'IndexSizeError');
      }
      const offset = ulong(bufferOffset);
      const src = b.data[c];
      if (offset < src.length) destination.set(src.subarray(offset, offset + min(destination.length, src.length - offset)));
    }
    copyToChannel(source, channelNumber, bufferOffset = 0) {
      const b = bufOf(this);
      if (!(source instanceof F32)) throw new TypeErrorC("Failed to execute 'copyToChannel' on 'AudioBuffer': parameter 1 is not of type 'Float32Array'.");
      const c = ulong(channelNumber);
      if (c >= b.data.length) {
        throw new DE(`Failed to execute 'copyToChannel' on 'AudioBuffer': The channelNumber provided (${c}) is outside the range [0, ${b.data.length - 1}].`, 'IndexSizeError');
      }
      const offset = ulong(bufferOffset);
      const dst = b.data[c];
      if (offset < dst.length) dst.set(source.subarray(0, min(source.length, dst.length - offset)), offset);
    }
  }
  webidl(AudioBuffer, 'AudioBuffer');
  function makeBuffer(rate, data) {
    made = { rate, length: data[0].length, data };
    return new AudioBuffer(secret);
  }

  // ---- AudioParam -----------------------------------------------------------
  const SET = 0, LINEAR = 1, EXPONENTIAL = 2, TARGET = 3, CURVE = 4;
  const params = new WeakMap();
  function paramOf(self) {
    const p = params.get(self);
    if (!p) throw invocation();
    return p;
  }
  const F32_MAX = 3.4028234663852886e38;
  class AudioParam {
    constructor(token, ctx, value, minValue = -F32_MAX, maxValue = F32_MAX, rate = 'a-rate', fixedRate = false) {
      if (token !== secret) throw illegal();
      params.set(this, {
        ctx, value: Math.fround(value), defaultValue: Math.fround(value), minValue, maxValue, rate, fixedRate,
        events: [], inputs: [], values: new F32(QUANTUM), q: -1,
      });
    }
    get value() {
      const p = paramOf(this);
      return p.events.length ? Math.fround(clampParam(p, valueAt(p, ctxOf(p.ctx).time()))) : p.value;
    }
    set value(v) {
      const p = paramOf(this);
      const value = finite(v, 'set the \'value\' property on', 'AudioParam');
      setParamValue(p, value, ctxOf(p.ctx).time());
    }
    get defaultValue() { return paramOf(this).defaultValue; }
    get minValue() { return paramOf(this).minValue; }
    get maxValue() { return paramOf(this).maxValue; }
    get automationRate() { return paramOf(this).rate; }
    set automationRate(v) {
      const p = paramOf(this);
      const rate = `${v}`;
      if (rate !== 'a-rate' && rate !== 'k-rate') return;
      if (p.fixedRate && rate !== p.rate) {
        throw new DE("Failed to set the 'automationRate' property on 'AudioParam': automationRate cannot be changed from \"" + p.rate + '" to "' + rate + '"', 'InvalidStateError');
      }
      p.rate = rate;
    }
    setValueAtTime(value, startTime) {
      const p = paramOf(this);
      const v = finite(value, 'execute', "setValueAtTime' on 'AudioParam");
      const t = time(startTime, 'setValueAtTime');
      insertEvent(p, { type: SET, time: t, value: v }, 'setValueAtTime');
      return this;
    }
    linearRampToValueAtTime(value, endTime) {
      const p = paramOf(this);
      const v = finite(value, 'execute', "linearRampToValueAtTime' on 'AudioParam");
      const t = time(endTime, 'linearRampToValueAtTime');
      insertEvent(p, ramp(p, { type: LINEAR, time: t, value: v }), 'linearRampToValueAtTime');
      return this;
    }
    exponentialRampToValueAtTime(value, endTime) {
      const p = paramOf(this);
      const v = finite(value, 'execute', "exponentialRampToValueAtTime' on 'AudioParam");
      if (abs(v) < 1.401298464324817e-45) {
        throw new RangeErrorC(`Failed to execute 'exponentialRampToValueAtTime' on 'AudioParam': The float target value provided (${v}) should not be in the range (-1.40130e-45, 1.40130e-45).`);
      }
      const t = time(endTime, 'exponentialRampToValueAtTime');
      insertEvent(p, ramp(p, { type: EXPONENTIAL, time: t, value: v }), 'exponentialRampToValueAtTime');
      return this;
    }
    setTargetAtTime(target, startTime, timeConstant) {
      const p = paramOf(this);
      const v = finite(target, 'execute', "setTargetAtTime' on 'AudioParam");
      const t = time(startTime, 'setTargetAtTime');
      const tc = finite(timeConstant, 'execute', "setTargetAtTime' on 'AudioParam");
      if (tc < 0) {
        throw new RangeErrorC(`Failed to execute 'setTargetAtTime' on 'AudioParam': Time constant must be a finite non-negative number: ${tc}`);
      }
      insertEvent(p, tc === 0 ? { type: SET, time: t, value: v } : { type: TARGET, time: t, value: v, tc }, 'setTargetAtTime');
      return this;
    }
    setValueCurveAtTime(values, startTime, duration) {
      const p = paramOf(this);
      if (values === null || typeof values !== 'object' || typeof values[Symbol.iterator] !== 'function') {
        throw new TypeErrorC("Failed to execute 'setValueCurveAtTime' on 'AudioParam': The provided value cannot be converted to a sequence.");
      }
      const curve = new F32([...values].map((v) => finite(v, 'execute', "setValueCurveAtTime' on 'AudioParam")));
      const t = time(startTime, 'setValueCurveAtTime');
      const d = finite(duration, 'execute', "setValueCurveAtTime' on 'AudioParam", 'double');
      if (d <= 0) throw new RangeErrorC(`Failed to execute 'setValueCurveAtTime' on 'AudioParam': The curve duration provided (${d}) is less than or equal to the minimum bound (0).`);
      if (curve.length < 2) {
        throw new DE(`Failed to execute 'setValueCurveAtTime' on 'AudioParam': The curve length provided (${curve.length}) is less than the minimum bound (2).`, 'InvalidStateError');
      }
      insertEvent(p, { type: CURVE, time: t, value: curve[curve.length - 1], curve, duration: d }, 'setValueCurveAtTime');
      return this;
    }
    cancelScheduledValues(cancelTime) {
      const p = paramOf(this);
      const t = time(cancelTime, 'cancelScheduledValues');
      p.events = p.events.filter((e) => e.time < t);
      return this;
    }
    cancelAndHoldAtTime(cancelTime) {
      const p = paramOf(this);
      const t = time(cancelTime, 'cancelAndHoldAtTime');
      const held = valueAt(p, t);
      const kept = p.events.filter((e) => e.time < t && !(e.type === CURVE && e.time + e.duration > t));
      const next = p.events.find((e) => e.time >= t);
      // A ramp under way ends where it is held.
      if (next && (next.type === LINEAR || next.type === EXPONENTIAL)) kept.push({ type: next.type, time: t, value: held, t0: next.t0, v0: next.v0 });
      else kept.push({ type: SET, time: t, value: held });
      p.events = kept;
      return this;
    }
  }
  webidl(AudioParam, 'AudioParam');

  function checkValueAtTime(p, t) {
    if (p.events.some(e => e.type === CURVE && e.time <= t && t < e.time + e.duration)) {
      throw new DE('The current time overlaps an automation curve.', 'NotSupportedError');
    }
  }
  function setParamValue(p, value, t) {
    // Setting value is equivalent to inserting a setValueAtTime at currentTime.
    checkValueAtTime(p, t);
    const at = p.events.findIndex(e => e.type === SET && e.time === t);
    if (at >= 0) p.events[at].value = value;
    else if (p.events.length) insertEvent(p, {type: SET, time: t, value});
    p.value = Math.fround(value);
  }
  function nodeParam(n, ...options) {
    const param = new AudioParam(secret, n.ctx, ...options);
    (n.audioParams ??= []).push(params.get(param));
    return param;
  }
  function time(v, method) {
    const t = finite(v, 'execute', `${method}' on 'AudioParam`, 'double');
    if (t < 0) throw new RangeErrorC(`Failed to execute '${method}' on 'AudioParam': Time must be a finite non-negative number: ${t}`);
    return t;
  }
  // A ramp first in the list starts at the time and value it was made at.
  function ramp(p, e) {
    e.t0 = ctxOf(p.ctx).time();
    e.v0 = p.value;
    return e;
  }
  function insertEvent(p, e, method) {
    const events = p.events;
    for (const o of events) {
      const oEnd = o.type === CURVE ? o.time + o.duration : o.time;
      const eEnd = e.type === CURVE ? e.time + e.duration : e.time;
      if ((o.type === CURVE && e.time > o.time && e.time < oEnd) || (e.type === CURVE && o.time > e.time && o.time < eEnd)
        || (o.type === CURVE && e.type === CURVE && o.time === e.time)) {
        throw new DE(`Failed to execute '${method}' on 'AudioParam': Event overlaps another event.`, 'NotSupportedError');
      }
    }
    let i = events.length;
    while (i > 0 && events[i - 1].time > e.time) i--;
    events.splice(i, 0, e);
    for (const o of events) o.start = undefined;
  }
  function clampParam(p, v) { return v < p.minValue ? p.minValue : v > p.maxValue ? p.maxValue : v; }
  // The value just before event i starts (memoized until the list changes).
  function startValue(p, i) {
    const e = p.events[i];
    if (e.start === undefined) e.start = i === 0 ? p.value : endValue(p, i - 1, e.time);
    return e.start;
  }
  // Event i's value at time t >= its time (held after it ends).
  function endValue(p, i, t) {
    const e = p.events[i];
    switch (e.type) {
      case TARGET: return e.value + (startValue(p, i) - e.value) * exp(-(t - e.time) / e.tc);
      case CURVE: return t >= e.time + e.duration ? e.value : curveAt(e, t);
      default: return e.value;
    }
  }
  function curveAt(e, t) {
    const n = e.curve.length;
    const k = (n - 1) * (t - e.time) / e.duration;
    const i = floor(k);
    if (i >= n - 1) return e.curve[n - 1];
    return e.curve[i] + (e.curve[i + 1] - e.curve[i]) * (k - i);
  }
  function valueAt(p, t) {
    const events = p.events;
    if (!events.length) return p.value;
    let lo = 0, hi = events.length;
    while (lo < hi) { const mid = (lo + hi) >> 1; if (events[mid].time <= t) lo = mid + 1; else hi = mid; }
    const i = lo - 1;
    const next = events[i + 1];
    if (next && (next.type === LINEAR || next.type === EXPONENTIAL)) {
      let t0, v0;
      if (i >= 0) {
        const prev = events[i];
        t0 = prev.type === CURVE ? prev.time + prev.duration : prev.time;
        v0 = endValue(p, i, max(t0, prev.time));
      } else {
        t0 = next.t0 || 0;
        v0 = next.v0 === undefined ? p.value : next.v0;
      }
      const t1 = next.time, v1 = next.value;
      if (t <= t0) return v0;
      if (t1 <= t0) return v1;
      const f = (t - t0) / (t1 - t0);
      if (next.type === LINEAR) return v0 + (v1 - v0) * f;
      if (v0 === 0 || v0 * v1 < 0) return v0;
      return v0 * pow(v1 / v0, f);
    }
    if (i < 0) return p.value;
    return endValue(p, i, t);
  }
  // The param's per-sample values for the quantum at frame f0 (with
  // connected inputs summed in), or its k-rate value repeated.
  function paramValues(p, f0, rate, q) {
    if (p.q === q) return p.values;
    p.q = q;
    const out = p.values;
    if (!p.events.length) out.fill(p.value);
    else if (p.rate === 'k-rate') out.fill(valueAt(p, f0 / rate));
    else for (let i = 0; i < QUANTUM; i++) out[i] = valueAt(p, (f0 + i) / rate);
    if (p.inputs.length) {
      const mono = new F32(QUANTUM);
      for (const { node, output } of p.inputs) mix(pull(node, q, f0, output), [mono], 'speakers', 1);
      if (p.rate === 'k-rate') { const v = mono[0]; for (let i = 0; i < QUANTUM; i++) out[i] += v; }
      else for (let i = 0; i < QUANTUM; i++) out[i] += mono[i];
    }
    for (let i = 0; i < QUANTUM; i++) {
      const v = out[i];
      if (v !== v) out[i] = p.defaultValue;
      else if (v < p.minValue) out[i] = p.minValue;
      else if (v > p.maxValue) out[i] = p.maxValue;
    }
    return out;
  }

  // ---- Mixing -----------------------------------------------------------
  // Adds bus `src` (array of channels) into `dst`, up/down-mixed to its
  // channel count (`interp` 'speakers' uses the speaker rules for 1, 2, 4
  // and 6 channels; otherwise channels map by index).
  function mix(src, dst, interp) {
    const si = src.length, di = dst.length;
    const add = (d, s, g = 1) => { for (let i = 0; i < QUANTUM; i++) d[i] += s[i] * g; };
    if (si === di || interp === 'discrete' || !speakerMix(src, dst, add)) {
      for (let c = 0; c < min(si, di); c++) add(dst[c], src[c]);
    }
  }
  function speakerMix(src, dst, add) {
    const si = src.length, di = dst.length;
    const s = src, d = dst;
    const H = Math.SQRT1_2;
    if (si === 1 && di === 2) { add(d[0], s[0]); add(d[1], s[0]); return true; }
    if (si === 1 && di === 4) { add(d[0], s[0]); add(d[1], s[0]); return true; }
    if (si === 1 && di === 6) { add(d[2], s[0]); return true; }
    if (si === 2 && di === 1) { add(d[0], s[0], 0.5); add(d[0], s[1], 0.5); return true; }
    if (si === 2 && di === 4) { add(d[0], s[0]); add(d[1], s[1]); return true; }
    if (si === 2 && di === 6) { add(d[0], s[0]); add(d[1], s[1]); return true; }
    if (si === 4 && di === 1) { for (let c = 0; c < 4; c++) add(d[0], s[c], 0.25); return true; }
    if (si === 4 && di === 2) { add(d[0], s[0], 0.5); add(d[0], s[2], 0.5); add(d[1], s[1], 0.5); add(d[1], s[3], 0.5); return true; }
    if (si === 4 && di === 6) { add(d[0], s[0]); add(d[1], s[1]); add(d[4], s[2]); add(d[5], s[3]); return true; }
    if (si === 6 && di === 1) {
      add(d[0], s[0], H); add(d[0], s[1], H); add(d[0], s[2]); add(d[0], s[4], 0.5); add(d[0], s[5], 0.5); return true;
    }
    if (si === 6 && di === 2) {
      add(d[0], s[0]); add(d[0], s[2], H); add(d[0], s[4], H);
      add(d[1], s[1]); add(d[1], s[2], H); add(d[1], s[5], H); return true;
    }
    if (si === 6 && di === 4) {
      add(d[0], s[0]); add(d[0], s[2], H); add(d[1], s[1]); add(d[1], s[2], H);
      add(d[2], s[4]); add(d[3], s[5]); return true;
    }
    return false;
  }
  const silentBus = () => [new F32(QUANTUM)];

  // ---- AudioNode ------------------------------------------------------------
  const nodes = new WeakMap();
  function nodeOf(self) {
    const n = nodes.get(self);
    if (!n) throw invocation();
    return n;
  }
  function contextArg(context, iface) {
    if (!contexts.has(context)) {
      throw new TypeErrorC(`Failed to construct '${iface}': parameter 1 is not of type 'BaseAudioContext'.`);
    }
    return context;
  }
  // The output of `node` (state) for quantum q at frame f0.
  function pull(n, q, f0, output = 0) {
    const bus = renderNode(n, q, f0);
    // A splitter exposes each discrete input channel as its own mono output port.
    return n.splitChannels ? (bus[output] ? [bus[output]] : silentBus()) : bus;
  }
  function renderNode(n, q, f0) {
    if (n.q === q) return n.out;
    if (n.busy) return silentBus();
    n.busy = true;
    try {
      const track = ctxOf(n.ctx).trackConvolverActivity;
      if (track) n.activeInput = false;
      n.out = n.process(f0, q);
      if (track) {
        // A scheduled source can be active even when it outputs zero. A
        // processor may also be emitting its own tail after its inputs end.
        n.active = n.activeInput || n.processingFrame === f0 || n.out.some(ch => ch.some(v => v !== 0));
      }
    } finally {
      n.busy = false;
    }
    n.q = q;
    return n.out;
  }
  // Input 0 of `n` for the quantum, mixed to its computed channel count;
  // null without connections.
  function input(n, q, f0, port = 0) {
    const sources = n.numberOfInputs > 1 ? n.inputs.filter(s => s.input === port) : n.inputs;
    if (!sources.length) return null;
    const buses = sources.map((s) => pull(s.node, q, f0, s.output));
    if (ctxOf(n.ctx).trackConvolverActivity && sources.some(s => s.node.q === q && s.node.active)) n.activeInput = true;
    let channels = 0;
    for (const b of buses) channels = max(channels, b.length);
    if (n.mode === 'clamped-max') channels = min(channels, n.channelCount);
    else if (n.mode === 'explicit') channels = n.channelCount;
    const out = [];
    for (let c = 0; c < channels; c++) out.push(new F32(QUANTUM));
    for (const b of buses) mix(b, out, n.interp);
    return out;
  }
  class AudioNode extends ET {
    constructor(token, ctx, init) {
      if (token !== secret) throw illegal();
      super();
      nodes.set(this, {
        self: this, ctx, inputs: [], outs: [], q: -1, out: null, busy: false,
        numberOfInputs: 1, numberOfOutputs: 1, channelCount: 2, mode: 'max', interp: 'speakers',
        process: () => silentBus(),
        ...init,
      });
    }
    get context() { return nodeOf(this).ctx; }
    get numberOfInputs() { return nodeOf(this).numberOfInputs; }
    get numberOfOutputs() { return nodeOf(this).numberOfOutputs; }
    get channelCount() { return nodeOf(this).channelCount; }
    set channelCount(v) {
      const n = nodeOf(this);
      const count = ulong(v);
      if (n.checkCount) { n.checkCount(count); return; }
      if (count < 1 || count > MAX_CHANNELS) {
        throw new DE(`Failed to set the 'channelCount' property on 'AudioNode': The channel count provided (${count}) is outside the range [1, ${MAX_CHANNELS}].`, 'NotSupportedError');
      }
      n.channelCount = count;
    }
    get channelCountMode() { return nodeOf(this).mode; }
    set channelCountMode(v) {
      const n = nodeOf(this);
      const mode = `${v}`;
      if (mode !== 'max' && mode !== 'clamped-max' && mode !== 'explicit') return;
      if (mode === 'max' && n.disallowMaxMode) {
        throw new DE('This node supports only explicit or clamped-max channel count.', 'NotSupportedError');
      }
      if (n.fixedMode && mode !== n.mode) {
        throw new DE(`Failed to set the 'channelCountMode' property on 'AudioNode': ChannelCountMode cannot be changed from '${n.mode}' to '${mode}'`, 'InvalidStateError');
      }
      n.mode = mode;
    }
    get channelInterpretation() { return nodeOf(this).interp; }
    set channelInterpretation(v) {
      const n = nodeOf(this);
      const interp = `${v}`;
      if (interp !== 'speakers' && interp !== 'discrete') return;
      if (n.fixedInterpretation && interp !== n.interp) throw new DE('The channel interpretation is fixed.', 'InvalidStateError');
      n.interp = interp;
    }
    connect(destination, output = 0, input = 0) {
      const n = nodeOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'connect' on 'AudioNode': 1 argument required, but only 0 present.");
      const out = ulong(output);
      const p = params.get(destination);
      const d = nodes.get(destination);
      if (!p && !d) throw new TypeErrorC("Failed to execute 'connect' on 'AudioNode': Overload resolution failed.");
      if ((p ? p.ctx : d.ctx) !== n.ctx) {
        throw new DE("Failed to execute 'connect' on 'AudioNode': cannot connect to an AudioNode belonging to a different audio context.", 'InvalidAccessError');
      }
      if (out >= n.numberOfOutputs) {
        throw new DE(`Failed to execute 'connect' on 'AudioNode': output index (${out}) exceeds number of outputs (${n.numberOfOutputs}).`, 'IndexSizeError');
      }
      if (p) {
        if (!p.inputs.some((s) => s.node === n && s.output === out)) p.inputs.push({ node: n, output: out });
        if (!n.outs.some((o) => o.param === p && o.output === out)) n.outs.push({ param: p, target: destination, output: out });
        return undefined;
      }
      const inp = ulong(input);
      if (inp >= d.numberOfInputs) {
        throw new DE(`Failed to execute 'connect' on 'AudioNode': input index (${inp}) exceeds number of inputs (${d.numberOfInputs}).`, 'IndexSizeError');
      }
      if (!d.inputs.some(s => s.node === n && s.output === out && s.input === inp)) {
        d.inputs.push({ node: n, output: out, input: inp });
        n.outs.push({ node: d, target: destination, output: out, input: inp });
      }
      return destination;
    }
    disconnect(destination, output, input) {
      const n = nodeOf(this);
      const drop = (o) => {
        if (o.param) o.param.inputs = o.param.inputs.filter((s) => s.node !== n || s.output !== o.output);
        else o.node.inputs = o.node.inputs.filter(s => s.node !== n || s.output !== o.output || s.input !== o.input);
      };
      if (arguments.length === 0) {
        for (const o of n.outs) drop(o);
        n.outs = [];
        return;
      }
      if (typeof destination === 'number' || (!params.has(destination) && !nodes.has(destination))) {
        const out = ulong(destination);
        if (out >= n.numberOfOutputs) {
          throw new DE(`Failed to execute 'disconnect' on 'AudioNode': The output index provided (${out}) is outside the range [0, ${n.numberOfOutputs - 1}].`, 'IndexSizeError');
        }
        for (const o of n.outs) if (o.output === out) drop(o);
        n.outs = n.outs.filter((o) => o.output !== out);
        return;
      }
      if (arguments.length >= 3 && params.has(destination)) throw new TypeErrorC('The three-argument overload requires an AudioNode.');
      const out = arguments.length >= 2 ? ulong(output) : null;
      if (out !== null) {
        if (out >= n.numberOfOutputs) {
          throw new DE(`Failed to execute 'disconnect' on 'AudioNode': The output index provided (${out}) is outside the range [0, ${n.numberOfOutputs - 1}].`, 'IndexSizeError');
        }
      }
      const inp = arguments.length >= 3 ? ulong(input) : null;
      if (inp !== null && nodes.has(destination)) {
        const d = nodes.get(destination);
        if (inp >= d.numberOfInputs) {
          throw new DE(`Failed to execute 'disconnect' on 'AudioNode': The input index provided (${inp}) is outside the range [0, ${d.numberOfInputs - 1}].`, 'IndexSizeError');
        }
      }
      const matching = n.outs.filter(o => o.target === destination
        && (out === null || o.output === out)
        && (inp === null || o.input === inp));
      if (!matching.length) {
        throw new DE("Failed to execute 'disconnect' on 'AudioNode': the given destination is not connected.", 'InvalidAccessError');
      }
      for (const o of matching) drop(o);
      n.outs = n.outs.filter((o) => !matching.includes(o));
    }
  }
  webidl(AudioNode, 'AudioNode');

  // WebIDL converts inherited dictionary members before derived members.
  function readNodeOptions(options, iface) {
    if (options == null) return {};
    if (typeof options !== 'object' && typeof options !== 'function') {
      throw new TypeErrorC(`Failed to construct '${iface}': The provided value is not of type '${iface}Options'.`);
    }
    const result = {};
    const count = options.channelCount;
    if (count !== undefined) result.channelCount = ulong(count);
    const mode = options.channelCountMode;
    if (mode !== undefined) {
      result.channelCountMode = `${mode}`;
      if (!['max', 'clamped-max', 'explicit'].includes(result.channelCountMode)) throw new TypeErrorC('Invalid ChannelCountMode.');
    }
    const interpretation = options.channelInterpretation;
    if (interpretation !== undefined) {
      result.channelInterpretation = `${interpretation}`;
      if (!['speakers', 'discrete'].includes(result.channelInterpretation)) throw new TypeErrorC('Invalid ChannelInterpretation.');
    }
    return result;
  }
  function applyNodeOptions(node, options) {
    if (options.channelCount !== undefined) node.channelCount = options.channelCount;
    if (options.channelCountMode !== undefined) node.channelCountMode = options.channelCountMode;
    if (options.channelInterpretation !== undefined) node.channelInterpretation = options.channelInterpretation;
  }
  function nodeOptions(n, options, iface) {
    applyNodeOptions(n.self, readNodeOptions(options, iface));
    return options ?? {};
  }

  // ---- AudioDestinationNode ------------------------------------------------------
  class AudioDestinationNode extends AudioNode {
    constructor(token, ctx, channels, offline) {
      if (token !== secret) throw illegal();
      super(secret, ctx, {
        numberOfOutputs: 0, channelCount: channels, mode: 'explicit', maxChannelCount: offline ? channels : 2,
        checkCount(count) {
          if (offline) {
            if (count !== channels) {
              throw new DE(`Failed to set the 'channelCount' property on 'AudioNode': Cannot change channel count of an OfflineAudioContext destination to ${count}.`, 'InvalidStateError');
            }
            return;
          }
          if (count < 1 || count > this.maxChannelCount) {
            throw new DE(`Failed to set the 'channelCount' property on 'AudioNode': The channel count provided (${count}) is outside the range [1, ${this.maxChannelCount}].`, 'IndexSizeError');
          }
          this.channelCount = count;
        },
      });
      const n = nodeOf(this);
      n.process = (f0, q) => {
        const bus = input(n, q, f0);
        const out = [];
        for (let c = 0; c < n.channelCount; c++) out.push(new F32(QUANTUM));
        if (bus) mix(bus, out, n.interp);
        return out;
      };
    }
    get maxChannelCount() { return nodeOf(this).maxChannelCount; }
  }
  webidl(AudioDestinationNode, 'AudioDestinationNode');

  // ---- GainNode ----------------------------------------------------------------
  class GainNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'GainNode');
      super(secret, context, {});
      const n = nodeOf(this);
      const gain = nodeParam(n, 1);
      n.gain = gain;
      const o = nodeOptions(n, options, 'GainNode');
      if (o.gain !== undefined) params.get(gain).value = Math.fround(finite(o.gain, 'construct', 'GainNode'));
      n.process = (f0, q) => {
        const bus = input(n, q, f0);
        if (!bus) return silentBus();
        const g = paramValues(params.get(gain), f0, ctxOf(context).rate, q);
        for (const ch of bus) for (let i = 0; i < QUANTUM; i++) ch[i] *= g[i];
        return bus;
      };
    }
    get gain() { return nodeOf(this).gain; }
  }
  webidl(GainNode, 'GainNode');

  const analysers = new WeakMap();
  function analyserOf(self) {
    const state = analysers.get(self);
    if (!state) throw invocation();
    return state;
  }
  function analyserSpectrum(state) {
    if (!state.dirty) return;
    const size = state.size, real = new F64(size), imaginary = new F64(size);
    for (let index = 0, reversed = 0; index < size; index++) {
      const angle = 2 * PI * index / size;
      const sample = state.history[(state.cursor - size + index) & 32767];
      real[reversed] = sample * (0.42 - 0.5 * cos(angle) + 0.08 * cos(2 * angle));
      let bit = size >> 1;
      while (bit && (reversed & bit)) { reversed ^= bit; bit >>= 1; }
      reversed ^= bit;
    }
    for (let width = 2; width <= size; width *= 2) {
      const half = width / 2;
      for (let offset = 0; offset < half; offset++) {
        const angle = -2 * PI * offset / width, cosine = cos(angle), sine = sin(angle);
        for (let left = offset; left < size; left += width) {
          const right = left + half;
          const re = real[right] * cosine - imaginary[right] * sine;
          const im = real[right] * sine + imaginary[right] * cosine;
          real[right] = real[left] - re; imaginary[right] = imaginary[left] - im;
          real[left] += re; imaginary[left] += im;
        }
      }
    }
    for (let bin = 0; bin < size / 2; bin++) {
      state.spectrum[bin] = state.smoothing * state.spectrum[bin] + (1 - state.smoothing) * hypot(real[bin], imaginary[bin]) / size;
    }
    state.dirty = false;
  }
  function readAnalyser(self, array, frequency, bytes) {
    const state = analyserOf(self);
    const type = bytes ? 'Uint8Array' : 'Float32Array';
    if (apply(arrayKind, array, []) !== type) throw new TypeErrorC(`Expected a ${type}`);
    const buffer = apply(arrayBufferOf, array, []);
    apply(bufferLength, buffer, []);
    if (resizable && apply(resizable, buffer, [])) throw new TypeErrorC('Expected a fixed-length ArrayBuffer');
    const count = min(apply(arrayLength, array, []), frequency ? state.size / 2 : state.size);
    if (frequency) analyserSpectrum(state);
    for (let index = 0; index < count; index++) {
      const value = frequency ? 20 * Math.log10(state.spectrum[index]) : state.history[(state.cursor - state.size + index) & 32767];
      array[index] = bytes ? min(255, max(0, floor(frequency ? 255 * (value - state.low) / (state.high - state.low) : 128 * (1 + value)))) : value;
    }
  }
  class AnalyserNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'AnalyserNode');
      super(secret, context, {});
      const node = nodeOf(this), opts = nodeOptions(node, options, 'AnalyserNode');
      const { fftSize = 2048, maxDecibels = -30, minDecibels = -100, smoothingTimeConstant = 0.8 } = opts;
      const high = finite(maxDecibels, 'construct', 'AnalyserNode', 'double'), low = finite(minDecibels, 'construct', 'AnalyserNode', 'double');
      if (low >= high) throw new DE('Invalid decibel range.', 'IndexSizeError');
      const state = { history: new F32(32768), cursor: 0, low, high };
      analysers.set(this, state);
      this.fftSize = fftSize;
      this.smoothingTimeConstant = smoothingTimeConstant;
      (ctxOf(context).analyserNodes ??= new Set()).add(new WeakRefC(node));
      node.process = (frame, quantum) => {
        const bus = input(node, quantum, frame) ?? silentBus();
        const mono = new F32(QUANTUM);
        mix(bus, [mono], 'speakers');
        state.history.set(mono, state.cursor);
        state.cursor = (state.cursor + QUANTUM) & 32767;
        state.dirty = true;
        return bus;
      };
    }
    get fftSize() { return analyserOf(this).size; }
    set fftSize(value) {
      const state = analyserOf(this), size = ulong(value);
      if (size < 32 || size > 32768 || (size & (size - 1))) throw new DE('Invalid FFT size.', 'IndexSizeError');
      if (state.size !== size) { state.size = size; state.spectrum = new F64(size / 2); state.dirty = true; }
    }
    get frequencyBinCount() { return analyserOf(this).size / 2; }
    get minDecibels() { return analyserOf(this).low; }
    set minDecibels(value) {
      const state = analyserOf(this), low = finite(value, 'set', 'AnalyserNode', 'double');
      if (low >= state.high) throw new DE('Invalid minimum decibels.', 'IndexSizeError');
      state.low = low;
    }
    get maxDecibels() { return analyserOf(this).high; }
    set maxDecibels(value) {
      const state = analyserOf(this), high = finite(value, 'set', 'AnalyserNode', 'double');
      if (high <= state.low) throw new DE('Invalid maximum decibels.', 'IndexSizeError');
      state.high = high;
    }
    get smoothingTimeConstant() { return analyserOf(this).smoothing; }
    set smoothingTimeConstant(value) {
      const state = analyserOf(this), smoothing = finite(value, 'set', 'AnalyserNode', 'double');
      if (smoothing < 0 || smoothing > 1) throw new DE('Invalid smoothing constant.', 'IndexSizeError');
      state.smoothing = smoothing;
    }
    getByteTimeDomainData(array) { readAnalyser(this, array, false, true); }
    getFloatTimeDomainData(array) { readAnalyser(this, array, false, false); }
    getByteFrequencyData(array) { readAnalyser(this, array, true, true); }
    getFloatFrequencyData(array) { readAnalyser(this, array, true, false); }
  }
  webidl(AnalyserNode, 'AnalyserNode');

  function writeEqualPower(bus, left, right, i, pan, gain) {
    if (bus.length === 1) {
      const angle = (pan + 1) * PI / 4;
      left[i] = bus[0][i] * cos(angle);
      right[i] = bus[0][i] * sin(angle);
    } else if (pan <= 0) {
      const angle = (pan + 1) * PI / 2;
      left[i] = bus[0][i] + bus[1][i] * cos(angle);
      right[i] = bus[1][i] * sin(angle);
    } else {
      const angle = pan * PI / 2;
      left[i] = bus[0][i] * cos(angle);
      right[i] = bus[1][i] + bus[0][i] * sin(angle);
    }
    // Distance and cone attenuation act on the stereo PCM produced by the panner.
    if (gain !== 1) {
      left[i] *= gain;
      right[i] *= gain;
    }
  }

  function pannerChannels(count) {
    if (count < 1 || count > 2) {
      const kind = this.spatial ? 'spatial' : 'stereo';
      throw new DE(`A ${kind} panner accepts one or two input channels.`, 'NotSupportedError');
    }
    this.channelCount = count;
  }

  class StereoPannerNode extends AudioNode {
    constructor(context, options = {}) {
      contextArg(context, 'StereoPannerNode');
      const settings = readNodeOptions(options, 'StereoPannerNode');
      const initialPan = options?.pan;
      const value = initialPan === undefined ? 0 : finite(Math.fround(initialPan), 'construct', 'StereoPannerNode');
      super(secret, context, {
        mode: 'clamped-max',
        disallowMaxMode: true,
        checkCount: pannerChannels,
      });
      applyNodeOptions(this, settings);
      const n = nodeOf(this);
      const pan = nodeParam(n, 0, -1, 1);
      pan.value = value;
      n.pan = pan;
      n.process = (f0, q) => {
        const bus = input(n, q, f0);
        const left = new F32(QUANTUM), right = new F32(QUANTUM);
        if (bus) {
          const values = paramValues(params.get(pan), f0, ctxOf(context).rate, q);
          for (let i = 0; i < QUANTUM; i++) writeEqualPower(bus, left, right, i, values[i], 1);
        }
        return [left, right];
      };
    }
    get pan() {
      const n = nodeOf(this);
      if (!n.pan) throw invocation();
      return n.pan;
    }
  }
  webidl(StereoPannerNode, 'StereoPannerNode');

  // Spatial AudioParams are shared by the legacy coordinate setters and the
  // renderer. A listener belongs to its context, independently of its panners.
  const listenerStates = new WeakMap();
  const positionNames = ['positionX', 'positionY', 'positionZ'];
  const orientationNames = ['orientationX', 'orientationY', 'orientationZ'];
  const listenerOrientation = ['forwardX', 'forwardY', 'forwardZ', 'upX', 'upY', 'upZ'];
  const listenerNames = [...positionNames, ...listenerOrientation];
  const pannerNames = [...positionNames, ...orientationNames];

  function spatialParams(ctx, names, defaults) {
    const result = Object.create(null);
    for (let j = 0; j < names.length; j++) result[names[j]] = new AudioParam(secret, ctx, defaults[j]);
    return result;
  }
  function spatialAttributes(C, stateOf, names) {
    for (const name of names) {
      defineProperty(C.prototype, name, {
        configurable: true, get() { return stateOf(this).spatial[name]; },
      });
    }
  }
  function setCoordinates(state, names, args) {
    const values = names.map((name, i) => finite(Math.fround(args[i]), 'set', name));
    const t = ctxOf(params.get(state.spatial[names[0]]).ctx).time();
    for (const name of names) checkValueAtTime(params.get(state.spatial[name]), t);
    for (let i = 0; i < names.length; i++) setParamValue(params.get(state.spatial[names[i]]), values[i], t);
  }
  function listenerOf(receiver) {
    const state = listenerStates.get(receiver);
    if (!state) throw invocation();
    return state;
  }
  class AudioListener {
    constructor(token = undefined, context) {
      if (token !== secret) throw illegal();
      listenerStates.set(this, {spatial: spatialParams(context, listenerNames, [0, 0, 0, 0, 0, -1, 0, 1, 0])});
    }
    setPosition(x, y, z) { setCoordinates(listenerOf(this), positionNames, arguments); }
    setOrientation(x, y, z, xUp, yUp, zUp) { setCoordinates(listenerOf(this), listenerOrientation, arguments); }
  }
  spatialAttributes(AudioListener, listenerOf, listenerNames);
  webidl(AudioListener, 'AudioListener');
  function contextListener(ctx) {
    const c = ctxOf(ctx);
    return c.listener || (c.listener = new AudioListener(secret, ctx));
  }
  function pannerOf(receiver) {
    const state = nodeOf(receiver);
    if (!state.spatial) throw invocation();
    return state;
  }
  const distanceModels = ['linear', 'inverse', 'exponential'];
  function distanceAttenuation(n, d) {
    const ref = n.refDistance, rolloff = n.rolloffFactor;
    if (n.distanceModel === 'linear') {
      const low = min(ref, n.maxDistance), high = max(ref, n.maxDistance);
      const fraction = low === high ? 1 : (min(max(d, low), high) - low) / (high - low);
      return 1 - min(rolloff, 1) * fraction;
    }
    if (ref === 0) return 0;
    d = max(d, ref);
    return n.distanceModel === 'inverse' ? ref / (ref + rolloff * (d - ref)) : pow(d / ref, -rolloff);
  }
  function coneAttenuation(n, dx, dy, dz, d, ox, oy, oz) {
    if (n.coneInnerAngle === 360 && n.coneOuterAngle === 360) return 1;
    const length = hypot(ox, oy, oz);
    if (!length) return 1;
    const cosine = d === 0 ? 0 : -(dx * ox + dy * oy + dz * oz) / (d * length);
    const angle = acos(max(-1, min(1, cosine))) * 180 / PI;
    const inner = abs(n.coneInnerAngle) / 2, outer = abs(n.coneOuterAngle) / 2;
    if (angle <= inner) return 1;
    if (angle >= outer) return n.coneOuterGain;
    return 1 + (n.coneOuterGain - 1) * (angle - inner) / (outer - inner);
  }
  function renderSpatial(n, f0, q) {
    const left = new F32(QUANTUM), right = new F32(QUANTUM);
    const bus = input(n, q, f0);
    if (!bus) return [left, right];
    const rate = ctxOf(n.ctx).rate;
    const source = n.spatial, listener = listenerOf(contextListener(n.ctx)).spatial;
    let stride = QUANTUM;
    function values(param) {
      const state = params.get(param);
      if (state.rate === 'a-rate' && (state.events.length || state.inputs.length)) stride = 1;
      return paramValues(state, f0, rate, q);
    }
    const [sx, sy, sz, ox, oy, oz] = pannerNames.map(k => values(source[k]));
    const [lx, ly, lz, fx, fy, fz, ux, uy, uz] = listenerNames.map(k => values(listener[k]));
    // Static and k-rate coordinates have one listener-relative position per quantum.
    for (let i = 0; i < QUANTUM; i += stride) {
      const dx = sx[i] - lx[i], dy = sy[i] - ly[i], dz = sz[i] - lz[i];
      const d = hypot(dx, dy, dz);
      // Project the source onto the listener's orthonormal right/forward axes.
      // asin of its horizontal right component folds rear azimuths into [-90, 90].
      const rx = fy[i] * uz[i] - fz[i] * uy[i];
      const ry = fz[i] * ux[i] - fx[i] * uz[i];
      const rz = fx[i] * uy[i] - fy[i] * ux[i];
      const rightLength = hypot(rx, ry, rz), forwardLength = hypot(fx[i], fy[i], fz[i]);
      let pan = 0;
      if (d && rightLength && forwardLength) {
        const side = (dx * rx + dy * ry + dz * rz) / rightLength;
        const front = (dx * fx[i] + dy * fy[i] + dz * fz[i]) / forwardLength;
        const horizontal = hypot(side, front);
        if (horizontal) pan = asin(max(-1, min(1, side / horizontal))) * 2 / PI;
      }
      const gain = fround(distanceAttenuation(n, d) * coneAttenuation(n, dx, dy, dz, d, ox[i], oy[i], oz[i]));
      for (let j = i; j < i + stride; j++) writeEqualPower(bus, left, right, j, pan, gain);
    }
    return [left, right];
  }
  const pannerDefaults = {coneInnerAngle: 360, coneOuterAngle: 360, coneOuterGain: 0,
    maxDistance: 10000, refDistance: 1, rolloffFactor: 1};
  function setPannerScalar(n, name, value) {
    if (name === 'coneOuterGain' && (value < 0 || value > 1)) throw new DE('Cone gain is outside [0, 1].', 'InvalidStateError');
    if ((name === 'maxDistance' && value <= 0) ||
        ((name === 'refDistance' || name === 'rolloffFactor') && value < 0)) throw new RangeErrorC('Invalid panner distance or rolloff.');
    n[name] = value;
  }
  function setPanningModel(model) {
    if (model === 'HRTF') throw new DE('HRTF rendering is unavailable.', 'NotSupportedError');
  }
  class PannerNode extends AudioNode {
    constructor(context, options = {}) {
      contextArg(context, 'PannerNode');
      const inherited = readNodeOptions(options, 'PannerNode');
      const settings = Object.create(null);
      for (const name of pannerOptionNames) {
        const value = options?.[name];
        if (value === undefined) continue;
        if (name === 'panningModel' || name === 'distanceModel') {
          const allowed = name === 'panningModel' ? ['equalpower', 'HRTF'] : distanceModels;
          const model = `${value}`;
          if (!allowed.includes(model)) throw new TypeErrorC('Invalid ' + name);
          settings[name] = model;
        } else {
          settings[name] = finite(pannerNames.includes(name) ? Math.fround(value) : value, 'construct', 'PannerNode');
        }
      }
      super(secret, context, {mode: 'clamped-max', disallowMaxMode: true, checkCount: pannerChannels,
        ...pannerDefaults, distanceModel: 'inverse', spatial: spatialParams(context, pannerNames, [0, 0, 0, 1, 0, 0])});
      applyNodeOptions(this, inherited);
      const n = nodeOf(this);
      const listener = listenerOf(contextListener(context)).spatial;
      n.audioParams = [...pannerNames.map(k => params.get(n.spatial[k])), ...listenerNames.map(k => params.get(listener[k]))];
      for (const name of Object.keys(settings)) {
        if (pannerNames.includes(name)) params.get(n.spatial[name]).value = settings[name];
        else if (name === 'panningModel') setPanningModel(settings[name]);
        else if (name === 'distanceModel') n.distanceModel = settings[name];
        else setPannerScalar(n, name, settings[name]);
      }
      n.process = (f0, q) => renderSpatial(n, f0, q);
    }
    get distanceModel() { return pannerOf(this).distanceModel; }
    set distanceModel(v) {
      const n = pannerOf(this), model = `${v}`;
      if (distanceModels.includes(model)) n.distanceModel = model;
    }
    get panningModel() { pannerOf(this); return 'equalpower'; }
    set panningModel(v) { pannerOf(this); setPanningModel(`${v}`); }
    setPosition(x, y, z) { setCoordinates(pannerOf(this), positionNames, arguments); }
    setOrientation(x, y, z) { setCoordinates(pannerOf(this), orientationNames, arguments); }
  }
  const pannerOptionNames = [...Object.keys(pannerDefaults), ...pannerNames, 'distanceModel', 'panningModel'].sort();
  spatialAttributes(PannerNode, pannerOf, pannerNames);
  for (const name of Object.keys(pannerDefaults)) {
    defineProperty(PannerNode.prototype, name, {
      configurable: true, get() { return pannerOf(this)[name]; },
      set(v) { setPannerScalar(pannerOf(this), name, finite(v, 'set', name, 'double')); },
    });
  }
  webidl(PannerNode, 'PannerNode');

  // A delay in a feedback path reads the previous render quantum before its
  // current input is pulled. Include signal connections to AudioParams when
  // finding the path. Topology is stable between offline render suspensions.
  function delayInCycle(delay) {
    const ctx = ctxOf(delay.ctx);
    if (delay.graphEpoch === ctx.graphEpoch) return delay.inCycle;
    const pending = [delay], seen = new Set([delay]);
    let cyclic = false;
    function visit(connection) {
      const source = connection.node;
      if (source === delay) cyclic = true;
      else if (!seen.has(source)) { seen.add(source); pending.push(source); }
    }
    while (pending.length && !cyclic) {
      const node = pending.pop();
      for (const edge of node.inputs) visit(edge);
      if (node.audioParams) for (const p of node.audioParams) for (const edge of p.inputs) visit(edge);
    }
    delay.graphEpoch = ctx.graphEpoch;
    delay.inCycle = cyclic;
    return cyclic;
  }
  function writeDelay(n, q, f0) {
    const bus = input(n, q, f0), slot = q % n.slots;
    if (bus) n.blocks.set(slot, {q, bus});
    else n.blocks.delete(slot);
  }
  function delayBlock(n, frame) {
    if (frame < 0) return null;
    const q = floor(frame / QUANTUM), block = n.blocks.get(q % n.slots);
    return block?.q === q ? block.bus : null;
  }
  function readDelay(n, q, f0) {
    const ctx = ctxOf(n.ctx), cyclic = delayInCycle(n);
    if (cyclic) (ctx.delayWriters ??= new Set()).add(n);
    else writeDelay(n, q, f0);
    const times = paramValues(params.get(n.delayTime), f0, ctx.rate, q);
    const positions = n.positions;
    let channels = 1;
    // Delay the channel layout with the samples. Where interpolation or
    // automation touches two layouts, mix them to this quantum's output width.
    for (let i = 0; i < QUANTUM; i++) {
      const duration = max(cyclic ? QUANTUM : 0, fround(times[i] * ctx.rate));
      const pos = f0 + i - duration, prev = floor(pos);
      positions[i] = pos;
      channels = max(channels, delayBlock(n, prev)?.length ?? 1);
      if (prev !== pos) channels = max(channels, delayBlock(n, prev + 1)?.length ?? 1);
    }
    const out = Array.from({length: channels}, () => new F32(QUANTUM));
    const converted = new Map();
    function source(frame) {
      const bus = delayBlock(n, frame);
      if (!bus || bus.length === channels) return bus;
      if (!converted.has(bus)) {
        const normalized = Array.from({length: channels}, () => new F32(QUANTUM));
        mix(bus, normalized, n.interp);
        converted.set(bus, normalized);
      }
      return converted.get(bus);
    }
    for (let i = 0; i < QUANTUM; i++) {
      const pos = positions[i], a = floor(pos), frac = pos - a;
      const first = source(a), second = frac ? source(a + 1) : null;
      for (let ch = 0; ch < channels; ch++) {
        const v = first ? first[ch][a % QUANTUM] : 0;
        out[ch][i] = frac ? v + frac * ((second ? second[ch][(a + 1) % QUANTUM] : 0) - v) : v;
      }
    }
    return out;
  }
  class DelayNode extends AudioNode {
    constructor(context, options = {}) {
      contextArg(context, 'DelayNode');
      const inherited = readNodeOptions(options, 'DelayNode');
      const initial = options?.delayTime, delay = initial === undefined ? 0 : finite(initial, 'construct', 'DelayNode', 'double');
      const requested = options?.maxDelayTime, limit = requested === undefined ? 1 : finite(requested, 'construct', 'DelayNode', 'double');
      if (limit <= 0 || limit >= 180) throw new DE('Maximum delay must be greater than zero and below 180 seconds.', 'NotSupportedError');
      super(secret, context, {});
      applyNodeOptions(this, inherited);
      const n = nodeOf(this);
      n.delayTime = nodeParam(n, 0, 0, fround(limit));
      params.get(n.delayTime).value = fround(delay);
      // Store only the rendered part of the delay line, bounded by its maximum
      // age. Two extra quanta cover the neighboring sample and the current write.
      n.slots = max(1, ceil(limit * ctxOf(context).rate / QUANTUM)) + 2;
      n.blocks = new Map();
      n.positions = new Float64Array(QUANTUM);
      n.process = (f0, q) => readDelay(n, q, f0);
      (ctxOf(context).delayNodes ??= new Set()).add(new WeakRefC(n));
    }
    get delayTime() {
      const n = nodeOf(this);
      if (!n.delayTime) throw invocation();
      return n.delayTime;
    }
  }
  webidl(DelayNode, 'DelayNode');

  // ---- Convolver -------------------------------------------------------------
  const WeakConvolver = WeakRef;
  function convolverOf(self) {
    const n = nodeOf(self);
    if (n.convolve === undefined) throw invocation();
    return n;
  }
  // Buffer acquisition runs in the AudioBuffer's own realm so its refreshed
  // channel views have that realm's intrinsic Float32Array prototype.
  function localImpulseInfo(buffer) {
    const b = buffers.get(buffer);
    if (!b) return undefined;
    return {rate: b.rate, length: b.length, count: b.data.length, acquire() {
      if (b.data.some(ch => apply(arrayLength, ch, []) !== b.length)) return null;
      b.data = b.data.map(ch => {
        const backing = apply(arrayBufferOf, ch, []);
        const next = transferOf ? apply(transferOf, backing, []) : structuredCloneC(backing, {transfer: [backing]});
        return new F32(next);
      });
      return b.data;
    }};
  }
  function impulseInfo(buffer) {
    if (buffer == null) return null;
    const info = localImpulseInfo(buffer) || natives.foreignImpulseInfo(buffer);
    if (!info) throw new TypeErrorC('Expected an AudioBuffer');
    return info;
  }
  function setImpulse(n, buffer, info) {
    if (!info) {
      n.convolve = n.impulse = null;
      n.tail = 0;
      return;
    }
    if (![1, 2, 4].includes(info.count) || info.rate !== ctxOf(n.ctx).rate) {
      throw new DE('The impulse response requires 1, 2 or 4 channels and the context sample rate.', 'NotSupportedError');
    }
    const data = info.acquire();
    n.convolve = data ? natives.makeConvolver(data, info.rate, n.normalize) : null;
    n.impulse = buffer;
    n.impulseLength = info.length;
    n.tail = 0;
  }
  class ConvolverNode extends AudioNode {
    constructor(context, options = undefined) {
      contextArg(context, 'ConvolverNode');
      const inherited = readNodeOptions(options, 'ConvolverNode');
      const source = options?.buffer;
      const info = impulseInfo(source);
      const normalize = !options?.disableNormalization;
      super(secret, context, {
        mode: 'clamped-max', disallowMaxMode: true, normalize,
        impulse: null, convolve: null, tail: 0, impulseLength: 0,
        checkCount(count) {
          if (count !== 1 && count !== 2) throw new DE('Convolver input requires 1 or 2 channels.', 'NotSupportedError');
          this.channelCount = count;
        },
      });
      const n = nodeOf(this);
      applyNodeOptions(this, inherited);
      const c = ctxOf(context);
      c.trackConvolverActivity = true;
      (c.convolverNodes ??= new Set()).add(new WeakConvolver(n));
      if (source != null) setImpulse(n, source, info);
      n.process = (f0, q) => {
        if (!n.convolve) return silentBus();
        const bus = input(n, q, f0);
        if (!n.activeInput && n.tail === 0) return silentBus();
        n.processingFrame = f0;
        n.tail = n.activeInput ? n.impulseLength - 1 : max(0, n.tail - QUANTUM);
        return n.convolve(bus ? bus[0] : new F32(QUANTUM), bus?.[1], n.interp === 'speakers');
      };
    }
    get normalize() { return convolverOf(this).normalize; }
    set normalize(value) { convolverOf(this).normalize = !!value; }
    get buffer() { return convolverOf(this).impulse; }
    set buffer(value) { setImpulse(convolverOf(this), value, impulseInfo(value)); }
  }
  webidl(ConvolverNode, 'ConvolverNode');

  // ---- Recursive filters -----------------------------------------------------
  const F64 = Float64Array;
  const typedArrayPrototype = Object.getPrototypeOf(F32.prototype);
  const arrayKind = getOwnPropertyDescriptor(typedArrayPrototype, Symbol.toStringTag).get;
  const arrayBufferOf = getOwnPropertyDescriptor(typedArrayPrototype, 'buffer').get;
  const arrayLength = getOwnPropertyDescriptor(typedArrayPrototype, 'length').get;
  const bufferLength = getOwnPropertyDescriptor(ArrayBufferC.prototype, 'byteLength').get;
  const resizable = getOwnPropertyDescriptor(ArrayBufferC.prototype, 'resizable')?.get;
  const { sqrt, hypot, atan2 } = Math;

  // Both nodes implement a rational transfer function B(z)/A(z). Keep input
  // and output samples separately: a biquad's coefficients can change at each
  // sample, while its previous samples still belong to the same channel.
  function filterProcess(n, f0, q, update) {
    const bus = input(n, q, f0);
    const channels = bus || silentBus();
    // A channel disappearing from the input remains audible for its tail.
    // Retire it once its delay line no longer contributes to any output.
    while (n.history.length > channels.length) {
      const last = n.history[n.history.length - 1];
      if (last.x.some(v => v !== 0) || last.y.some(v => v !== 0)) break;
      n.history.pop();
    }
    const count = max(channels.length, n.history.length);
    while (channels.length < count) channels.push(new F32(QUANTUM));
    const size = max(n.a.length, n.b.length);
    while (n.history.length < count) n.history.push({x: new F64(size), y: new F64(size)});
    for (let i = 0; i < QUANTUM; i++) {
      if (update) update(i);
      const position = n.position;
      for (let c = 0; c < count; c++) {
        const {x, y} = n.history[c];
        const sample = channels[c][i];
        x[position] = sample;
        let filtered = n.b[0] * sample;
        for (let k = 1, previous = position; k < size; k++) {
          if (--previous < 0) previous = size - 1;
          if (k < n.b.length) filtered += n.b[k] * x[previous];
          if (k < n.a.length) filtered -= n.a[k] * y[previous];
        }
        y[position] = filtered;
        channels[c][i] = filtered;
      }
      n.position = (position + 1) % size;
    }
    return channels;
  }

  function polynomial(coefficients, re, im) {
    let real = 0, imaginary = 0;
    for (let k = coefficients.length - 1; k >= 0; k--) {
      const nextReal = real * re - imaginary * im + coefficients[k];
      imaginary = real * im + imaginary * re;
      real = nextReal;
    }
    return [real, imaginary];
  }

  function frequencyResponse(n, frequencies, magnitudes, phases) {
    const lengths = [frequencies, magnitudes, phases].map(a => {
      if (apply(arrayKind, a, []) !== 'Float32Array') throw new TypeErrorC('Expected a Float32Array');
      const buffer = apply(arrayBufferOf, a, []);
      // The ArrayBuffer getter brand-checks the backing buffer, excluding
      // SharedArrayBuffer even when it belongs to another realm.
      apply(bufferLength, buffer, []);
      if (resizable && apply(resizable, buffer, [])) throw new TypeErrorC('Expected a fixed-length ArrayBuffer');
      return apply(arrayLength, a, []);
    });
    const length = lengths[0];
    if (length !== lengths[1] || length !== lengths[2]) {
      throw new DE('The frequency and response arrays must have the same length.', 'InvalidAccessError');
    }
    const rate = ctxOf(n.ctx).rate;
    for (let i = 0; i < length; i++) {
      const hz = frequencies[i];
      if (!(hz >= 0 && hz <= rate / 2)) {
        magnitudes[i] = phases[i] = NaN;
        continue;
      }
      const angle = -2 * PI * hz / rate;
      const re = cos(angle), im = sin(angle);
      const [br, bi] = polynomial(n.b, re, im);
      const [ar, ai] = polynomial(n.a, re, im);
      magnitudes[i] = hypot(br, bi) / hypot(ar, ai);
      phases[i] = atan2(bi * ar - br * ai, br * ar + bi * ai);
    }
  }

  function filterOf(receiver, kind) {
    const n = nodeOf(receiver);
    if (n.filterKind !== kind) throw invocation();
    return n;
  }

  const biquadTypes = ['lowpass', 'highpass', 'bandpass', 'lowshelf', 'highshelf', 'peaking', 'notch', 'allpass'];

  // Web Audio's Audio EQ Cookbook transfer functions, normalized to a0=1.
  // At DC, Nyquist and zero Q use the limits; a pole and a cancelling zero
  // on the unit circle would otherwise produce a spurious singularity.
  function biquadCoefficients(n, hz, cents, quality, gain) {
    const f = max(0, min(1, hz * pow(2, cents / 1200) / (ctxOf(n.ctx).rate / 2)));
    if (n.lastType === n.type && n.lastF === f && n.lastQ === quality && n.lastGain === gain) return;
    n.lastType = n.type; n.lastF = f; n.lastQ = quality; n.lastGain = gain;
    const t = n.type;
    const A = pow(10, gain / 40);
    let a0 = 1, a1 = 0, a2 = 0, b0 = 1, b1 = 0, b2 = 0;
    if (f === 0 || f === 1) {
      if (t === 'lowpass') b0 = f;
      else if (t === 'highpass') b0 = 1 - f;
      else if (t === 'bandpass') b0 = 0;
      else if ((t === 'lowshelf' && f === 1) || (t === 'highshelf' && f === 0)) b0 = A * A;
    } else if (t === 'highshelf' || t === 'lowshelf') {
      const c = cos(PI * f), beta = sqrt(2 * A) * sin(PI * f);
      const sum = A + 1, diff = A - 1;
      const sign = t === 'highshelf' ? 1 : -1;
      b0 = A * (sum + sign * diff * c + beta);
      b1 = -2 * A * (sign * diff + sum * c);
      b2 = A * (sum + sign * diff * c - beta);
      a0 = sum - sign * diff * c + beta;
      a1 = 2 * (sign * diff - sum * c);
      a2 = sum - sign * diff * c - beta;
    } else if (t === 'lowpass' || t === 'highpass') {
      const c = cos(PI * f), alpha = sin(PI * f) / (2 * pow(10, max(-20 * Math.log10(F32_MAX), min(20 * Math.log10(F32_MAX), quality)) / 20));
      const sign = t === 'lowpass' ? 1 : -1;
      b0 = b2 = (1 - sign * c) / 2;
      b1 = 2 * sign * b0;
      a0 = 1 + alpha; a1 = -2 * c; a2 = 1 - alpha;
    } else if (quality <= 0) {
      if (t === 'notch') b0 = 0;
      else if (t === 'allpass') b0 = -1;
      else if (t === 'peaking') b0 = A * A;
    } else {
      const c = cos(PI * f), alpha = sin(PI * f) / (2 * quality);
      a0 = 1 + alpha; a1 = -2 * c; a2 = 1 - alpha;
      if (t === 'bandpass') {
        b0 = alpha; b2 = -alpha;
      } else if (t === 'notch') {
        b0 = b2 = 1; b1 = a1;
      } else if (t === 'allpass') {
        b0 = a2; b1 = a1; b2 = a0;
      } else {
        a0 = 1 + alpha / A; a2 = 1 - alpha / A;
        b0 = 1 + alpha * A; b1 = a1; b2 = 1 - alpha * A;
      }
    }
    n.b[0] = b0 / a0; n.b[1] = b1 / a0; n.b[2] = b2 / a0;
    n.a[1] = a1 / a0; n.a[2] = a2 / a0;
  }

  class BiquadFilterNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'BiquadFilterNode');
      super(secret, context, {filterKind: 'biquad', type: 'lowpass', a: [1, 0, 0], b: [0, 0, 0], history: [], position: 0});
      const n = nodeOf(this);
      const inherited = readNodeOptions(options, 'BiquadFilterNode');
      const rate = ctxOf(context).rate;
      const parameter = (name, value, lo, hi) => {
        const param = nodeParam(n, value, lo, hi);
        const supplied = options?.[name];
        if (supplied !== undefined) {
          const rounded = Math.fround(+supplied);
          if (!Number.isFinite(rounded)) throw new TypeErrorC(`Expected a finite float for ${name}.`);
          param.value = rounded;
        }
        return param;
      };
      n.Q = parameter('Q', 1, -F32_MAX, F32_MAX);
      n.detune = parameter('detune', 0, -153600, 153600);
      n.frequency = parameter('frequency', 350, 0, rate / 2);
      n.gain = parameter('gain', 0, -F32_MAX, Math.fround(40 * Math.log10(F32_MAX)));
      const suppliedType = options?.type;
      if (suppliedType !== undefined) {
        const type = `${suppliedType}`;
        if (!biquadTypes.includes(type)) throw new TypeErrorC('Unknown BiquadFilterType');
        n.type = type;
      }
      applyNodeOptions(this, inherited);
      n.process = (f0, q) => {
        const values = [n.frequency, n.detune, n.Q, n.gain].map(p => paramValues(params.get(p), f0, rate, q));
        return filterProcess(n, f0, q, i => biquadCoefficients(n, values[0][i], values[1][i], values[2][i], values[3][i]));
      };
    }
    get type() { return filterOf(this, 'biquad').type; }
    set type(value) {
      const n = filterOf(this, 'biquad'), type = String(value);
      if (biquadTypes.includes(type)) n.type = type;
    }
    get Q() { return filterOf(this, 'biquad').Q; }
    get frequency() { return filterOf(this, 'biquad').frequency; }
    get detune() { return filterOf(this, 'biquad').detune; }
    get gain() { return filterOf(this, 'biquad').gain; }
    getFrequencyResponse(frequencies, magnitudes, phases) {
      const n = filterOf(this, 'biquad');
      const current = p => clampParam(params.get(p), p.value);
      biquadCoefficients(n, current(n.frequency), current(n.detune), current(n.Q), current(n.gain));
      frequencyResponse(n, frequencies, magnitudes, phases);
    }
  }
  webidl(BiquadFilterNode, 'BiquadFilterNode');

  function coefficients(value) {
    const iterator = value != null && (typeof value === 'object' || typeof value === 'function')
      ? value[Symbol.iterator] : undefined;
    if (typeof iterator !== 'function') throw new TypeErrorC('Filter coefficients must be an iterable sequence.');
    // Fetch the iterator method only once, including for proxies/getters.
    return Array.from({[Symbol.iterator]: () => apply(iterator, value, [])},
      v => finite(v, 'construct', 'IIRFilterNode', 'double'));
  }
  class IIRFilterNode extends AudioNode {
    constructor(context, options, converted = undefined) {
      contextArg(context, 'IIRFilterNode');
      const inherited = readNodeOptions(options, 'IIRFilterNode');
      const feedback = converted === secret ? options.feedback : coefficients(options?.feedback);
      const feedforward = converted === secret ? options.feedforward : coefficients(options?.feedforward);
      if (![feedback, feedforward].every(a => a.length >= 1 && a.length <= 20)) {
        throw new DE('Filter coefficient lengths must be between 1 and 20.', 'NotSupportedError');
      }
      if (feedback[0] === 0 || feedforward.every(x => x === 0)) {
        throw new DE('The first feedback coefficient and at least one feedforward coefficient must be nonzero.', 'InvalidStateError');
      }
      super(secret, context, {
        filterKind: 'iir', a: feedback.map(v => v / feedback[0]), b: feedforward.map(v => v / feedback[0]), history: [], position: 0,
      });
      const n = nodeOf(this);
      applyNodeOptions(this, inherited);
      n.process = (f0, q) => filterProcess(n, f0, q);
    }
    getFrequencyResponse(frequencies, magnitudes, phases) {
      frequencyResponse(filterOf(this, 'iir'), frequencies, magnitudes, phases);
    }
  }
  webidl(IIRFilterNode, 'IIRFilterNode');

  class ChannelSplitterNode extends AudioNode {
    constructor(context, options = {}) {
      contextArg(context, 'ChannelSplitterNode');
      const inherited = readNodeOptions(options, 'ChannelSplitterNode');
      const outputs = options?.numberOfOutputs;
      const count = outputs === undefined ? 6 : ulong(outputs);
      if (!count || count > MAX_CHANNELS) throw new DE('Unsupported number of splitter outputs.', 'IndexSizeError');
      super(secret, context, {
        numberOfOutputs: count, channelCount: count, mode: 'explicit', interp: 'discrete',
        fixedMode: true, fixedInterpretation: true, splitChannels: true,
        checkCount(value) {
          if (value !== count) throw new DE('Splitter channel count must match the number of outputs.', 'InvalidStateError');
        },
      });
      applyNodeOptions(this, inherited);
      const n = nodeOf(this);
      n.process = (f0, q) => input(n, q, f0) || [];
    }
  }
  webidl(ChannelSplitterNode, 'ChannelSplitterNode');

  // Each connected input is down-mixed to mono; unconnected inputs still
  // occupy their channel in the output.
  class ChannelMergerNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'ChannelMergerNode');
      const inherited = readNodeOptions(options, 'ChannelMergerNode');
      const suppliedCount = options?.numberOfInputs;
      const count = suppliedCount === undefined ? 6 : ulong(suppliedCount);
      if (!count || count > MAX_CHANNELS) throw new DE('Unsupported number of merger inputs.', 'IndexSizeError');
      super(secret, context, {
        numberOfInputs: count, channelCount: 1, mode: 'explicit', fixedMode: true,
        checkCount(value) {
          if (value !== 1) throw new DE('A merger input has one channel.', 'InvalidStateError');
        },
      });
      const n = nodeOf(this);
      applyNodeOptions(this, inherited);
      n.process = (f0, q) => Array.from({length: count}, (_, port) => input(n, q, f0, port)?.[0] || new F32(QUANTUM));
    }
  }
  webidl(ChannelMergerNode, 'ChannelMergerNode');

  // ---- Scheduled sources ------------------------------------------------------
  class AudioScheduledSourceNode extends AudioNode {
    constructor(token, ctx, init) {
      if (token !== secret) throw illegal();
      super(secret, ctx, { numberOfInputs: 0, start: -1, stop: Infinity, ended: false, ...init });
    }
    start(when = 0) { startSource(this, 'AudioScheduledSourceNode', when); }
    stop(when = 0) {
      const n = nodeOf(this);
      const t = finite(when, 'execute', "stop' on 'AudioScheduledSourceNode", 'double');
      if (n.start < 0) {
        throw new DE("Failed to execute 'stop' on 'AudioScheduledSourceNode': cannot call stop without calling start first.", 'InvalidStateError');
      }
      if (t < 0) throw new RangeErrorC(`Failed to execute 'stop' on 'AudioScheduledSourceNode': The stop time provided (${t}) is less than the minimum bound (0).`);
      n.stop = t;
      ctxOf(n.ctx).scheduleEnds();
    }
  }
  handler(AudioScheduledSourceNode, 'ended');
  webidl(AudioScheduledSourceNode, 'AudioScheduledSourceNode');
  function startSource(self, iface, when) {
    const n = nodeOf(self);
    const t = finite(when, 'execute', `start' on '${iface}`, 'double');
    if (n.start >= 0) {
      throw new DE(`Failed to execute 'start' on '${iface}': cannot call start more than once.`, 'InvalidStateError');
    }
    if (t < 0) throw new RangeErrorC(`Failed to execute 'start' on '${iface}': The start time provided (${t}) is less than the minimum bound (0).`);
    n.start = t;
    const c = ctxOf(n.ctx);
    c.sources.add(n);
    c.scheduleEnds();
    return n;
  }
  // Marks a source ended at the end of the quantum it stopped in.
  function end(n) {
    if (n.ended) return;
    n.ended = true;
    ctxOf(n.ctx).endedQueue.push(n.self);
  }

  // A source's [first, last) active frames within the quantum at f0.
  function activeRange(n, rate, f0) {
    if (n.start < 0 || n.ended) return null;
    const first = max(f0, ceil(n.start * rate - 1e-9));
    const stopAt = n.stop === Infinity ? Infinity : max(ceil(n.stop * rate - 1e-9), ceil(n.start * rate - 1e-9));
    const last = min(f0 + QUANTUM, stopAt);
    if (stopAt <= f0) { end(n); return null; }
    if (first >= f0 + QUANTUM || first >= last) return null;
    n.processingFrame = f0;
    return [first - f0, last - f0, stopAt <= f0 + QUANTUM];
  }

  class AudioBufferSourceNode extends AudioScheduledSourceNode {
    constructor(context, options) {
      contextArg(context, 'AudioBufferSourceNode');
      super(secret, context, { buffer: null, loop: false, loopStart: 0, loopEnd: 0, pos: undefined, played: 0 });
      const n = nodeOf(this);
      n.playbackRate = nodeParam(n, 1, -F32_MAX, F32_MAX, 'k-rate', true);
      n.detune = nodeParam(n, 0, -F32_MAX, F32_MAX, 'k-rate', true);
      const o = nodeOptions(n, options, 'AudioBufferSourceNode');
      if (o.buffer !== undefined && o.buffer !== null) this.buffer = o.buffer;
      if (o.loop !== undefined) n.loop = !!o.loop;
      if (o.loopStart !== undefined) n.loopStart = finite(o.loopStart, 'construct', 'AudioBufferSourceNode', 'double');
      if (o.loopEnd !== undefined) n.loopEnd = finite(o.loopEnd, 'construct', 'AudioBufferSourceNode', 'double');
      if (o.playbackRate !== undefined) params.get(n.playbackRate).value = Math.fround(finite(o.playbackRate, 'construct', 'AudioBufferSourceNode'));
      if (o.detune !== undefined) params.get(n.detune).value = Math.fround(finite(o.detune, 'construct', 'AudioBufferSourceNode'));
      n.process = (f0, q) => processBufferSource(n, f0, q);
    }
    get buffer() { return nodeOf(this).buffer; }
    set buffer(v) {
      const n = nodeOf(this);
      if (v !== null && !buffers.has(v)) {
        throw new TypeErrorC("Failed to set the 'buffer' property on 'AudioBufferSourceNode': The provided value is not of type 'AudioBuffer'.");
      }
      if (v !== null && n.buffer !== null) {
        throw new DE("Failed to set the 'buffer' property on 'AudioBufferSourceNode': Cannot set buffer to non-null after it has been already been set to a non-null buffer", 'InvalidStateError');
      }
      n.buffer = v;
      // A buffer set after start() is acquired now.
      if (v && n.start >= 0) n.data = acquire(v);
      ctxOf(n.ctx).scheduleEnds();
    }
    get playbackRate() { return nodeOf(this).playbackRate; }
    get detune() { return nodeOf(this).detune; }
    get loop() { return nodeOf(this).loop; }
    set loop(v) { nodeOf(this).loop = !!v; ctxOf(nodeOf(this).ctx).scheduleEnds(); }
    get loopStart() { return nodeOf(this).loopStart; }
    set loopStart(v) { nodeOf(this).loopStart = finite(v, 'set the \'loopStart\' property on', 'AudioBufferSourceNode', 'double'); }
    get loopEnd() { return nodeOf(this).loopEnd; }
    set loopEnd(v) { nodeOf(this).loopEnd = finite(v, 'set the \'loopEnd\' property on', 'AudioBufferSourceNode', 'double'); }
    start(when = 0, offset = undefined, duration = undefined) {
      const iface = 'AudioBufferSourceNode';
      const off = offset === undefined ? 0 : finite(offset, 'execute', `start' on '${iface}`, 'double');
      const dur = duration === undefined ? Infinity : finite(duration, 'execute', `start' on '${iface}`, 'double');
      if (off < 0) throw new RangeErrorC(`Failed to execute 'start' on '${iface}': The offset provided (${off}) is less than the minimum bound (0).`);
      if (dur < 0) throw new RangeErrorC(`Failed to execute 'start' on '${iface}': The duration provided (${dur}) is less than the minimum bound (0).`);
      const n = startSource(this, iface, when);
      n.offset = off;
      n.duration = dur;
      // HTML "acquire the content": later writes to the buffer are not heard.
      if (n.buffer) n.data = acquire(n.buffer);
    }
  }
  webidl(AudioBufferSourceNode, 'AudioBufferSourceNode');
  function acquire(buffer) {
    const b = buffers.get(buffer);
    return { rate: b.rate, length: b.length, channels: b.data.map((c) => c.slice()) };
  }
  function processBufferSource(n, f0, q) {
    const c = ctxOf(n.ctx);
    const rate = c.rate;
    const data = n.data;
    const range = activeRange(n, rate, f0);
    if (!range || !data) {
      if (range && range[2]) end(n);
      return silentBus();
    }
    const [from, to, stopping] = range;
    const out = [];
    for (let ch = 0; ch < data.channels.length; ch++) out.push(new F32(QUANTUM));
    const pr = paramValues(params.get(n.playbackRate), f0, rate, q)[0];
    const dt = paramValues(params.get(n.detune), f0, rate, q)[0];
    const step = pr * pow(2, dt / 1200) * data.rate / rate;
    const len = data.length;
    let loopStart = 0, loopEnd = len;
    if (n.loop && n.loopStart >= 0 && n.loopEnd > 0 && n.loopStart < n.loopEnd) {
      loopStart = n.loopStart * data.rate;
      loopEnd = min(n.loopEnd * data.rate, len);
    }
    if (n.pos === undefined) {
      // The start time between frames: the first frame plays from the
      // matching point of the buffer.
      const startAt = n.start * rate;
      n.pos = n.offset * data.rate + (f0 + from - startAt) * step;
      if (n.loop && n.offset * data.rate >= loopEnd) n.pos = loopStart;
      n.played = 0;
    }
    const maxPlayed = n.duration * data.rate;
    let pos = n.pos;
    let i = from;
    for (; i < to; i++) {
      if (n.loop) {
        const span = loopEnd - loopStart;
        if (span > 0) {
          while (pos >= loopEnd) pos -= span;
          while (pos < loopStart && step < 0) pos += span;
        }
      } else if (pos >= len || pos < 0) {
        break;
      }
      if (n.played >= maxPlayed) break;
      const k = floor(pos);
      const frac = pos - k;
      for (let ch = 0; ch < out.length; ch++) {
        const src = data.channels[ch];
        const a = src[k] || 0;
        let next = k + 1;
        if (n.loop && next >= loopEnd) next = floor(loopStart);
        const b = frac === 0 ? 0 : src[next] || 0;
        out[ch][i] = a + (b - a) * frac;
      }
      pos += step;
      n.played += abs(step);
    }
    n.pos = pos;
    if (i < to || stopping) end(n);
    return out;
  }

  class OscillatorNode extends AudioScheduledSourceNode {
    constructor(context, options) {
      contextArg(context, 'OscillatorNode');
      super(secret, context, { type: 'sine', phase: 0 });
      const n = nodeOf(this);
      const nyquist = context.sampleRate / 2;
      n.frequency = nodeParam(n, 440, -nyquist, nyquist);
      n.detune = nodeParam(n, 0, -153600, 153600);
      const o = nodeOptions(n, options, 'OscillatorNode');
      if (o.type !== undefined) {
        if (o.type === 'custom') throw new DE("Failed to construct 'OscillatorNode': A PeriodicWave must be specified if the type is set to \"custom\"", 'InvalidStateError');
        this.type = o.type;
      }
      if (o.frequency !== undefined) params.get(n.frequency).value = Math.fround(finite(o.frequency, 'construct', 'OscillatorNode'));
      if (o.detune !== undefined) params.get(n.detune).value = Math.fround(finite(o.detune, 'construct', 'OscillatorNode'));
      n.process = (f0, q) => {
        const rate = ctxOf(context).rate;
        const range = activeRange(n, rate, f0);
        if (!range) return silentBus();
        const [from, to, stopping] = range;
        const out = new F32(QUANTUM);
        const fr = paramValues(params.get(n.frequency), f0, rate, q);
        const dt = paramValues(params.get(n.detune), f0, rate, q);
        let phase = n.phase;
        for (let i = from; i < to; i++) {
          const f = fr[i] * pow(2, dt[i] / 1200);
          out[i] = wave(n.type, phase);
          phase += f / rate;
          phase -= floor(phase);
        }
        n.phase = phase;
        if (stopping) end(n);
        return [out];
      };
    }
    get type() { return nodeOf(this).type; }
    set type(v) {
      const t = `${v}`;
      if (t === 'custom') {
        throw new DE("Failed to set the 'type' property on 'OscillatorNode': 'type' cannot be set directly to 'custom'.  Use setPeriodicWave() to create a custom Oscillator type.", 'InvalidStateError');
      }
      if (t === 'sine' || t === 'square' || t === 'sawtooth' || t === 'triangle') nodeOf(this).type = t;
    }
    get frequency() { return nodeOf(this).frequency; }
    get detune() { return nodeOf(this).detune; }
  }
  webidl(OscillatorNode, 'OscillatorNode');
  // One period over phase [0, 1), each starting at 0 like Chrome's.
  function wave(type, phase) {
    switch (type) {
      case 'square': return phase < 0.5 ? 1 : -1;
      case 'sawtooth': return phase < 0.5 ? 2 * phase : 2 * phase - 2;
      case 'triangle': return phase < 0.25 ? 4 * phase : phase < 0.75 ? 2 - 4 * phase : 4 * phase - 4;
      default: return sin(2 * PI * phase);
    }
  }

  class ConstantSourceNode extends AudioScheduledSourceNode {
    constructor(context, options) {
      contextArg(context, 'ConstantSourceNode');
      super(secret, context, {});
      const n = nodeOf(this);
      n.offset = nodeParam(n, 1);
      const o = nodeOptions(n, options, 'ConstantSourceNode');
      if (o.offset !== undefined) params.get(n.offset).value = Math.fround(finite(o.offset, 'construct', 'ConstantSourceNode'));
      n.process = (f0, q) => {
        const rate = ctxOf(context).rate;
        const range = activeRange(n, rate, f0);
        if (!range) return silentBus();
        const [from, to, stopping] = range;
        const v = paramValues(params.get(n.offset), f0, rate, q);
        const out = new F32(QUANTUM);
        for (let i = from; i < to; i++) out[i] = v[i];
        if (stopping) end(n);
        return [out];
      };
    }
    get offset() { return nodeOf(this).offset; }
  }
  webidl(ConstantSourceNode, 'ConstantSourceNode');

  // ---- Contexts ------------------------------------------------------------------
  const contexts = new WeakMap();
  function ctxOf(self) {
    const c = contexts.get(self);
    if (!c) throw invocation();
    return c;
  }
  class BaseAudioContext extends ET {
    constructor(token, rate, channels, offline) {
      if (token !== secret) throw illegal();
      super();
      const self = this;
      const c = {
        self, rate, offline, state: 'suspended', frames: 0, sources: new Set(), endedQueue: [],
        time() { return c.currentFrames() / c.rate; },
        currentFrames() { return c.frames; },
        scheduleEnds() {},
      };
      contexts.set(this, c);
      c.destination = new AudioDestinationNode(secret, this, channels, offline);
    }
    get sampleRate() { return ctxOf(this).rate; }
    get currentTime() { return ctxOf(this).time(); }
    get destination() { return ctxOf(this).destination; }
    get listener() { return contextListener(this); }
    get state() { return ctxOf(this).state; }
    createBuffer(numberOfChannels, length, sampleRate) {
      ctxOf(this);
      if (arguments.length < 3) throw new TypeErrorC(`Failed to execute 'createBuffer' on 'BaseAudioContext': 3 arguments required, but only ${arguments.length} present.`);
      const channels = ulong(numberOfChannels), frames = ulong(length);
      const rate = finite(sampleRate, 'execute', "createBuffer' on 'BaseAudioContext");
      checkBuffer('execute', "createBuffer' on 'BaseAudioContext", channels, frames, rate);
      const data = [];
      for (let i = 0; i < channels; i++) data.push(new F32(frames));
      return makeBuffer(rate, data);
    }
    createBufferSource() { return new AudioBufferSourceNode(this); }
    createConvolver() { return new ConvolverNode(this); }
    createGain() { return new GainNode(this); }
    createAnalyser() { return new AnalyserNode(this); }
    createDelay(maxDelayTime = 1) { return new DelayNode(this, {maxDelayTime}); }
    createPanner() { return new PannerNode(this); }
    createStereoPanner() { return new StereoPannerNode(this); }
    createBiquadFilter() { return new BiquadFilterNode(this); }
    createIIRFilter(feedforward, feedback) {
      ctxOf(this);
      if (arguments.length < 2) throw new TypeErrorC('Two coefficient sequences are required.');
      // Positional arguments are converted left to right; the IIR options
      // dictionary instead enumerates feedback before feedforward.
      const b = coefficients(feedforward);
      const a = coefficients(feedback);
      return new IIRFilterNode(this, {feedforward: b, feedback: a}, secret);
    }
    createChannelSplitter(numberOfOutputs = 6) { return new ChannelSplitterNode(this, {numberOfOutputs}); }
    createChannelMerger(numberOfInputs = 6) { return new ChannelMergerNode(this, {numberOfInputs}); }
    createOscillator() { return new OscillatorNode(this); }
    createConstantSource() { return new ConstantSourceNode(this); }
    decodeAudioData(audioData, successCallback, errorCallback) {
      const c = ctxOf(this);
      if (arguments.length < 1) {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'decodeAudioData' on 'BaseAudioContext': 1 argument required, but only 0 present."));
      }
      if (!(audioData instanceof ArrayBufferC) && Object.prototype.toString.call(audioData) !== '[object ArrayBuffer]') {
        return PromiseC.reject(new TypeErrorC("Failed to execute 'decodeAudioData' on 'BaseAudioContext': parameter 1 is not of type 'ArrayBuffer'."));
      }
      const success = typeof successCallback === 'function' ? successCallback : null;
      const failure = typeof errorCallback === 'function' ? errorCallback : null;
      if (detachedOf ? apply(detachedOf, audioData, []) : false) {
        const error = new DE("Failed to execute 'decodeAudioData' on 'BaseAudioContext': Cannot decode detached ArrayBuffer", 'DataCloneError');
        if (failure) queueMicrotaskC(() => failure(error));
        return PromiseC.reject(error);
      }
      // The buffer is detached; its bytes go to the decoder.
      let bytes = audioData;
      if (transferOf) bytes = apply(transferOf, audioData, []);
      else if (structuredCloneC) bytes = structuredCloneC(audioData, { transfer: [audioData] });
      return new PromiseC((resolve, reject) => {
        decode(bytes, c.rate, (error, result) => {
          if (error !== null && error !== undefined) {
            const e = new DE('Unable to decode audio data', 'EncodingError');
            if (failure) { try { failure(e); } catch (x) { reportErrorC(x); } }
            reject(e);
            return;
          }
          const buffer = makeBuffer(result[0], result.slice(1));
          if (success) { try { success(buffer); } catch (x) { reportErrorC(x); } }
          resolve(buffer);
        });
      });
    }
  }
  handler(BaseAudioContext, 'statechange');
  webidl(BaseAudioContext, 'BaseAudioContext');
  function setState(c, state) {
    if (c.state === state) return;
    c.state = state;
    fire(c.self, 'statechange');
  }
  function flushEnded(c) {
    const ended = c.endedQueue;
    c.endedQueue = [];
    for (const node of ended) {
      c.sources.delete(nodes.get(node));
      fire(node, 'ended');
    }
  }
  function contextOptions(options, iface) {
    if (options === undefined || options === null) return {};
    if (typeof options !== 'object' && typeof options !== 'function') {
      throw new TypeErrorC(`Failed to construct '${iface}': The provided value is not of type '${iface}Options'.`);
    }
    return options;
  }

  // A real-time context: no output device, a clock while running.
  class AudioContext extends BaseAudioContext {
    constructor(options) {
      const o = contextOptions(options, 'AudioContext');
      let rate = 48000;
      if (o.sampleRate !== undefined) {
        rate = finite(o.sampleRate, 'construct', 'AudioContext');
        if (!(rate >= MIN_RATE && rate <= MAX_RATE)) {
          throw new DE(`Failed to construct 'AudioContext': The hardware sample rate provided (${rate}) is outside the range [${MIN_RATE}, ${MAX_RATE}].`, 'NotSupportedError');
        }
      }
      if (o.latencyHint !== undefined && typeof o.latencyHint === 'string'
        && !['interactive', 'balanced', 'playback'].includes(o.latencyHint)) {
        throw new TypeErrorC(`Failed to construct 'AudioContext': Failed to read the 'latencyHint' property from 'AudioContextOptions': The provided value '${o.latencyHint}' is not a valid enum value of type AudioContextLatencyCategory.`);
      }
      super(secret, rate, 2, false);
      const c = ctxOf(this);
      c.started = 0; // document time the clock (re)started at
      c.timer = undefined;
      c.pendingResumes = [];
      c.currentFrames = () => {
        if (c.state !== 'running') return c.frames;
        const elapsed = (now() - c.started) / 1000 * c.rate;
        return c.frames + floor(elapsed / QUANTUM) * QUANTUM;
      };
      c.run = () => {
        if (c.state === 'running' || c.state === 'closed') return;
        c.started = now();
        setState(c, 'running');
        c.scheduleEnds();
      };
      c.halt = (state) => {
        if (c.state === 'running') c.frames = c.currentFrames();
        if (c.timer !== undefined) { clearClock(c.timer); c.timer = undefined; }
        setState(c, state);
      };
      // The sources' ends, on the clock.
      c.scheduleEnds = () => {
        if (c.timer !== undefined) { clearClock(c.timer); c.timer = undefined; }
        if (c.state !== 'running') return;
        const t = c.time();
        let next = Infinity;
        for (const n of c.sources) {
          const at = sourceEnd(n, c.rate);
          if (at <= t) end(n);
          else next = min(next, at);
        }
        if (c.endedQueue.length) flushEnded(c);
        // Not a page timer: a playing source does not keep the page busy.
        if (next !== Infinity) c.timer = timer(max(0, (next - c.time()) * 1000) + 1, () => { c.timer = undefined; c.scheduleEnds(); });
      };
      // Chrome's autoplay policy: a context made after a user gesture runs.
      if (activated()) setTimer(() => c.run(), 0);
    }
    get baseLatency() { ctxOf(this); return 0.01; }
    get outputLatency() { ctxOf(this); return 0; }
    getOutputTimestamp() {
      const c = ctxOf(this);
      return { contextTime: c.time(), performanceTime: c.state === 'running' ? now() : 0 };
    }
    resume() {
      const c = ctxOf(this);
      if (c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'resume' on 'AudioContext': Cannot resume a context that has been closed.", 'InvalidStateError'));
      return new PromiseC((resolve) => {
        // Without a user gesture the promise waits for one (Chrome).
        if (!activated()) { c.pendingResumes.push(resolve); return; }
        setTimer(() => {
          c.run();
          const waiting = c.pendingResumes;
          c.pendingResumes = [];
          for (const r of waiting) r();
          resolve();
        }, 0);
      });
    }
    suspend() {
      const c = ctxOf(this);
      if (c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'suspend' on 'AudioContext': Cannot suspend a context that has been closed.", 'InvalidStateError'));
      return new PromiseC((resolve) => setTimer(() => { if (c.state !== 'closed') c.halt('suspended'); resolve(); }, 0));
    }
    close() {
      const c = ctxOf(this);
      if (c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'close' on 'AudioContext': Cannot close a context that is being closed or has already been closed.", 'InvalidStateError'));
      return new PromiseC((resolve) => setTimer(() => { c.halt('closed'); resolve(); }, 0));
    }
  }
  webidl(AudioContext, 'AudioContext');
  // When a source of a running real-time context stops (context time).
  function sourceEnd(n, rate) {
    if (n.ended || n.start < 0) return Infinity;
    let at = n.stop;
    const b = n.buffer && buffers.get(n.buffer);
    if (b && !n.loop) {
      const pr = params.get(n.playbackRate).value * pow(2, params.get(n.detune).value / 1200);
      if (pr > 0) {
        const remaining = min(n.duration === undefined ? Infinity : n.duration, max(0, b.length / b.rate - (n.offset || 0)));
        at = min(at, n.start + remaining / pr);
      }
    }
    return at;
  }

  // An offline context renders its graph into a buffer.
  class OfflineAudioContext extends BaseAudioContext {
    constructor(numberOfChannels, length, sampleRate) {
      let channels, frames, rate;
      if (arguments.length === 1 && numberOfChannels !== null && typeof numberOfChannels === 'object') {
        const o = numberOfChannels;
        if (o.length === undefined) throw new TypeErrorC("Failed to construct 'OfflineAudioContext': Failed to read the 'length' property from 'OfflineAudioContextOptions': Required member is undefined.");
        if (o.sampleRate === undefined) throw new TypeErrorC("Failed to construct 'OfflineAudioContext': Failed to read the 'sampleRate' property from 'OfflineAudioContextOptions': Required member is undefined.");
        channels = o.numberOfChannels === undefined ? 1 : ulong(o.numberOfChannels);
        frames = ulong(o.length);
        rate = finite(o.sampleRate, 'construct', 'OfflineAudioContext');
      } else {
        if (arguments.length < 3) throw new TypeErrorC(`Failed to construct 'OfflineAudioContext': 3 arguments required, but only ${arguments.length} present.`);
        channels = ulong(numberOfChannels);
        frames = ulong(length);
        rate = finite(sampleRate, 'construct', 'OfflineAudioContext');
      }
      checkBuffer('construct', 'OfflineAudioContext', channels, frames, rate);
      super(secret, rate, channels, true);
      const c = ctxOf(this);
      c.length = frames;
      c.channels = channels;
      c.suspends = new Map();
      c.rendering = false;
    }
    get length() { return ctxOf(this).length; }
    startRendering() {
      const c = ctxOf(this);
      if (c.rendering || c.state === 'closed') {
        return PromiseC.reject(new DE("Failed to execute 'startRendering' on 'OfflineAudioContext': cannot call startRendering more than once", 'InvalidStateError'));
      }
      c.rendering = true;
      c.output = [];
      for (let i = 0; i < c.channels; i++) c.output.push(new F32(c.length));
      return new PromiseC((resolve) => {
        c.finish = resolve;
        setTimer(() => render(c), 0);
      });
    }
    resume() {
      const c = ctxOf(this);
      if (c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'resume' on 'OfflineAudioContext': Cannot resume a context that has been closed.", 'InvalidStateError'));
      if (!c.rendering) return PromiseC.reject(new DE("Failed to execute 'resume' on 'OfflineAudioContext': Cannot resume a context that has not started.", 'InvalidStateError'));
      return new PromiseC((resolve) => setTimer(() => { if (c.state === 'suspended') render(c); resolve(); }, 0));
    }
    suspend(suspendTime) {
      const c = ctxOf(this);
      if (arguments.length < 1) return PromiseC.reject(new TypeErrorC("Failed to execute 'suspend' on 'OfflineAudioContext': 1 argument required, but only 0 present."));
      const t = +suspendTime;
      if (!(t >= 0)) return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': negative suspend time (${t}) is not allowed`, 'InvalidStateError'));
      const frame = floor(t * c.rate / QUANTUM) * QUANTUM;
      if (frame >= c.length) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': cannot schedule a suspend at frame ${frame} (${t} seconds) because it is greater than or equal to the total render duration of ${c.length} frames`, 'InvalidStateError'));
      }
      if (frame < c.frames || (c.rendering && frame === c.frames && c.state === 'running')) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': suspend(${t}) failed to suspend at frame ${frame} because it is earlier than the current frame of ${c.frames}`, 'InvalidStateError'));
      }
      if (c.suspends.has(frame)) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': cannot schedule more than one suspend at frame ${frame} (${t} seconds)`, 'InvalidStateError'));
      }
      return new PromiseC((resolve) => c.suspends.set(frame, resolve));
    }
  }
  handler(OfflineAudioContext, 'complete');
  webidl(OfflineAudioContext, 'OfflineAudioContext');

  // Renders quanta until the end or a suspend point.
  function render(c) {
    setState(c, 'running');
    c.graphEpoch = (c.graphEpoch ?? 0) + 1;
    const dest = nodes.get(c.destination);
    while (c.frames < c.length) {
      const pause = c.suspends.get(c.frames);
      if (pause) {
        c.suspends.delete(c.frames);
        setState(c, 'suspended');
        flushEnded(c);
        pause();
        return;
      }
      const f0 = c.frames;
      const q = f0 / QUANTUM;
      const bus = pull(dest, q, f0);
      const count = min(QUANTUM, c.length - f0);
      for (let ch = 0; ch < c.channels; ch++) c.output[ch].set(count === QUANTUM ? bus[ch] : bus[ch].subarray(0, count), f0);
      // Started sources nothing pulled still play (and end) on time.
      for (const n of c.sources) if (n.q !== q) pull(n, q, f0);
      if (c.analyserNodes) for (const ref of c.analyserNodes) {
        const node = ref.deref();
        if (!node) c.analyserNodes.delete(ref);
        else if (node.q !== q) pull(node, q, f0);
      }
      // Preserve a fed convolver's history even while its output is disconnected.
      if (c.convolverNodes) for (const ref of c.convolverNodes) {
        const n = ref.deref();
        if (!n) c.convolverNodes.delete(ref);
        else if (n.convolve && n.q !== q && (n.inputs.length || n.tail)) pull(n, q, f0);
      }
      // A fed delay keeps the signal it will output even while downstream is
      // disconnected. A weak registration does not keep an unused node alive.
      if (c.delayNodes) for (const ref of c.delayNodes) {
        const n = ref.deref();
        if (!n) c.delayNodes.delete(ref);
        else if (n.q !== q && (n.inputs.length || n.blocks.size)) pull(n, q, f0);
      }
      if (c.delayWriters?.size) {
        for (const n of c.delayWriters) writeDelay(n, q, f0);
        c.delayWriters.clear();
      }
      c.frames += QUANTUM;
    }
    c.frames = c.length;
    const rendered = makeBuffer(c.rate, c.output);
    c.output = null;
    flushEnded(c);
    setState(c, 'closed');
    c.finish(rendered);
    apply(dispatch, c.self, [new OfflineAudioCompletionEvent('complete', { renderedBuffer: rendered })]);
  }

  class OfflineAudioCompletionEvent extends Ev {
    #buffer;
    constructor(type, eventInitDict) {
      if (arguments.length < 2) {
        throw new TypeErrorC(`Failed to construct 'OfflineAudioCompletionEvent': 2 arguments required, but only ${arguments.length} present.`);
      }
      const init = eventInitDict;
      if (init === null || typeof init !== 'object' || !buffers.has(init.renderedBuffer)) {
        throw new TypeErrorC("Failed to construct 'OfflineAudioCompletionEvent': Failed to read the 'renderedBuffer' property from 'OfflineAudioCompletionEventInit': Required member is undefined.");
      }
      super(type, init);
      this.#buffer = init.renderedBuffer;
    }
    get renderedBuffer() { return this.#buffer; }
  }
  webidl(OfflineAudioCompletionEvent, 'OfflineAudioCompletionEvent');

  return {
    localImpulseInfo, BaseAudioContext, AudioContext, OfflineAudioContext, AudioBuffer, AudioParam, AudioNode,
    AudioDestinationNode, AudioScheduledSourceNode, AudioBufferSourceNode, OscillatorNode,
    ConstantSourceNode, GainNode, AnalyserNode, DelayNode, ConvolverNode, AudioListener, PannerNode, StereoPannerNode, BiquadFilterNode, IIRFilterNode, ChannelMergerNode, ChannelSplitterNode, OfflineAudioCompletionEvent,
  };
})
