// Web Audio for one realm, made on first use (webaudio.rs). The script
// evaluates to a function of the engine's natives (decode, activated, a
// timer that does not keep the page busy)
// returning the module object.
//
// The graph renders in context-sized quanta, pulled from the destination: each
// node's output for a quantum is computed once (fan-out shares it), inputs
// are summed after up/down-mixing to the input's computed channel count, and
// AudioParams compute their automation (and connected inputs) per sample
// (a-rate) or per quantum (k-rate). An OfflineAudioContext renders when
// startRendering() runs; a real-time AudioContext has no output device, so
// it only runs its clock and ends its sources on time.
(natives => {
  'use strict';
  const { decode, activated, timer, clearTimer: clearClock, audioWorklet: makeAudioWorklet, secureContext, mediaBlocked } = natives;
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
  const DEFAULT_QUANTUM = 128;
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
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'copyFromChannel' on 'AudioBuffer': 2 arguments required, but only ${arguments.length} present.`);
      checkCopyArray(destination, 'copyFromChannel');
      const c = ulong(channelNumber);
      const offset = ulong(bufferOffset);
      if (c >= b.data.length) {
        throw new DE(`Failed to execute 'copyFromChannel' on 'AudioBuffer': The channelNumber provided (${c}) is outside the range [0, ${b.data.length - 1}].`, 'IndexSizeError');
      }
      const src = b.data[c];
      copySamples(destination, 0, src, offset);
    }
    copyToChannel(source, channelNumber, bufferOffset = 0) {
      const b = bufOf(this);
      if (arguments.length < 2) throw new TypeErrorC(`Failed to execute 'copyToChannel' on 'AudioBuffer': 2 arguments required, but only ${arguments.length} present.`);
      checkCopyArray(source, 'copyToChannel');
      const c = ulong(channelNumber);
      const offset = ulong(bufferOffset);
      if (c >= b.data.length) {
        throw new DE(`Failed to execute 'copyToChannel' on 'AudioBuffer': The channelNumber provided (${c}) is outside the range [0, ${b.data.length - 1}].`, 'IndexSizeError');
      }
      const dst = b.data[c];
      copySamples(dst, offset, source, 0);
    }
  }
  webidl(AudioBuffer, 'AudioBuffer');
  function checkCopyArray(view, method) {
    const prefix = `Failed to execute '${method}' on 'AudioBuffer': `;
    if (apply(arrayKind, view, []) !== 'Float32Array') throw new TypeErrorC(prefix + "parameter 1 is not of type 'Float32Array'.");
    const buffer = apply(arrayBufferOf, view, []);
    try { apply(bufferLength, buffer, []); } catch { throw new TypeErrorC(prefix + 'The provided Float32Array value must not be shared.'); }
    if (resizable && apply(resizable, buffer, [])) throw new TypeErrorC(prefix + 'Expected a fixed-length ArrayBuffer.');
  }
  function copySamples(destination, destinationOffset, source, sourceOffset) {
    const length = min(apply(arrayLength, source, []) - sourceOffset, apply(arrayLength, destination, []) - destinationOffset);
    if (length <= 0) return;
    const byteOffset = apply(arrayByteOffset, source, []) + sourceOffset * F32.BYTES_PER_ELEMENT;
    apply(arraySet, destination, [new F32(apply(arrayBufferOf, source, []), byteOffset, length), destinationOffset]);
  }
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
        events: [], inputs: [], values: null, q: -1,
      });
    }
    get value() {
      const p = paramOf(this);
      const context = ctxOf(p.ctx);
      if (context.offline) return p.renderedValue ?? p.value;
      return p.events.length ? Math.fround(clampParam(p, valueAt(p, context.time()))) : p.value;
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
      const t = automationTime(p, startTime, 'setValueAtTime');
      insertEvent(p, { type: SET, time: t, value: v }, 'setValueAtTime');
      return this;
    }
    linearRampToValueAtTime(value, endTime) {
      const p = paramOf(this);
      const v = finite(value, 'execute', "linearRampToValueAtTime' on 'AudioParam");
      const t = automationTime(p, endTime, 'linearRampToValueAtTime');
      insertEvent(p, ramp(p, { type: LINEAR, time: t, value: v }), 'linearRampToValueAtTime');
      return this;
    }
    exponentialRampToValueAtTime(value, endTime) {
      const p = paramOf(this);
      const v = finite(value, 'execute', "exponentialRampToValueAtTime' on 'AudioParam");
      if (abs(v) < 1.401298464324817e-45) {
        throw new RangeErrorC(`Failed to execute 'exponentialRampToValueAtTime' on 'AudioParam': The float target value provided (${v}) should not be in the range (-1.40130e-45, 1.40130e-45).`);
      }
      const t = automationTime(p, endTime, 'exponentialRampToValueAtTime');
      insertEvent(p, ramp(p, { type: EXPONENTIAL, time: t, value: v }), 'exponentialRampToValueAtTime');
      return this;
    }
    setTargetAtTime(target, startTime, timeConstant) {
      const p = paramOf(this);
      const v = finite(target, 'execute', "setTargetAtTime' on 'AudioParam");
      const t = automationTime(p, startTime, 'setTargetAtTime');
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
      const t = automationTime(p, startTime, 'setValueCurveAtTime');
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
      const t = automationTime(p, cancelTime, 'cancelScheduledValues');
      p.events = p.events.filter(e => e.time < t && !(e.type === CURVE && t < eventEnd(e)));
      return this;
    }
    cancelAndHoldAtTime(cancelTime) {
      const p = paramOf(this);
      const t = automationTime(p, cancelTime, 'cancelAndHoldAtTime');
      const next = p.events.find((e) => e.time >= t);
      const kept = p.events.filter(e => e.time < t || (e.time === t && e.type === SET));
      const previous = kept.at(-1);
      const inCurve = previous?.type === CURVE && t < eventEnd(previous);
      const inRamp = !inCurve && next && (next.type === LINEAR || next.type === EXPONENTIAL);
      const held = fround(inRamp ? valueAt({...p, events: [...kept, next]}, t) : valueAt({...p, events: kept}, t));
      if (inCurve) kept[kept.length - 1] = {...previous, cutoff: t, value: held};
      // A ramp under way ends where it is held.
      if (inRamp) kept.push({ type: next.type, time: t, value: held, t0: next.t0, v0: next.v0 });
      else if (previous?.type === TARGET) kept.push({ type: SET, time: t, value: held });
      p.events = kept;
      return this;
    }
  }
  webidl(AudioParam, 'AudioParam');

  function eventEnd(e) { return e.type === CURVE ? e.cutoff ?? (e.time + e.duration) : e.time; }
  function checkValueAtTime(p, t) {
    if (p.events.some(e => e.type === CURVE && e.time <= t && t < eventEnd(e))) {
      throw new DE('The current time overlaps an automation curve.', 'NotSupportedError');
    }
  }
  function setParamValue(p, value, t) {
    // Setting value is equivalent to inserting a setValueAtTime at currentTime.
    checkValueAtTime(p, t);
    value = Math.fround(clampParam(p, value));
    const at = p.events.findIndex(e => e.type === SET && e.time === t);
    if (at >= 0) p.events[at].value = value;
    else if (p.events.length) insertEvent(p, {type: SET, time: t, value});
    p.value = value;
    p.renderedValue = value;
  }
  function nodeParam(n, ...options) {
    const param = new AudioParam(secret, n.ctx, ...options);
    (n.audioParams ??= []).push(params.get(param));
    return param;
  }
  function automationTime(param, v, method) {
    const t = finite(v, 'execute', `${method}' on 'AudioParam`, 'double');
    if (t < 0) throw new RangeErrorC(`Failed to execute '${method}' on 'AudioParam': Time must be a finite non-negative number: ${t}`);
    return max(t, ctxOf(param.ctx).time());
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
      const oEnd = eventEnd(o);
      const eEnd = eventEnd(e);
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
      case CURVE: return t >= eventEnd(e) ? e.value : curveAt(e, t);
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
        t0 = eventEnd(prev);
        v0 = endValue(p, i, max(t0, prev.time));
      } else {
        t0 = next.t0 || 0;
        v0 = next.v0 === undefined ? p.value : next.v0;
      }
      const t1 = next.time, v1 = next.value;
      if (t < t0 && i >= 0 && events[i].type === CURVE) return curveAt(events[i], t);
      if (t <= t0) return v0;
      if (t1 <= t0) return v1;
      const f = (t - t0) / (t1 - t0);
      if (next.type === LINEAR) return v0 + fround((v1 - v0) * f);
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
    const out = (p.values ??= new F32(ctxOf(p.ctx).quantum));
    if (!p.events.length) out.fill(p.value);
    else if (p.rate === 'k-rate') out.fill(clampParam(p, valueAt(p, f0 / rate)));
    else for (let i = 0; i < out.length; i++) out[i] = clampParam(p, valueAt(p, (f0 + i) / rate));
    p.renderedValue = out[out.length - 1];
    if (p.inputs.length) {
      const mono = new F32(out.length);
      for (const { node, output } of p.inputs) mix(pull(node, q, f0, output), [mono], 'speakers', 1);
      if (p.rate === 'k-rate') { const v = mono[0]; for (let i = 0; i < out.length; i++) out[i] += v; }
      else for (let i = 0; i < out.length; i++) out[i] += mono[i];
    }
    for (let i = 0; i < out.length; i++) {
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
    const add = (d, s, g = 1) => { for (let i = 0; i < d.length; i++) d[i] += s[i] * g; };
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
  const silentBus = size => [new F32(size)];

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
    // A worklet node has a bus of its own per output port.
    if (n.outputPorts) return n.outputPorts[output] ?? silentBus(n.blockSize);
    // A splitter exposes each discrete input channel as its own mono output port.
    return n.splitChannels ? (bus[output] ? [bus[output]] : silentBus(n.blockSize)) : bus;
  }
  function renderNode(n, q, f0) {
    if (n.q === q) return n.out;
    if (n.busy) return silentBus(n.blockSize);
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
    for (let c = 0; c < channels; c++) out.push(new F32(n.blockSize));
    for (const b of buses) mix(b, out, n.interp);
    return out;
  }
  class AudioNode extends ET {
    constructor(token, ctx, init) {
      if (token !== secret) throw illegal();
      super();
      nodes.set(this, {
        self: this, ctx, blockSize: ctxOf(ctx).quantum, inputs: [], outs: [], q: -1, out: null, busy: false,
        numberOfInputs: 1, numberOfOutputs: 1, channelCount: 2, mode: 'max', interp: 'speakers',
        process: () => silentBus(ctxOf(ctx).quantum),
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
        throw new DE(`Failed to set the 'channelCountMode' property on 'AudioNode': ChannelCountMode cannot be changed from '${n.mode}' to '${mode}'`, n.modeError ?? 'InvalidStateError');
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
          if (count === 0) throw new DE('Destination channel count must be positive.', 'NotSupportedError');
          if (count > this.maxChannelCount) {
            throw new DE(`Failed to set the 'channelCount' property on 'AudioNode': The channel count provided (${count}) is outside the range [1, ${this.maxChannelCount}].`, 'IndexSizeError');
          }
          this.channelCount = count;
        },
      });
      const n = nodeOf(this);
      n.process = (f0, q) => {
        const bus = input(n, q, f0);
        const out = [];
        for (let c = 0; c < n.channelCount; c++) out.push(new F32(n.blockSize));
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
        if (!bus) return silentBus(n.blockSize);
        const g = paramValues(params.get(gain), f0, ctxOf(context).rate, q);
        for (const ch of bus) for (let i = 0; i < n.blockSize; i++) ch[i] *= g[i];
        return bus;
      };
    }
    get gain() { return nodeOf(this).gain; }
  }
  webidl(GainNode, 'GainNode');

  const compressorParams = {attack: [0.003, 0, 1], knee: [30, 0, 40], ratio: [12, 1, 20], release: [0.25, 0, 1], threshold: [-24, -100, 0]};
  const compressors = new WeakMap();
  function compressorOf(self) {
    const state = compressors.get(self);
    if (!state) throw invocation();
    return state;
  }
  function renderCompressor(node, state, frame, quantum) {
    const rate = ctxOf(node.ctx).rate;
    const bus = input(node, quantum, frame);
    const setting = name => paramValues(params.get(state[name]), frame, rate, quantum)[0];
    const threshold = setting('threshold'), knee = setting('knee'), ratio = setting('ratio');
    const attackSeconds = setting('attack'), releaseSeconds = setting('release');
    const attackStep = attackSeconds ? 10 / (rate * attackSeconds) : Infinity;
    const releaseStep = releaseSeconds ? 10 / (rate * releaseSeconds) : Infinity;
    const entry = pow(10, threshold / 20), exit = pow(10, (threshold + knee) / 20);
    const span = exit - entry;
    const bend = (distance, stiffness) => entry - Math.expm1(-stiffness * distance) / stiffness;
    let low = 0, high = 64 / (span || 1);
    if (span && ratio !== 1) for (let iteration = 0; iteration < 24; iteration++) {
      const stiffness = (low + high) / 2;
      if (exit * exp(-stiffness * span) / bend(span, stiffness) > 1 / ratio) low = stiffness;
      else high = stiffness;
    }
    const upper = span ? bend(span, high) : entry;
    const curve = level => {
      if (level <= entry || ratio === 1) return 0;
      const shaped = level < exit ? bend(level - entry, high) : upper * pow(level / exit, 1 / ratio);
      return 20 * Math.log10(shaped / level);
    };
    const makeup = pow(10, -0.6 * curve(1) / 20), meterRelease = exp(-1 / (rate * 0.325));
    const width = bus?.length ?? (node.mode === 'explicit' ? node.channelCount : 1);
    const output = Array.from({length: max(width, state.stereoTail ? 2 : 1)}, () => new F32(node.blockSize));
    for (let index = 0; index < node.blockSize; index++) {
      const left = bus?.[0][index] ?? 0, right = bus?.[1]?.[index] ?? 0;
      const peak = max(abs(left), abs(right));
      const target = curve(peak);
      state.envelope = max(state.envelope - attackStep, min(target, state.envelope + releaseStep));
      state.meter = min(state.envelope, state.envelope + meterRelease * (state.meter - state.envelope));
      const gain = makeup * pow(10, state.envelope / 20), pos = state.position;
      output[0][index] = state.left[pos] * gain;
      if (output[1]) output[1][index] = state.right[pos] * gain;
      state.left[pos] = left;
      state.right[pos] = width === 1 && node.interp === 'speakers' ? left : right;
      state.position = (pos + 1) % state.left.length;
    }
    state.stereoTail = width === 2 ? state.left.length : max(0, state.stereoTail - node.blockSize);
    return output;
  }
  class DynamicsCompressorNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'DynamicsCompressorNode');
      const settings = readNodeOptions(options, 'DynamicsCompressorNode');
      const values = {};
      for (const name of Object.keys(compressorParams)) {
        const value = options?.[name];
        if (value !== undefined) values[name] = finite(fround(value), 'construct', 'DynamicsCompressorNode');
      }
      super(secret, context, {mode: 'clamped-max', disallowMaxMode: true, checkCount(count) {
        if (count !== 1 && count !== 2) throw new DE('Compressor input requires one or two channels.', 'NotSupportedError');
        this.channelCount = count;
      }});
      applyNodeOptions(this, settings);
      const node = nodeOf(this), delay = max(1, floor(0.006 * ctxOf(context).rate));
      const state = {left: new F32(delay), right: new F32(delay), envelope: 0, meter: 0, position: 0, stereoTail: 0};
      compressors.set(this, state);
      for (const [name, bounds] of Object.entries(compressorParams)) {
        const param = nodeParam(node, ...bounds, 'k-rate', true);
        state[name] = param;
        if (values[name] !== undefined) setParamValue(params.get(param), values[name], 0);
      }
      node.process = (frame, quantum) => renderCompressor(node, state, frame, quantum);
    }
    get attack() { return compressorOf(this).attack; }
    get knee() { return compressorOf(this).knee; }
    get ratio() { return compressorOf(this).ratio; }
    get release() { return compressorOf(this).release; }
    get threshold() { return compressorOf(this).threshold; }
    get reduction() { return fround(compressorOf(this).meter); }
  }
  webidl(DynamicsCompressorNode, 'DynamicsCompressorNode');

  function shapeSample(curve, sample) {
    const last = curve.length - 1;
    if (sample <= -1) return curve[0];
    if (sample >= 1) return curve[last];
    const index = (sample + 1) * (last * 0.5);
    const lower = floor(index), fraction = index - lower;
    return (1 - fraction) * curve[lower] + fraction * curve[min(lower + 1, last)];
  }

  const shapingKernels = new Map();
  function shapingKernel(factor) {
    if (shapingKernels.has(factor)) return shapingKernels.get(factor);
    const radius = 32 * factor;
    const taps = Array.from({length: 2 * radius + 1}, (_, index) => {
      const distance = (index - radius) / factor;
      const phase = PI * index / radius;
      const window = 0.42 - 0.5 * cos(phase) + 0.08 * cos(2 * phase);
      return window * (distance === 0 ? 1 : sin(PI * distance) / (PI * distance)) / factor;
    });
    const sum = taps.reduce((total, tap) => total + tap, 0);
    const normalized = taps.map(tap => tap / sum);
    shapingKernels.set(factor, normalized);
    return normalized;
  }

  function oversampleChannel(samples, curve, factor, history) {
    const taps = shapingKernel(factor);
    const past = history.input, shaped = history.shaped;
    for (let frame = 0; frame < samples.length; frame++) {
      past[history.write] = samples[frame];
      for (let phase = 0; phase < factor; phase++) {
        let interpolated = 0, read = history.write;
        for (let tap = phase; tap < taps.length; tap += factor) {
          interpolated += past[read] * taps[tap];
          if (--read < 0) read = past.length - 1;
        }
        shaped[history.highWrite] = shapeSample(curve, factor * interpolated);
        if (phase === 0) {
          let filtered = 0, highRead = history.highWrite;
          for (let tap = 0; tap < taps.length; tap++) {
            filtered += taps[tap] * shaped[highRead];
            if (--highRead < 0) highRead = shaped.length - 1;
          }
          samples[frame] = filtered;
        }
        history.highWrite = (history.highWrite + 1) % shaped.length;
      }
      history.write = (history.write + 1) % past.length;
    }
  }

  const waveShapers = new WeakMap();
  function shaperOf(self) {
    const state = waveShapers.get(self);
    if (!state) throw invocation();
    return state;
  }
  function assignShaperCurve(state, curve) {
    if (curve != null) {
      if (apply(arrayKind, curve, []) !== 'Float32Array') throw new TypeErrorC('Expected a Float32Array.');
      const buffer = apply(arrayBufferOf, curve, []);
      apply(bufferLength, buffer, []);
      if (resizable && apply(resizable, buffer, [])) throw new TypeErrorC('Expected a fixed-length ArrayBuffer.');
      if (state.assigned || apply(arrayLength, curve, []) < 2) throw new DE('Cannot assign this shaping curve.', 'InvalidStateError');
      state.curve = new F32(curve);
      state.assigned = true;
    } else {
      state.curve = null;
    }
    state.history = [];
  }
  const oversamplingModes = ['none', '2x', '4x'];
  class WaveShaperNode extends AudioNode {
    constructor(context, options) {
      contextArg(context, 'WaveShaperNode');
      const inherited = readNodeOptions(options, 'WaveShaperNode');
      const supplied = options?.curve;
      const curve = supplied === undefined ? null : F32.from([...supplied], value => finite(fround(value), 'construct', 'WaveShaperNode'));
      const setting = options?.oversample, mode = setting === undefined ? 'none' : `${setting}`;
      if (!oversamplingModes.includes(mode)) throw new TypeErrorC('Invalid OverSampleType.');
      super(secret, context, {});
      applyNodeOptions(this, inherited);
      const state = {curve: null, assigned: false, mode, history: []};
      waveShapers.set(this, state);
      if (curve !== null) assignShaperCurve(state, curve);
      const node = nodeOf(this);
      node.process = (frame, quantum) => {
        const bus = input(node, quantum, frame) ?? Array.from({length: node.mode === 'explicit' ? node.channelCount : 1}, () => new F32(node.blockSize));
        if (!state.curve) return bus;
        state.history.length = bus.length;
        for (let channel = 0; channel < bus.length; channel++) {
          const samples = bus[channel];
          if (state.mode === 'none') {
            for (let index = 0; index < samples.length; index++) samples[index] = shapeSample(state.curve, samples[index]);
          } else {
            const factor = state.mode === '2x' ? 2 : 4;
            const length = shapingKernel(factor).length;
            const history = state.history[channel] ??= {input: new F32(ceil(length / factor)), shaped: new F32(length), write: 0, highWrite: 0};
            oversampleChannel(samples, state.curve, factor, history);
          }
        }
        return bus;
      };
    }
    get curve() { const curve = shaperOf(this).curve; return curve === null ? null : new F32(curve); }
    set curve(value) { assignShaperCurve(shaperOf(this), value); }
    get oversample() { return shaperOf(this).mode; }
    set oversample(value) {
      const state = shaperOf(this), mode = `${value}`;
      if (oversamplingModes.includes(mode) && mode !== state.mode) {
        state.mode = mode;
        state.history = [];
      }
    }
  }
  webidl(WaveShaperNode, 'WaveShaperNode');

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
    transformBitReversed(real, imaginary);
    for (let bin = 0; bin < size / 2; bin++) {
      state.spectrum[bin] = state.smoothing * state.spectrum[bin] + (1 - state.smoothing) * hypot(real[bin], imaginary[bin]) / size;
    }
    state.dirty = false;
  }
  function transformBitReversed(real, imaginary) {
    const size = real.length;
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
      const analysed = ctxOf(context);
      (analysed.analyserNodes ??= new Set()).add(new WeakRefC(node));
      // A running real-time context computes samples for it.
      if (analysed.syncPump) analysed.syncPump();
      node.process = (frame, quantum) => {
        const bus = input(node, quantum, frame) ?? silentBus(node.blockSize);
        const mono = new F32(node.blockSize);
        mix(bus, [mono], 'speakers');
        for (let index = max(0, mono.length - state.history.length); index < mono.length; index++) state.history[(state.cursor + index) & 32767] = mono[index];
        state.cursor = (state.cursor + node.blockSize) & 32767;
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
        const left = new F32(n.blockSize), right = new F32(n.blockSize);
        if (bus) {
          const values = paramValues(params.get(pan), f0, ctxOf(context).rate, q);
          for (let i = 0; i < n.blockSize; i++) writeEqualPower(bus, left, right, i, values[i], 1);
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
    const left = new F32(n.blockSize), right = new F32(n.blockSize);
    const bus = input(n, q, f0);
    if (!bus) return [left, right];
    const rate = ctxOf(n.ctx).rate;
    const source = n.spatial, listener = listenerOf(contextListener(n.ctx)).spatial;
    let stride = n.blockSize;
    function values(param) {
      const state = params.get(param);
      if (state.rate === 'a-rate' && (state.events.length || state.inputs.length)) stride = 1;
      return paramValues(state, f0, rate, q);
    }
    const [sx, sy, sz, ox, oy, oz] = pannerNames.map(k => values(source[k]));
    const [lx, ly, lz, fx, fy, fz, ux, uy, uz] = listenerNames.map(k => values(listener[k]));
    // Static and k-rate coordinates have one listener-relative position per quantum.
    for (let i = 0; i < n.blockSize; i += stride) {
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
    const q = floor(frame / n.blockSize), block = n.blocks.get(q % n.slots);
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
    for (let i = 0; i < n.blockSize; i++) {
      const duration = max(cyclic ? n.blockSize : 0, fround(times[i] * ctx.rate));
      const pos = f0 + i - duration, prev = floor(pos);
      positions[i] = pos;
      channels = max(channels, delayBlock(n, prev)?.length ?? 1);
      if (prev !== pos) channels = max(channels, delayBlock(n, prev + 1)?.length ?? 1);
    }
    const out = Array.from({length: channels}, () => new F32(n.blockSize));
    const converted = new Map();
    function source(frame) {
      const bus = delayBlock(n, frame);
      if (!bus || bus.length === channels) return bus;
      if (!converted.has(bus)) {
        const normalized = Array.from({length: channels}, () => new F32(n.blockSize));
        mix(bus, normalized, n.interp);
        converted.set(bus, normalized);
      }
      return converted.get(bus);
    }
    for (let i = 0; i < n.blockSize; i++) {
      const pos = positions[i], a = floor(pos), frac = pos - a;
      const first = source(a), second = frac ? source(a + 1) : null;
      for (let ch = 0; ch < channels; ch++) {
        const v = first ? first[ch][a % n.blockSize] : 0;
        out[ch][i] = frac ? v + frac * ((second ? second[ch][(a + 1) % n.blockSize] : 0) - v) : v;
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
      setParamValue(params.get(n.delayTime), delay, 0);
      // Store only the rendered part of the delay line, bounded by its maximum
      // age. Two extra quanta cover the neighboring sample and the current write.
      n.slots = max(1, ceil(limit * ctxOf(context).rate / n.blockSize)) + 2;
      n.blocks = new Map();
      n.positions = new Float64Array(n.blockSize);
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
        if (!n.convolve) return silentBus(n.blockSize);
        const bus = input(n, q, f0);
        if (!n.activeInput && n.tail === 0) return silentBus(n.blockSize);
        n.processingFrame = f0;
        n.tail = n.activeInput ? n.impulseLength - 1 : max(0, n.tail - n.blockSize);
        return n.convolve(bus ? bus[0] : new F32(n.blockSize), bus?.[1], n.interp === 'speakers');
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
  const arrayByteOffset = getOwnPropertyDescriptor(typedArrayPrototype, 'byteOffset').get;
  const arraySet = typedArrayPrototype.set;
  const bufferLength = getOwnPropertyDescriptor(ArrayBufferC.prototype, 'byteLength').get;
  const resizable = getOwnPropertyDescriptor(ArrayBufferC.prototype, 'resizable')?.get;
  const { sqrt, hypot, atan2 } = Math;

  // Both nodes implement a rational transfer function B(z)/A(z). Keep input
  // and output samples separately: a biquad's coefficients can change at each
  // sample, while its previous samples still belong to the same channel.
  function filterProcess(n, f0, q, update) {
    const bus = input(n, q, f0);
    const channels = bus || silentBus(n.blockSize);
    // A channel disappearing from the input remains audible for its tail.
    // Retire it once its delay line no longer contributes to any output.
    while (n.history.length > channels.length) {
      const last = n.history[n.history.length - 1];
      if (last.x.some(v => v !== 0) || last.y.some(v => v !== 0)) break;
      n.history.pop();
    }
    const count = max(channels.length, n.history.length);
    while (channels.length < count) channels.push(new F32(n.blockSize));
    const size = max(n.a.length, n.b.length);
    while (n.history.length < count) n.history.push({x: new F64(size), y: new F64(size)});
    for (let i = 0; i < n.blockSize; i++) {
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
      n.gain = parameter('gain', 0, -F32_MAX, fround(40 * fround(Math.log10(F32_MAX))));
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
      n.process = (f0, q) => Array.from({length: count}, (_, port) => input(n, q, f0, port)?.[0] || new F32(n.blockSize));
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
    const last = min(f0 + n.blockSize, stopAt);
    if (stopAt <= f0) { end(n); return null; }
    if (first >= f0 + n.blockSize || first >= last) return null;
    n.processingFrame = f0;
    return [first - f0, last - f0, stopAt <= f0 + n.blockSize];
  }

  class AudioBufferSourceNode extends AudioScheduledSourceNode {
    constructor(context, options) {
      contextArg(context, 'AudioBufferSourceNode');
      super(secret, context, { buffer: null, bufferSet: false, loop: false, loopStart: 0, loopEnd: 0, pos: undefined, played: 0 });
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
      if (v !== null && n.bufferSet) {
        throw new DE("Failed to set the 'buffer' property on 'AudioBufferSourceNode': Cannot set buffer to non-null after it has been already been set to a non-null buffer", 'InvalidStateError');
      }
      if (v !== null) n.bufferSet = true;
      n.buffer = v;
      // A buffer set after start() is acquired now.
      if (n.start >= 0) n.data = v === null ? null : acquire(v);
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
      return silentBus(n.blockSize);
    }
    const [from, to, stopping] = range;
    const out = [];
    for (let ch = 0; ch < data.channels.length; ch++) out.push(new F32(n.blockSize));
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
      let offset = min(n.offset * data.rate, len);
      if (n.loop && step < 0) offset = max(offset, loopStart);
      if (n.loop && step >= 0 && offset >= loopEnd) offset = loopStart;
      n.pos = offset + max(0, f0 + from - startAt) * step;
      n.played = max(0, ceil(startAt - 1e-9) - startAt) * abs(step);
    }
    const maxPlayed = n.duration * data.rate;
    let pos = n.pos;
    let i = from;
    for (; i < to; i++) {
      if (n.loop) {
        const span = loopEnd - loopStart;
        if (span > 0) {
          if (step >= 0 && pos >= loopEnd) pos = loopStart + (pos - loopEnd) % span;
          if (step < 0 && pos < loopStart) pos = loopStart + ((pos - loopStart) % span + span) % span;
        }
      } else if ((pos >= len && !(pos === len && step < 0)) || pos < 0) {
        break;
      }
      if (n.played >= maxPlayed) break;
      const k = floor(pos);
      const frac = pos - k;
      for (let ch = 0; ch < out.length; ch++) {
        const src = data.channels[ch];
        const a = src[k] || 0;
        if (frac === 0) { out[ch][i] = a; continue; }
        let next = k + 1;
        if (n.loop && pos < loopEnd && next >= loopEnd) next = floor(loopStart);
        const slope = next < len ? (src[next] || 0) - a : k === len - 1 && k > 0 ? a - src[k - 1] : 0;
        out[ch][i] = a + slope * frac;
      }
      pos += step;
      n.played += abs(step);
    }
    n.pos = pos;
    if (i < to || stopping) end(n);
    return out;
  }

  const periodicWaves = new WeakMap();
  function waveCoefficients(value) {
    if (value === undefined) return undefined;
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) throw new TypeErrorC('Expected a coefficient sequence.');
    const iterator = value[Symbol.iterator];
    if (typeof iterator !== 'function') throw new TypeErrorC('Expected a coefficient sequence.');
    return Array.from({[Symbol.iterator]: () => apply(iterator, value, [])}, entry => finite(fround(entry), 'construct', 'PeriodicWave'));
  }
  function waveTable(state, last) {
    const {size, harmonics} = state, real = new F64(size), imaginary = new F64(size);
    for (let index = 0; index < harmonics.length && harmonics[index][0] <= last; index++) {
      const [order, cosine, sine] = harmonics[index];
      let slot = 0;
      for (let bit = 1; bit < size; bit *= 2) slot = slot * 2 + Number((order & bit) !== 0);
      real[slot] = cosine; imaginary[slot] = sine;
    }
    transformBitReversed(real, imaginary);
    return real;
  }
  class PeriodicWave {
    constructor(context, options = {}) {
      contextArg(context, 'PeriodicWave');
      if (options != null && typeof options !== 'object' && typeof options !== 'function') throw new TypeErrorC('Invalid PeriodicWaveOptions.');
      const normalize = !options?.disableNormalization;
      const imag = waveCoefficients(options?.imag), real = waveCoefficients(options?.real);
      const length = real?.length ?? imag?.length ?? 2;
      if (length < 2 || (imag && imag.length !== length)) throw new DE('Invalid coefficient lengths.', 'IndexSizeError');
      const harmonics = [];
      for (let order = 1; order < length; order++) {
        const cosine = real?.[order] ?? 0, sine = imag?.[order] ?? Number(!real && !imag && order === 1);
        if (cosine || sine) harmonics.push([order, cosine, sine]);
      }
      const highest = harmonics.at(-1)?.[0] ?? 0;
      const state = {harmonics, highest, size: 2 ** ceil(Math.log2(max(4096, 4 * highest))), scale: 1};
      if (normalize && highest) {
        const table = waveTable(state, highest);
        let peak = 0;
        for (const sample of table) peak = max(peak, abs(sample));
        if (peak) state.scale = 1 / peak;
      }
      periodicWaves.set(this, state);
    }
  }
  webidl(PeriodicWave, 'PeriodicWave');
  function customSample(state, phase, frequency, rate) {
    const limit = frequency ? rate / (2 * abs(frequency)) : Infinity;
    let sample = 0;
    for (const [order, cosine, sine] of state.harmonics) {
      if (order >= limit) break;
      const angle = 2 * PI * order * phase;
      sample += cosine * cos(angle) + sine * sin(angle);
    }
    return state.scale * sample;
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
      if (o.periodicWave !== undefined) this.setPeriodicWave(o.periodicWave);
      else if (o.type !== undefined) {
        if (o.type === 'custom') throw new DE("Failed to construct 'OscillatorNode': A PeriodicWave must be specified if the type is set to \"custom\"", 'InvalidStateError');
        this.type = o.type;
      }
      if (o.frequency !== undefined) setParamValue(params.get(n.frequency), finite(o.frequency, 'construct', 'OscillatorNode'), 0);
      if (o.detune !== undefined) setParamValue(params.get(n.detune), finite(o.detune, 'construct', 'OscillatorNode'), 0);
      n.process = (f0, q) => {
        const rate = ctxOf(context).rate;
        const range = activeRange(n, rate, f0);
        if (!range) return silentBus(n.blockSize);
        const [from, to, stopping] = range;
        const out = new F32(n.blockSize);
        const fr = paramValues(params.get(n.frequency), f0, rate, q);
        const dt = paramValues(params.get(n.detune), f0, rate, q);
        let phase = n.phase;
        for (let i = from; i < to; i++) {
          const f = fr[i] * pow(2, dt[i] / 1200);
          if (abs(f) < nyquist) {
            out[i] = n.type === 'custom' ? customSample(n.periodicWave, phase, f, rate) : wave(n.type, phase);
          }
          phase += max(-nyquist, min(nyquist, f)) / rate;
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
    setPeriodicWave(periodicWave) {
      const node = nodeOf(this), state = periodicWaves.get(periodicWave);
      if (!state) throw new TypeErrorC('Expected a PeriodicWave.');
      node.periodicWave = state; node.type = 'custom';
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
        if (!range) return silentBus(n.blockSize);
        const [from, to, stopping] = range;
        const v = paramValues(params.get(n.offset), f0, rate, q);
        const out = new F32(n.blockSize);
        for (let i = from; i < to; i++) out[i] = v[i];
        if (stopping) end(n);
        return [out];
      };
    }
    get offset() { return nodeOf(this).offset; }
  }
  webidl(ConstantSourceNode, 'ConstantSourceNode');

  // ---- AudioWorkletNode -----------------------------------------------------------------------------
  // The node's processor lives in the context's AudioWorkletGlobalScope realm (audio_worklet.rs,
  // audio_worklet_global.js); `process()` runs there once per render quantum, with copies of the
  // quantum's input channels and fresh output channels the node reads back. Messages travel over a
  // MessageChannel of this realm whose far end is bridged to the processor's own `port`.
  const paramMaps = new WeakMap();
  function paramMapOf(self) {
    const m = paramMaps.get(self);
    if (!m) throw invocation();
    return m;
  }
  class AudioParamMap {
    constructor(token, entries) {
      if (token !== secret) throw illegal();
      paramMaps.set(this, new Map(entries));
    }
    get size() { return paramMapOf(this).size; }
    get(key) {
      const m = paramMapOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'get' on 'AudioParamMap': 1 argument required, but only 0 present.");
      return m.get(`${key}`);
    }
    has(key) {
      const m = paramMapOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'has' on 'AudioParamMap': 1 argument required, but only 0 present.");
      return m.has(`${key}`);
    }
    entries() { return paramMapOf(this).entries(); }
    keys() { return paramMapOf(this).keys(); }
    values() { return paramMapOf(this).values(); }
    forEach(callback, thisArg) {
      const m = paramMapOf(this);
      if (arguments.length < 1) throw new TypeErrorC("Failed to execute 'forEach' on 'AudioParamMap': 1 argument required, but only 0 present.");
      if (typeof callback !== 'function') throw new TypeErrorC("Failed to execute 'forEach' on 'AudioParamMap': parameter 1 is not of type 'Function'.");
      for (const [key, value] of m) apply(callback, thisArg, [value, key, this]);
    }
  }
  AudioParamMap.prototype[Symbol.iterator] = AudioParamMap.prototype.entries;
  webidl(AudioParamMap, 'AudioParamMap');

  const ErrorEventC = typeof ErrorEvent === 'function' ? ErrorEvent : Ev;
  const portPost = typeof MessagePort === 'function' ? MessagePort.prototype.postMessage : null;
  // Registers a node the render loop pulls every quantum, whether or not anything listens to it
  // (AudioWorkletNode and ScriptProcessorNode run for their side effects).
  function addPuller(c, n) {
    (c.pulled ??= new Set()).add(n);
    if (c.syncPump) c.syncPump();
  }
  // The node's processor failed: `processorerror` is queued as an ErrorEvent, and the node outputs
  // silence from now on.
  function processorError(n, error) {
    n.worklet.dead = true;
    n.outputPorts = null;
    const c = ctxOf(n.ctx);
    if (c.pulled) c.pulled.delete(n);
    let message = '', filename = '', lineno = 0, colno = 0;
    try {
      const object = error !== null && (typeof error === 'object' || typeof error === 'function');
      message = object && typeof error.message === 'string' ? error.message : `${error}`;
      if (object) {
        if (typeof error.sourceURL === 'string') filename = error.sourceURL;
        if (typeof error.line === 'number') lineno = error.line;
        if (typeof error.column === 'number') colno = error.column;
      }
    } catch {}
    if (message === '') message = 'The AudioWorkletProcessor failed.';
    setTimer(() => { apply(dispatch, n.self, [new ErrorEventC('processorerror', { message, filename, lineno, colno, error })]); }, 0);
  }
  function processWorklet(n, f0, q) {
    const size = n.blockSize;
    const w = n.worklet;
    const rate = ctxOf(n.ctx).rate;
    // Every input is pulled (the nodes feeding it keep their own state), an input with no active
    // connection is an empty array of channels.
    const inputs = [];
    let anyActive = false;
    for (let port = 0; port < n.numberOfInputs; port++) {
      const sources = n.numberOfInputs > 1 ? n.inputs.filter(s => s.input === port) : n.inputs;
      const bus = input(n, q, f0, port);
      const active = bus !== null && sources.some(s => s.node.q === q && s.node.active);
      inputs.push(active ? bus : []);
      if (active) anyActive = true;
    }
    const silence = () => {
      n.outputPorts = Array.from({ length: n.numberOfOutputs }, () => silentBus(size));
      return n.outputPorts[0] ?? silentBus(size);
    };
    // A processor that returned false runs again only while an input is active.
    if (w.dead || (!w.processing && !anyActive)) return silence();
    const counts = n.outputChannelCount ?? (n.numberOfInputs === 1 && n.numberOfOutputs === 1 ? [max(1, inputs[0].length)] : new Array(n.numberOfOutputs).fill(1));
    const names = [], values = [];
    for (const [name, p] of n.workletParams) {
      let v = paramValues(p, f0, rate, q);
      if (v.length > 1) {
        // k-rate, and a-rate that holds one value over the quantum without inputs: one sample.
        let constant = p.rate === 'k-rate';
        if (!constant && !p.inputs.length) {
          constant = true;
          for (let i = 1; i < v.length; i++) if (v[i] !== v[0]) { constant = false; break; }
        }
        if (constant) v = v.subarray(0, 1);
      }
      names.push(name);
      values.push(v);
    }
    const r = w.host.process(w.handle, f0, inputs, counts, names, values);
    if (!r.ok) {
      processorError(n, r.error);
      return silence();
    }
    w.processing = r.active;
    n.processingFrame = f0;
    n.outputPorts = counts.map((count, port) => {
      const bus = [];
      const produced = r.outputs[port];
      for (let ch = 0; ch < count; ch++) {
        const channel = new F32(size);
        const source = produced === undefined ? undefined : produced[ch];
        // A channel whose buffer the processor transferred away is silent.
        if (source !== undefined && source.length === size) channel.set(source);
        bus.push(channel);
      }
      return bus;
    });
    return n.outputPorts[0] ?? silentBus(size);
  }
  function workletOptions(options) {
    const o = options;
    const inputs = o.numberOfInputs === undefined ? 1 : ulong(o.numberOfInputs);
    const outputs = o.numberOfOutputs === undefined ? 1 : ulong(o.numberOfOutputs);
    let counts = null;
    if (o.outputChannelCount !== undefined) {
      if (o.outputChannelCount === null || typeof o.outputChannelCount[Symbol.iterator] !== 'function') {
        throw new TypeErrorC("Failed to construct 'AudioWorkletNode': The provided value cannot be converted to a sequence.");
      }
      counts = Array.from(o.outputChannelCount, ulong);
    }
    let data = null;
    if (o.parameterData !== undefined) {
      const given = o.parameterData;
      if (given === null || (typeof given !== 'object' && typeof given !== 'function')) {
        throw new TypeErrorC("Failed to construct 'AudioWorkletNode': The provided value is not of type 'record'.");
      }
      data = {};
      for (const key of Object.keys(given)) data[key] = finite(given[key], 'construct', 'AudioWorkletNode', 'double');
    }
    let processorOptions;
    if (o.processorOptions !== undefined) {
      if (o.processorOptions === null || (typeof o.processorOptions !== 'object' && typeof o.processorOptions !== 'function')) {
        throw new TypeErrorC("Failed to construct 'AudioWorkletNode': Failed to read the 'processorOptions' property from 'AudioWorkletNodeOptions': The provided value is not of type 'object'.");
      }
      processorOptions = o.processorOptions;
    }
    return { inputs, outputs, counts, data, processorOptions };
  }
  class AudioWorkletNode extends AudioNode {
    constructor(context, name, options = undefined) {
      if (arguments.length < 2) throw new TypeErrorC(`Failed to construct 'AudioWorkletNode': 2 arguments required, but only ${arguments.length} present.`);
      contextArg(context, 'AudioWorkletNode');
      const nodeName = `${name}`;
      const inherited = readNodeOptions(options, 'AudioWorkletNode');
      const { inputs, outputs, counts, data, processorOptions } = workletOptions(options ?? {});
      const c = ctxOf(context);
      const host = c.workletHost === undefined ? undefined : c.workletHost();
      const descriptors = host === undefined ? undefined : host.lookup(nodeName);
      if (descriptors === undefined) {
        throw new DE(`Failed to construct 'AudioWorkletNode': The node name '${nodeName}' is not defined in AudioWorkletGlobalScope.`, 'InvalidStateError');
      }
      if (inputs === 0 && outputs === 0) {
        throw new DE("Failed to construct 'AudioWorkletNode': Number of inputs and number of outputs cannot be both zero.", 'NotSupportedError');
      }
      if (inputs > MAX_CHANNELS || outputs > MAX_CHANNELS) {
        throw new DE(`Failed to construct 'AudioWorkletNode': The number of inputs (${inputs}) or outputs (${outputs}) exceeds the maximum of ${MAX_CHANNELS}.`, 'NotSupportedError');
      }
      if (counts !== null) {
        if (counts.length !== outputs) {
          throw new DE(`Failed to construct 'AudioWorkletNode': Length of specified outputChannelCount (${counts.length}) does not match the given number of outputs (${outputs}).`, 'IndexSizeError');
        }
        for (const count of counts) {
          if (count < 1 || count > MAX_CHANNELS) {
            throw new DE(`Failed to construct 'AudioWorkletNode': Channel count (${count}) in outputChannelCount is outside the range [1, ${MAX_CHANNELS}].`, 'NotSupportedError');
          }
        }
      }
      // StructuredSerialize of the options (a DataCloneError for what cannot be cloned).
      const record = { numberOfInputs: inputs, numberOfOutputs: outputs };
      if (counts !== null) record.outputChannelCount = counts;
      if (data !== null) record.parameterData = data;
      if (processorOptions !== undefined) record.processorOptions = processorOptions;
      const serialized = structuredCloneC === null ? record : structuredCloneC(record);
      super(secret, context, { numberOfInputs: inputs, numberOfOutputs: outputs });
      const n = nodeOf(this);
      applyNodeOptions(this, inherited);
      n.outputChannelCount = counts;
      const entries = [];
      n.workletParams = [];
      for (const d of descriptors) {
        const param = nodeParam(n, d.defaultValue, d.minValue, d.maxValue, d.automationRate);
        const state = params.get(param);
        if (data !== null && Object.prototype.hasOwnProperty.call(data, d.name)) setParamValue(state, fround(data[d.name]), 0);
        n.workletParams.push([d.name, state]);
        entries.push([d.name, param]);
      }
      n.parameters = new AudioParamMap(secret, entries);
      // The global's `currentFrame` / `currentTime` outside a render are the context's.
      host.setClock(() => c.currentFrames());
      const channel = new MessageChannel();
      n.port = channel.port1;
      const far = channel.port2;
      const send = (message, transfer) => { apply(portPost, far, transfer === undefined ? [message] : [message, transfer]); };
      const created = host.construct(nodeName, serialized, send, () => { far.close(); });
      n.deliver = created.deliver;
      far.onmessage = (event) => { n.deliver(event.data); };
      n.worklet = { host, handle: created.handle, processing: true, dead: false };
      // Outputs and inputs follow what the processor reports; its tail is its own business.
      c.trackConvolverActivity = true;
      n.process = (f0, q) => processWorklet(n, f0, q);
      addPuller(c, n);
      if (!created.ok) processorError(n, created.error);
    }
    get parameters() { return nodeOf(this).parameters; }
    get port() { return nodeOf(this).port; }
  }
  handler(AudioWorkletNode, 'processorerror');
  webidl(AudioWorkletNode, 'AudioWorkletNode');

  // ---- ScriptProcessorNode (deprecated) ---------------------------------------------------------------
  // Input is collected into a buffer of `bufferSize` frames; each time it fills, `audioprocess` runs
  // with it and an empty output buffer. What the handler writes plays during the buffer after the
  // next one (two buffers of latency, as in Chrome), and silence plays until then.
  class AudioProcessingEvent extends Ev {
    #playbackTime;
    #inputBuffer;
    #outputBuffer;
    constructor(type, eventInitDict) {
      if (arguments.length < 2) {
        throw new TypeErrorC(`Failed to construct 'AudioProcessingEvent': 2 arguments required, but only ${arguments.length} present.`);
      }
      const init = eventInitDict;
      if (init === null || typeof init !== 'object') {
        throw new TypeErrorC("Failed to construct 'AudioProcessingEvent': The provided value is not of type 'AudioProcessingEventInit'.");
      }
      for (const member of ['inputBuffer', 'outputBuffer', 'playbackTime']) {
        if (init[member] === undefined) {
          throw new TypeErrorC(`Failed to construct 'AudioProcessingEvent': Failed to read the '${member}' property from 'AudioProcessingEventInit': Required member is undefined.`);
        }
      }
      for (const member of ['inputBuffer', 'outputBuffer']) {
        if (!buffers.has(init[member])) {
          throw new TypeErrorC(`Failed to construct 'AudioProcessingEvent': Failed to read the '${member}' property from 'AudioProcessingEventInit': The provided value is not of type 'AudioBuffer'.`);
        }
      }
      const playbackTime = finite(init.playbackTime, 'construct', 'AudioProcessingEvent', 'double');
      super(type, init);
      this.#playbackTime = playbackTime;
      this.#inputBuffer = init.inputBuffer;
      this.#outputBuffer = init.outputBuffer;
    }
    get playbackTime() { return this.#playbackTime; }
    get inputBuffer() { return this.#inputBuffer; }
    get outputBuffer() { return this.#outputBuffer; }
  }
  webidl(AudioProcessingEvent, 'AudioProcessingEvent');

  const SCRIPT_BUFFER_SIZES = [256, 512, 1024, 2048, 4096, 8192, 16384];
  function processScript(n, f0, q) {
    const s = n.script;
    const size = n.blockSize;
    const bus = input(n, q, f0);
    const at = s.index;
    for (let ch = 0; ch < s.inputChannels; ch++) {
      if (bus && bus[ch]) s.input[ch].set(bus[ch].subarray(0, size), at);
    }
    const out = [];
    for (let ch = 0; ch < s.outputChannels; ch++) {
      const channel = new F32(size);
      channel.set(s.output[s.current][ch].subarray(at, at + size));
      out.push(channel);
    }
    s.index += size;
    if (s.index >= s.bufferSize) {
      const c = ctxOf(n.ctx);
      const inputBuffer = makeBuffer(c.rate, s.input);
      const outputData = [];
      for (let ch = 0; ch < max(1, s.outputChannels); ch++) outputData.push(new F32(s.bufferSize));
      const outputBuffer = makeBuffer(c.rate, outputData);
      s.input = Array.from({ length: max(1, s.inputChannels) }, () => new F32(s.bufferSize));
      s.index = 0;
      // The buffer that plays out next time round is the one the handler fills now.
      const filled = s.current;
      s.current = 1 - s.current;
      const event = new AudioProcessingEvent('audioprocess', { playbackTime: (f0 + size) / c.rate, inputBuffer, outputBuffer });
      apply(dispatch, n.self, [event]);
      s.output[filled] = outputData;
    }
    return s.outputChannels ? out : silentBus(size);
  }
  class ScriptProcessorNode extends AudioNode {
    constructor(token, ctx, bufferSize, inputChannels, outputChannels) {
      if (token !== secret) throw illegal();
      super(secret, ctx, {
        channelCount: inputChannels, mode: 'explicit', modeError: 'NotSupportedError', fixedMode: true,
        checkCount(count) {
          if (count !== inputChannels) {
            throw new DE(`Failed to set the 'channelCount' property on 'AudioNode': channelCount cannot be changed from ${inputChannels} to ${count}`, 'NotSupportedError');
          }
        },
      });
      const n = nodeOf(this);
      n.script = {
        bufferSize, inputChannels, outputChannels, index: 0, current: 0,
        input: Array.from({ length: max(1, inputChannels) }, () => new F32(bufferSize)),
        output: [0, 1].map(() => Array.from({ length: max(1, outputChannels) }, () => new F32(bufferSize))),
      };
      n.process = (f0, q) => processScript(n, f0, q);
      const c = ctxOf(ctx);
      addPuller(c, n);
    }
    get bufferSize() { return nodeOf(this).script.bufferSize; }
  }
  handler(ScriptProcessorNode, 'audioprocess');
  webidl(ScriptProcessorNode, 'ScriptProcessorNode');
  function createScriptProcessor(context, bufferSize, inputChannels, outputChannels) {
    const c = ctxOf(context);
    const method = "Failed to execute 'createScriptProcessor' on 'BaseAudioContext'";
    const inputs = ulong(inputChannels), outputs = ulong(outputChannels);
    let size = ulong(bufferSize);
    if (size === 0) size = c.offline ? 16384 : 512;
    else if (!SCRIPT_BUFFER_SIZES.includes(size)) {
      throw new DE(`${method}: buffer size (${size}) must be 0 or a power of two between 256 and 16384.`, 'IndexSizeError');
    }
    if (inputs === 0 && outputs === 0) throw new DE(`${method}: number of input channels and output channels cannot both be zero.`, 'IndexSizeError');
    if (inputs > MAX_CHANNELS) throw new DE(`${method}: number of input channels (${inputs}) exceeds maximum (${MAX_CHANNELS}).`, 'IndexSizeError');
    if (outputs > MAX_CHANNELS) throw new DE(`${method}: number of output channels (${outputs}) exceeds maximum (${MAX_CHANNELS}).`, 'IndexSizeError');
    // The buffer is made of whole render quanta.
    if (size % c.quantum !== 0) throw new DE(`${method}: buffer size (${size}) is not a multiple of the render quantum size (${c.quantum}).`, 'NotSupportedError');
    return new ScriptProcessorNode(secret, context, size, inputs, outputs);
  }

  // ---- Contexts ------------------------------------------------------------------
  const contexts = new WeakMap();
  function ctxOf(self) {
    const c = contexts.get(self);
    if (!c) throw invocation();
    return c;
  }
  function renderQuantum(hint) {
    if (typeof hint === 'number') return ulong(hint);
    if (hint !== undefined && !['default', 'hardware'].includes(`${hint}`)) throw new TypeErrorC('Invalid AudioContextRenderSizeCategory.');
    return DEFAULT_QUANTUM;
  }
  function checkQuantum(quantum, rate) {
    if (!quantum || quantum > 6 * rate) throw new DE('Render quantum size outside supported range.', 'NotSupportedError');
  }
  class BaseAudioContext extends ET {
    constructor(token, rate, channels, offline, quantum = DEFAULT_QUANTUM) {
      if (token !== secret) throw illegal();
      super();
      const self = this;
      const c = {
        self, rate, offline, quantum, state: 'suspended', frames: 0, sources: new Set(), endedQueue: [],
        time() { return c.currentFrames() / c.rate; },
        currentFrames() { return c.frames; },
        scheduleEnds() {},
      };
      contexts.set(this, c);
      c.destination = new AudioDestinationNode(secret, this, channels, offline);
    }
    get sampleRate() { return ctxOf(this).rate; }
    get renderQuantumSize() { return ctxOf(this).quantum; }
    get currentTime() { return ctxOf(this).time(); }
    get destination() { return ctxOf(this).destination; }
    // Its own AudioWorklet (paint_worklet.js, audio_worklet.rs), made on first use.
    get audioWorklet() {
      const c = ctxOf(this);
      if (c.audioWorklet === undefined) {
        // [the AudioWorklet, a function returning the worklet global's host once its realm exists]
        const made = makeAudioWorklet(c.rate, c.quantum);
        if (made !== undefined) {
          c.audioWorklet = made[0];
          c.workletHost = made[1];
        }
      }
      return c.audioWorklet;
    }
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
    createDynamicsCompressor() { return new DynamicsCompressorNode(this); }
    createGain() { return new GainNode(this); }
    createWaveShaper() { return new WaveShaperNode(this); }
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
    createPeriodicWave(real, imag, constraints = {}) {
      ctxOf(this);
      const cosine = waveCoefficients(real), sine = waveCoefficients(imag);
      if (!cosine || !sine) throw new TypeErrorC('Two coefficient sequences are required.');
      if (constraints != null && typeof constraints !== 'object' && typeof constraints !== 'function') throw new TypeErrorC('Invalid PeriodicWaveConstraints.');
      return new PeriodicWave(this, {real: cosine, imag: sine, disableNormalization: constraints?.disableNormalization});
    }
    createConstantSource() { return new ConstantSourceNode(this); }
    createScriptProcessor(bufferSize = 0, numberOfInputChannels = 2, numberOfOutputChannels = 2) {
      return createScriptProcessor(this, bufferSize, numberOfInputChannels, numberOfOutputChannels);
    }
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
  if (!secureContext) delete BaseAudioContext.prototype.audioWorklet;
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

  const playbackContexts = new WeakMap();
  function statsContext(self) {
    const c = playbackContexts.get(self);
    if (!c) throw invocation();
    return c;
  }
  function refreshDuration(c) {
    const stamp = now();
    if (c.state === 'running' && stamp - c.statsStamp >= 1000 && natives.canUpdatePlaybackStats?.()) {
      c.statsDuration = c.time();
      c.statsStamp = stamp;
    }
  }
  function statsDuration(self) {
    const c = statsContext(self);
    if (!c.statsLocked) {
      refreshDuration(c);
      c.statsLocked = true;
      queueMicrotaskC(() => { c.statsLocked = false; });
    }
    return c.statsDuration;
  }
  class AudioPlaybackStats {
    constructor(...args) {
      if (args[0] !== secret) throw illegal();
      playbackContexts.set(this, args[1]);
    }
    get underrunDuration() { statsDuration(this); return 0; }
    get underrunEvents() { statsDuration(this); return 0; }
    get totalDuration() { return statsDuration(this); }
    get averageLatency() { statsDuration(this); return 0; }
    get minimumLatency() { statsDuration(this); return 0; }
    get maximumLatency() { statsDuration(this); return 0; }
    resetLatency() { statsContext(this); }
    toJSON() {
      return { underrunDuration: 0, underrunEvents: 0, totalDuration: statsDuration(this), averageLatency: 0, minimumLatency: 0, maximumLatency: 0 };
    }
  }
  webidl(AudioPlaybackStats, 'AudioPlaybackStats');

  // A real-time context: no output device, a clock while running.
  class AudioContext extends BaseAudioContext {
    constructor(options) {
      const o = contextOptions(options, 'AudioContext');
      const latency = o.latencyHint;
      if (typeof latency === 'string' && !['interactive', 'balanced', 'playback'].includes(latency)) {
        throw new TypeErrorC(`Failed to construct 'AudioContext': Failed to read the 'latencyHint' property from 'AudioContextOptions': The provided value '${latency}' is not a valid enum value of type AudioContextLatencyCategory.`);
      }
      const quantum = renderQuantum(o.renderSizeHint);
      const sampleRate = o.sampleRate;
      const rate = sampleRate === undefined ? 48000 : fround(finite(sampleRate, 'construct', 'AudioContext'));
      if (!(rate >= MIN_RATE && rate <= MAX_RATE)) {
        throw new DE(`Failed to construct 'AudioContext': The hardware sample rate provided (${rate}) is outside the range [${MIN_RATE}, ${MAX_RATE}].`, 'NotSupportedError');
      }
      checkQuantum(quantum, rate);
      super(secret, rate, 2, false, quantum);
      const c = ctxOf(this);
      c.started = 0; // document time the clock (re)started at
      c.timer = undefined;
      c.pendingResumes = [];
      c.statsStamp = now();
      c.statsDuration = 0;
      c.currentFrames = () => {
        if (c.state !== 'running') return c.frames;
        const elapsed = (now() - c.started) / 1000 * c.rate;
        return c.frames + floor(elapsed / quantum) * quantum;
      };
      // There is no output device: the graph is computed (so that script processors, worklets and
      // analysers see real samples) for the quanta the clock has passed, but only while one of those
      // nodes exists (`c.pulled`, `c.analyserNodes`); the destination's result is dropped.
      c.rendered = 0; // next frame to compute
      c.pumpOn = false;
      c.pumpTimer = undefined;
      c.wantsPump = () => !!(c.pulled?.size || c.analyserNodes?.size);
      c.catchUp = (target, limit) => {
        c.graphEpoch = (c.graphEpoch ?? 0) + 1;
        for (let done = 0; c.rendered <= target && done < limit; done++) {
          renderBlock(c, c.rendered);
          c.rendered += quantum;
        }
        flushEnded(c);
      };
      c.syncPump = () => {
        if (c.state !== 'running' || !c.wantsPump()) {
          if (c.pumpTimer !== undefined) { clearClock(c.pumpTimer); c.pumpTimer = undefined; }
          return;
        }
        if (!c.pumpOn) { c.pumpOn = true; c.rendered = max(c.rendered, c.currentFrames()); }
        if (c.pumpTimer === undefined) {
          c.pumpTimer = timer(max(1, quantum * 1000 / rate), () => {
            c.pumpTimer = undefined;
            if (c.state !== 'running') return;
            // A node that throws does not stop the clock of the others.
            try { c.catchUp(c.currentFrames(), 64); } finally { c.syncPump(); }
          });
        }
      };
      c.run = () => {
        if (c.state === 'running' || c.state === 'closed') return;
        c.started = now();
        setState(c, 'running');
        c.scheduleEnds();
        c.syncPump();
      };
      c.halt = (state) => {
        if (!c.statsLocked) refreshDuration(c);
        if (c.state === 'running') {
          // What the clock has passed is computed before it stops.
          if (c.pumpOn) c.catchUp(c.currentFrames(), 2048);
          c.frames = c.currentFrames();
        }
        if (c.timer !== undefined) { clearClock(c.timer); c.timer = undefined; }
        if (c.pumpTimer !== undefined) { clearClock(c.pumpTimer); c.pumpTimer = undefined; }
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
        if (next !== Infinity) {
          const endFrame = ceil(next * rate / quantum) * quantum;
          const delay = (endFrame - c.frames) * 1000 / rate - (now() - c.started);
          c.timer = timer(max(0, delay) + 1, () => { c.timer = undefined; c.scheduleEnds(); });
        }
      };
      realtime.add(new WeakRefC(c));
      // Chrome's autoplay policy: a context made after a user gesture runs; one whose frame is
      // not rendered under a `media-playback-while-not-visible` policy that disables it is
      // interrupted instead (and runs when the frame is rendered, `frameRendered`).
      if (activated()) setTimer(() => { if (mediaBlocked()) c.halt('interrupted'); else c.run(); }, 0);
    }
    get baseLatency() {
      const c = ctxOf(this);
      return max(DEFAULT_QUANTUM, ceil(256 * c.rate / 48000), c.quantum) / c.rate;
    }
    get outputLatency() { ctxOf(this); return 0; }
    get playbackStats() {
      const c = ctxOf(this);
      if (c.offline) throw invocation();
      return c.playbackStats ??= new AudioPlaybackStats(secret, c);
    }
    getOutputTimestamp() {
      const c = ctxOf(this);
      return { contextTime: c.time(), performanceTime: c.state === 'running' ? now() : 0 };
    }
    resume() {
      const c = ctxOf(this);
      if (c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'resume' on 'AudioContext': Cannot resume a context that has been closed.", 'InvalidStateError'));
      // iframe-media-pausing: not rendered under a policy that disables it, the context is
      // interrupted and the promise rejects.
      if (mediaBlocked()) {
        return new PromiseC((resolve, reject) => setTimer(() => {
          if (c.state !== 'closed') c.halt('interrupted');
          reject(new DE("Failed to execute 'resume' on 'AudioContext': Cannot resume a context whose frame is not visible.", 'InvalidStateError'));
        }, 0));
      }
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
  // The real-time contexts of this realm (weakly), for `frameRendered`.
  const realtime = new Set();
  // The frame became not rendered (false) or rendered (true) while the document's policy
  // disables `media-playback-while-not-visible` (webaudio.rs `frame_rendered_changed`): running
  // contexts are interrupted, interrupted ones run again. A suspended context is left alone.
  function frameRendered(rendered) {
    for (const ref of realtime) {
      const c = ref.deref();
      if (c === undefined || c.state === 'closed') { realtime.delete(ref); continue; }
      if (!rendered) { if (c.state === 'running') c.halt('interrupted'); }
      else if (c.state === 'interrupted') c.run();
    }
  }
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
      let channels, frames, rate, quantum = DEFAULT_QUANTUM;
      if (arguments.length === 1 && numberOfChannels !== null && typeof numberOfChannels === 'object') {
        const o = numberOfChannels;
        const lengthOption = o.length;
        channels = o.numberOfChannels === undefined ? 1 : ulong(o.numberOfChannels);
        frames = lengthOption == null ? null : ulong(lengthOption);
        quantum = renderQuantum(o.renderSizeHint);
        const rateOption = o.sampleRate;
        if (rateOption === undefined) throw new TypeErrorC("Failed to construct 'OfflineAudioContext': Failed to read the 'sampleRate' property from 'OfflineAudioContextOptions': Required member is undefined.");
        rate = fround(finite(rateOption, 'construct', 'OfflineAudioContext'));
      } else {
        if (arguments.length < 3) throw new TypeErrorC(`Failed to construct 'OfflineAudioContext': 3 arguments required, but only ${arguments.length} present.`);
        channels = ulong(numberOfChannels);
        frames = ulong(length);
        rate = finite(sampleRate, 'construct', 'OfflineAudioContext');
      }
      checkBuffer('construct', 'OfflineAudioContext', channels, frames ?? 1, rate);
      checkQuantum(quantum, rate);
      super(secret, rate, channels, true, quantum);
      const c = ctxOf(this);
      c.length = frames;
      c.channels = channels;
      c.suspends = new Map();
      c.rendering = false;
      c.committed = 0;
      c.chunks = [];
    }
    get length() { return ctxOf(this).length; }
    startRendering(chunkSize = null) {
      const c = ctxOf(this);
      return new PromiseC((resolve, reject) => {
        const size = chunkSize === null ? null : ulong(chunkSize);
        if (c.closing || c.state === 'closed' || c.committed === c.length) throw new DE('No further frames can be rendered.', 'InvalidStateError');
        if (size === 0) throw new DE('The requested chunk size cannot be zero.', 'NotSupportedError');
        const start = c.committed;
        const end = size === null ? (c.length ?? start + c.quantum) : min(c.length ?? Infinity, start + ceil(size / c.quantum) * c.quantum);
        const output = Array.from({length: c.channels}, () => new F32(end - start));
        c.committed = end;
        c.chunks.push({start, end, output, resolve, reject});
        if (!c.rendering) {
          c.rendering = true;
          c.state = 'running';
          setTimer(() => fire(c.self, 'statechange'), 0);
        }
        if (c.chunks.length === 1) setTimer(() => render(c), 0);
      });
    }
    close() {
      const c = ctxOf(this);
      if (c.closing || c.state === 'closed') return PromiseC.reject(new DE('Context is already closed.', 'InvalidStateError'));
      c.closing = true;
      return new PromiseC(resolve => {
        const finish = () => {
          c.state = 'closed';
          for (const chunk of c.chunks) chunk.reject(new DE('Rendering was stopped.', 'AbortError'));
          c.chunks.length = 0;
          for (const pause of c.suspends.values()) pause.reject(new DE('Context was closed.', 'InvalidStateError'));
          c.suspends.clear();
          setTimer(() => fire(c.self, 'statechange'), 0);
          resolve();
        };
        if (c.rendering) setTimer(finish, 0);
        else finish();
      });
    }
    resume() {
      const c = ctxOf(this);
      if (c.closing || c.state === 'closed') return PromiseC.reject(new DE("Failed to execute 'resume' on 'OfflineAudioContext': Cannot resume a context that has been closed.", 'InvalidStateError'));
      if (!c.rendering) return PromiseC.reject(new DE("Failed to execute 'resume' on 'OfflineAudioContext': Cannot resume a context that has not started.", 'InvalidStateError'));
      if (c.state === 'suspended') {
        c.state = 'running';
        setTimer(() => fire(c.self, 'statechange'), 0);
        setTimer(() => render(c), 0);
      }
      return PromiseC.resolve();
    }
    suspend(suspendTime) {
      const c = ctxOf(this);
      if (arguments.length < 1) return PromiseC.reject(new TypeErrorC("Failed to execute 'suspend' on 'OfflineAudioContext': 1 argument required, but only 0 present."));
      const t = +suspendTime;
      if (!(t >= 0)) return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': negative suspend time (${t}) is not allowed`, 'InvalidStateError'));
      const lower = floor(t * c.rate / c.quantum) * c.quantum;
      const frame = t > lower / c.rate ? lower + c.quantum : lower;
      if (c.closing || c.state === 'closed') return PromiseC.reject(new DE('Context is closed.', 'InvalidStateError'));
      if (c.length !== null && frame >= c.length) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': cannot schedule a suspend at frame ${frame} (${t} seconds) because it is greater than or equal to the total render duration of ${c.length} frames`, 'InvalidStateError'));
      }
      if (frame < c.frames) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': suspend(${t}) failed to suspend at frame ${frame} because it is earlier than the current frame of ${c.frames}`, 'InvalidStateError'));
      }
      if (c.suspends.has(frame)) {
        return PromiseC.reject(new DE(`Failed to execute 'suspend' on 'OfflineAudioContext': cannot schedule more than one suspend at frame ${frame} (${t} seconds)`, 'InvalidStateError'));
      }
      return new PromiseC((resolve, reject) => c.suspends.set(frame, {resolve, reject}));
    }
  }
  handler(OfflineAudioContext, 'complete');
  webidl(OfflineAudioContext, 'OfflineAudioContext');

  // One render quantum of the graph at frame f0: what reaches the destination (returned), and
  // everything that runs whether or not it feeds the destination: started sources, analysers, the
  // history of fed convolvers and delays, and the nodes pulled automatically (worklets, script
  // processors). `take(bus)` gets the destination's quantum before anything else runs.
  function renderBlock(c, f0, take) {
    const q = f0 / c.quantum;
    const bus = pull(nodes.get(c.destination), q, f0);
    if (take) take(bus);
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
    if (c.pulled) for (const n of c.pulled) if (n.q !== q) pull(n, q, f0);
    if (c.delayWriters?.size) {
      for (const n of c.delayWriters) writeDelay(n, q, f0);
      c.delayWriters.clear();
    }
    return bus;
  }

  // Renders quanta until the end or a suspend point.
  function render(c) {
    const chunk = c.chunks[0];
    if (!chunk || c.closing) return;
    setState(c, 'running');
    c.graphEpoch = (c.graphEpoch ?? 0) + 1;
    const sliceEnd = min(chunk.end, c.frames + min(128 * c.quantum, 16384));
    while (c.frames < sliceEnd) {
      const pause = c.suspends.get(c.frames);
      if (pause) {
        c.suspends.delete(c.frames);
        setState(c, 'suspended');
        flushEnded(c);
        pause.resolve();
        return;
      }
      const f0 = c.frames;
      renderBlock(c, f0, (bus) => {
        const count = min(c.quantum, chunk.end - f0);
        for (let ch = 0; ch < c.channels; ch++) chunk.output[ch].set(count === c.quantum ? bus[ch] : bus[ch].subarray(0, count), f0 - chunk.start);
      });
      c.frames += c.quantum;
    }
    if (c.frames < chunk.end) {
      setTimer(() => render(c), 0);
      return;
    }
    const rendered = makeBuffer(c.rate, chunk.output);
    const final = chunk.end === c.length;
    c.chunks.shift();
    if (c.chunks.length) setTimer(() => render(c), 0);
    flushEnded(c);
    if (final) {
      c.state = 'closed';
      setTimer(() => fire(c.self, 'statechange'), 0);
    }
    chunk.resolve(rendered);
    if (final) apply(dispatch, c.self, [new OfflineAudioCompletionEvent('complete', { renderedBuffer: rendered })]);
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
    localImpulseInfo, frameRendered, BaseAudioContext, AudioContext, AudioPlaybackStats, OfflineAudioContext, AudioBuffer, AudioParam, AudioNode,
    AudioDestinationNode, AudioScheduledSourceNode, AudioBufferSourceNode, OscillatorNode, PeriodicWave,
    ConstantSourceNode, GainNode, WaveShaperNode, AnalyserNode, DelayNode, ConvolverNode, DynamicsCompressorNode, AudioListener, PannerNode, StereoPannerNode, BiquadFilterNode, IIRFilterNode, ChannelMergerNode, ChannelSplitterNode, OfflineAudioCompletionEvent,
    AudioParamMap, AudioWorkletNode, AudioProcessingEvent, ScriptProcessorNode,
  };
})
