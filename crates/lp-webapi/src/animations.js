// Web Animations (https://drafts.csswg.org/web-animations-1/), made on a
// realm's first use. The script evaluates to a function of the engine's
// helpers (animations.rs) that returns the module object.
//
// Time comes from the page clock (`now`, the clock `performance.now()` and
// CDP virtual time use), and nothing ticks: an animation that is playing
// asks the engine for one timer at the moment it will finish, and a pending
// play or pause gets one task for its ready time. Pages without animations
// never make this module; animations that are idle, paused or finished
// cost nothing over time.
//
// Animated values are not part of the cascade: `getComputedStyle` and the
// layout show the underlying style. `commitStyles()` writes the animated
// values (computed here, see `animatedValue`) into the inline style.
(native => {
  'use strict';
  const { now, schedule, cssName } = native;
  const apply = Reflect.apply;
  const P = Promise;
  const promiseThen = P.prototype.then;
  const resolvedPromise = P.resolve();
  const microtask = fn => apply(promiseThen, resolvedPromise, [fn]);
  const ET = EventTarget;
  const Ev = Event;
  const addListener = ET.prototype.addEventListener;
  const dispatch = ET.prototype.dispatchEvent;
  const DOMEx = DOMException;
  const ElementProto = Element.prototype;
  const getter = (proto, name) => {
    const d = Object.getOwnPropertyDescriptor(proto, name);
    return d && d.get ? o => apply(d.get, o, []) : o => o[name];
  };
  const isConnected = getter(Node.prototype, 'isConnected');
  const getRootNode = Node.prototype.getRootNode;
  const contains = Node.prototype.contains;
  const inlineStyle = getter(HTMLElement.prototype, 'style');
  const setProperty = CSSStyleDeclaration.prototype.setProperty;
  const getPropertyValue = CSSStyleDeclaration.prototype.getPropertyValue;
  const computedStyle = globalThis.getComputedStyle;
  const isElement = v => v !== null && typeof v === 'object' && v instanceof Element;
  const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');
  const illegal = () => new TypeError('Illegal constructor');
  const brand = (map, o, what) => {
    const st = map.get(o);
    if (!st) throw new TypeError(`Illegal invocation: not ${what}`);
    return st;
  };
  const invalidState = msg => new DOMEx(msg, 'InvalidStateError');
  const abortError = () => new DOMEx('The user aborted a request.', 'AbortError');

  // ----- timing functions ---------------------------------------------------

  const KEYWORDS = {
    linear: 'linear',
    ease: 'cubic-bezier(0.25, 0.1, 0.25, 1)',
    'ease-in': 'cubic-bezier(0.42, 0, 1, 1)',
    'ease-out': 'cubic-bezier(0, 0, 0.58, 1)',
    'ease-in-out': 'cubic-bezier(0.42, 0, 0.58, 1)',
    'step-start': 'steps(1, start)',
    'step-end': 'steps(1)',
  };
  const NUM = '\\s*([+-]?(?:\\d+\\.?\\d*|\\.\\d+)(?:e[+-]?\\d+)?)\\s*';
  const BEZIER = new RegExp(`^cubic-bezier\\(${NUM},${NUM},${NUM},${NUM}\\)$`, 'i');
  const STEPS = /^steps\(\s*([+-]?\d+)\s*(?:,\s*(jump-start|jump-end|jump-none|jump-both|start|end)\s*)?\)$/i;
  const fmt = n => String(Math.round(n * 1e6) / 1e6);

  function bezier(x1, y1, x2, y2) {
    const cx = 3 * x1, bx = 3 * (x2 - x1) - cx, ax = 1 - cx - bx;
    const cy = 3 * y1, by = 3 * (y2 - y1) - cy, ay = 1 - cy - by;
    const sx = t => ((ax * t + bx) * t + cx) * t;
    const sy = t => ((ay * t + by) * t + cy) * t;
    const dx = t => (3 * ax * t + 2 * bx) * t + cx;
    const solve = x => {
      let t = x;
      for (let i = 0; i < 8; i++) {
        const err = sx(t) - x;
        if (Math.abs(err) < 1e-7) return t;
        const d = dx(t);
        if (Math.abs(d) < 1e-6) break;
        t -= err / d;
      }
      let lo = 0, hi = 1;
      t = x;
      while (lo < hi) {
        const v = sx(t);
        if (Math.abs(v - x) < 1e-7) return t;
        if (x > v) lo = t; else hi = t;
        t = (hi - lo) / 2 + lo;
        if (hi - lo < 1e-9) break;
      }
      return t;
    };
    return x => {
      // Outside [0, 1]: the tangent at the nearer end.
      if (x < 0) {
        const slope = x1 > 0 ? y1 / x1 : (y1 === 0 && x2 > 0 ? y2 / x2 : 0);
        return slope * x;
      }
      if (x > 1) {
        const slope = x2 < 1 ? (y2 - 1) / (x2 - 1) : (y2 === 1 && x1 < 1 ? (y1 - 1) / (x1 - 1) : 0);
        return 1 + slope * (x - 1);
      }
      return sy(solve(x));
    };
  }

  function steps(n, pos) {
    return (x, before) => {
      let current = Math.floor(x * n);
      if (pos === 'jump-start' || pos === 'jump-both') current += 1;
      if (before && Math.floor(x * n) === x * n) current -= 1;
      if (x >= 0 && current < 0) current = 0;
      const jumps = pos === 'jump-none' ? n - 1 : pos === 'jump-both' ? n + 1 : n;
      if (x <= 1 && current > jumps) current = jumps;
      return current / jumps;
    };
  }

  function linearPoints(text) {
    const inner = text.slice(7, -1).trim();
    if (!inner) return null;
    const points = [];
    for (const part of inner.split(',')) {
      const m = /^\s*([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)((?:\s+[+-]?(?:\d+\.?\d*|\.\d+)%){0,2})\s*$/i.exec(part);
      if (!m) return null;
      const stops = m[2].trim() ? m[2].trim().split(/\s+/).map(s => parseFloat(s) / 100) : [];
      if (stops.length === 0) points.push({ y: +m[1], x: null });
      for (const x of stops) points.push({ y: +m[1], x });
    }
    if (points.length < 2) return null;
    if (points[0].x === null) points[0].x = 0;
    if (points[points.length - 1].x === null) points[points.length - 1].x = 1;
    let max = points[0].x;
    for (const p of points) { if (p.x !== null) { if (p.x < max) p.x = max; max = p.x; } }
    for (let i = 0; i < points.length; i++) {
      if (points[i].x !== null) continue;
      let j = i;
      while (points[j].x === null) j++;
      const a = points[i - 1].x, b = points[j].x;
      for (let k = i; k < j; k++) points[k].x = a + (b - a) * (k - i + 1) / (j - i + 1);
      i = j;
    }
    return points;
  }

  // An easing's canonical text and function; a TypeError for anything else.
  function parseEasing(value) {
    const text = String(value).trim();
    const lower = text.toLowerCase();
    if (lower === 'linear') return { text: 'linear', fn: x => x };
    if (lower in KEYWORDS) {
      const parsed = parseEasing(KEYWORDS[lower]);
      return { text: lower, fn: parsed.fn };
    }
    let m = BEZIER.exec(text);
    if (m) {
      const [x1, y1, x2, y2] = m.slice(1).map(Number);
      if (x1 >= 0 && x1 <= 1 && x2 >= 0 && x2 <= 1) {
        return { text: `cubic-bezier(${fmt(x1)}, ${fmt(y1)}, ${fmt(x2)}, ${fmt(y2)})`, fn: bezier(x1, y1, x2, y2) };
      }
    }
    m = STEPS.exec(text);
    if (m) {
      const n = +m[1];
      let pos = (m[2] || 'end').toLowerCase();
      if (pos === 'start') pos = 'jump-start';
      if (pos === 'end') pos = 'jump-end';
      if (n >= 1 && !(pos === 'jump-none' && n < 2)) {
        const shown = pos === 'jump-end' ? `steps(${n})` : `steps(${n}, ${pos === 'jump-start' && /start\s*\)$/i.test(text) && !/jump/i.test(text) ? 'start' : pos})`;
        return { text: shown, fn: steps(n, pos) };
      }
    }
    if (/^linear\(.*\)$/i.test(text)) {
      const points = linearPoints(text);
      if (points) {
        const fn = x => {
          let i = 0;
          if (x <= points[0].x) i = 0;
          else if (x >= points[points.length - 1].x) i = points.length - 2;
          else while (i < points.length - 2 && !(x < points[i + 1].x)) i++;
          // The last point at an input wins (a step).
          if (x >= points[points.length - 1].x) return points[points.length - 1].y;
          const a = points[i], b = points[i + 1];
          if (b.x === a.x) return b.y;
          return a.y + (b.y - a.y) * (x - a.x) / (b.x - a.x);
        };
        return { text: `linear(${points.map(p => `${fmt(p.y)} ${fmt(p.x * 100)}%`).join(', ')})`, fn };
      }
    }
    throw new TypeError(`'${text}' is not a valid value for easing`);
  }
  const LINEAR = parseEasing('linear');

  // ----- timing -------------------------------------------------------------

  const FILLS = ['none', 'forwards', 'backwards', 'both', 'auto'];
  const DIRECTIONS = ['normal', 'reverse', 'alternate', 'alternate-reverse'];
  const toDouble = (v, what) => {
    const n = Number(v);
    if (!Number.isFinite(n)) throw new TypeError(`${what} must be a finite number`);
    return n;
  };

  function defaultTiming() {
    return {
      delay: 0, endDelay: 0, fill: 'auto', iterationStart: 0, iterations: 1, duration: 'auto',
      direction: 'normal', easing: LINEAR,
    };
  }

  // Validates `input` (an EffectTiming dictionary or a duration) and merges
  // it into `timing` only when every member is valid.
  function updateTimingFrom(timing, input, allowNumber) {
    const next = { ...timing };
    if (allowNumber && !isObject(input)) {
      if (input === undefined) return timing;
      const n = Number(input);
      if (Number.isNaN(n) || n < 0) throw new TypeError('duration must be a non-negative number');
      next.duration = n;
      return next;
    }
    if (input === undefined || input === null) return next;
    if (!isObject(input)) throw new TypeError('The provided value is not of type EffectTiming');
    const has = k => input[k] !== undefined;
    // Read in WebIDL dictionary (alphabetical) order.
    if (has('delay')) next.delay = toDouble(input.delay, 'delay');
    if (has('direction')) {
      const d = String(input.direction);
      if (!DIRECTIONS.includes(d)) throw new TypeError(`'${d}' is not a valid value for direction`);
      next.direction = d;
    }
    if (has('duration')) {
      const d = input.duration;
      if (typeof d === 'string') {
        if (d !== 'auto') throw new TypeError(`'${d}' is not a valid value for duration`);
        next.duration = 'auto';
      } else {
        const n = Number(d);
        if (Number.isNaN(n) || n < 0) throw new TypeError('duration must be a non-negative number');
        next.duration = n;
      }
    }
    if (has('easing')) next.easing = parseEasing(input.easing);
    if (has('endDelay')) next.endDelay = toDouble(input.endDelay, 'endDelay');
    if (has('fill')) {
      const f = String(input.fill);
      if (!FILLS.includes(f)) throw new TypeError(`'${f}' is not a valid value for fill`);
      next.fill = f;
    }
    if (has('iterationStart')) {
      const n = toDouble(input.iterationStart, 'iterationStart');
      if (n < 0) throw new TypeError('iterationStart must be non-negative');
      next.iterationStart = n;
    }
    if (has('iterations')) {
      const n = Number(input.iterations);
      if (Number.isNaN(n) || n < 0) throw new TypeError('iterations must be non-negative');
      next.iterations = n;
    }
    return next;
  }

  const iterationDuration = t => (t.duration === 'auto' ? 0 : t.duration);
  function activeDuration(t) {
    const d = iterationDuration(t);
    return d === 0 || t.iterations === 0 ? 0 : d * t.iterations;
  }
  const endTime = t => Math.max(t.delay + activeDuration(t) + t.endDelay, 0);

  // The effect's computed timing at `localTime` for playback `rate`.
  function computeTiming(t, localTime, rate) {
    const active = activeDuration(t);
    const end = endTime(t);
    const fill = t.fill === 'auto' ? 'none' : t.fill;
    const out = { phase: 'none', activeTime: null, progress: null, currentIteration: null, end, active };
    if (localTime === null) return out;
    const backwards = rate < 0;
    const beforeActive = Math.max(Math.min(t.delay, end), 0);
    const activeAfter = Math.max(Math.min(t.delay + active, end), 0);
    if (localTime < beforeActive || (backwards && localTime === beforeActive)) out.phase = 'before';
    else if (localTime > activeAfter || (!backwards && localTime === activeAfter)) out.phase = 'after';
    else out.phase = 'active';
    let activeTime = null;
    if (out.phase === 'before') {
      if (fill === 'backwards' || fill === 'both') activeTime = Math.max(localTime - t.delay, 0);
    } else if (out.phase === 'active') {
      activeTime = localTime - t.delay;
    } else if (fill === 'forwards' || fill === 'both') {
      activeTime = Math.max(Math.min(localTime - t.delay, active), 0);
    }
    out.activeTime = activeTime;
    if (activeTime === null) return out;
    const duration = iterationDuration(t);
    let overall;
    if (duration === 0) overall = out.phase === 'before' ? t.iterationStart : t.iterationStart + t.iterations;
    else overall = activeTime / duration + t.iterationStart;
    let simple = overall === Infinity ? t.iterationStart % 1 : overall % 1;
    if (simple === 0 && (out.phase === 'active' || out.phase === 'after') && activeTime === active &&
        t.iterations !== 0 && overall !== 0) {
      simple = 1;
    }
    let iteration;
    if (out.phase === 'after' && t.iterations === Infinity) iteration = Infinity;
    else if (simple === 1) iteration = Math.floor(overall) - 1;
    else iteration = Math.floor(overall);
    out.currentIteration = iteration;
    let forwards = true;
    if (t.direction === 'reverse') forwards = false;
    else if (t.direction === 'alternate' || t.direction === 'alternate-reverse') {
      let d = iteration;
      if (t.direction === 'alternate-reverse') d += 1;
      forwards = d === Infinity || d % 2 === 0;
    }
    const directed = forwards ? simple : 1 - simple;
    const before = (out.phase === 'before' && !backwards) || (out.phase === 'after' && backwards);
    out.progress = t.easing.fn(directed, before);
    return out;
  }

  // ----- keyframes ----------------------------------------------------------

  const COMPOSITES = ['replace', 'add', 'accumulate'];
  // A keyframe member name to its CSS property, or null for anything that is
  // not an animatable property.
  function propertyOf(member) {
    if (member === 'offset' || member === 'easing' || member === 'composite') return null;
    if (member === 'cssFloat') return 'float';
    if (member === 'cssOffset') return 'offset';
    if (member === 'float' || member === 'offset') return null;
    if (member.startsWith('--')) return member;
    if (/[A-Z]/.test(member[0]) || member.includes('-')) return null;
    return cssName(member);
  }
  function stringValue(v) {
    if (typeof v === 'symbol') throw new TypeError('Cannot convert a Symbol value to a string');
    return String(v);
  }
  const offsetValue = v => {
    if (v === null || v === undefined) return null;
    const n = Number(v);
    if (!Number.isFinite(n)) throw new TypeError('offset must be a finite number');
    if (n < 0 || n > 1) throw new TypeError('Offsets must be null or in the range [0,1]');
    return n;
  };
  const compositeValue = v => {
    if (v === undefined || v === null || v === 'auto') return 'auto';
    const s = String(v);
    if (!COMPOSITES.includes(s)) throw new TypeError(`'${s}' is not a valid value for composite`);
    return s;
  };

  // The property members of a keyframe object, in Unicode code point order of
  // their CSS names (as Chrome reads them).
  function propertyMembers(obj) {
    const out = [];
    for (const key of Object.keys(obj)) {
      const prop = propertyOf(key);
      if (prop) out.push({ key, prop });
    }
    out.sort((a, b) => (a.prop < b.prop ? -1 : a.prop > b.prop ? 1 : 0));
    return out;
  }

  // "Process a keyframes argument": the keyframes as
  // `{offset, easing, composite, values: Map(member -> {prop, value})}`.
  function processKeyframes(input) {
    if (input === null || input === undefined) return [];
    if (!isObject(input)) throw new TypeError('Keyframes must be an object or an array');
    const iterator = input[Symbol.iterator];
    let frames = [];
    if (iterator !== undefined && iterator !== null) {
      if (typeof iterator !== 'function') throw new TypeError('@@iterator is not callable');
      for (const kf of { [Symbol.iterator]: () => apply(iterator, input, []) }) {
        if (kf !== null && kf !== undefined && !isObject(kf)) {
          throw new TypeError('Keyframes must be objects');
        }
        const obj = kf || {};
        const values = new Map();
        const offset = offsetValue(obj.offset);
        const easing = parseEasing(obj.easing === undefined ? 'linear' : obj.easing);
        const composite = compositeValue(obj.composite);
        for (const { key, prop } of propertyMembers(obj)) {
          values.set(key, { prop, value: stringValue(obj[key]) });
        }
        frames.push({ offset, easing, composite, values });
      }
      let last = -Infinity;
      for (const f of frames) {
        if (f.offset === null) continue;
        if (f.offset < last) throw new TypeError('Offsets must be monotonically non-decreasing');
        last = f.offset;
      }
    } else {
      // A property-indexed keyframe.
      const list = v => (isObject(v) && typeof v[Symbol.iterator] === 'function' ? Array.from(v) : [v]);
      const rawOffset = input.offset, rawEasing = input.easing, rawComposite = input.composite;
      const byOffset = new Map();
      for (const { key, prop } of propertyMembers(input)) {
        const values = list(input[key]).map(stringValue);
        values.forEach((value, i) => {
          const offset = values.length === 1 ? 1 : i / (values.length - 1);
          if (!byOffset.has(offset)) byOffset.set(offset, new Map());
          byOffset.get(offset).set(key, { prop, value });
        });
      }
      const offsets = [...byOffset.keys()].sort((a, b) => a - b);
      frames = offsets.map(o => ({ offset: null, computed: o, easing: LINEAR, composite: 'auto', values: byOffset.get(o) }));
      const offsetList = rawOffset === undefined ? [] : list(rawOffset).map(offsetValue);
      offsetList.slice(0, frames.length).forEach((o, i) => { frames[i].offset = o; });
      let lastO = -Infinity;
      for (const f of frames) {
        if (f.offset === null) continue;
        if (f.offset < lastO) throw new TypeError('Offsets must be monotonically non-decreasing');
        lastO = f.offset;
      }
      const easings = rawEasing === undefined ? [] : list(rawEasing).map(parseEasing);
      if (easings.length) frames.forEach((f, i) => { f.easing = easings[i % easings.length]; });
      const composites = rawComposite === undefined ? [] : list(rawComposite).map(compositeValue);
      if (composites.length) frames.forEach((f, i) => { f.composite = composites[i % composites.length]; });
      // Without offsets, each keyframe stays where its properties' own
      // lists put it.
      if (offsetList.length === 0) frames.forEach(f => { f.implicit = f.computed; });
    }
    computeOffsets(frames);
    return frames;
  }

  function computeOffsets(frames) {
    frames.forEach(f => { f.computed = f.offset; });
    if (frames.length === 0) return;
    if (frames.every(f => f.implicit !== undefined && f.offset === null)) {
      frames.forEach(f => { f.computed = f.implicit; });
      return;
    }
    if (frames.length > 1 && frames[0].computed === null) frames[0].computed = 0;
    if (frames[frames.length - 1].computed === null) frames[frames.length - 1].computed = 1;
    for (let i = 0; i < frames.length; i++) {
      if (frames[i].computed !== null) continue;
      let j = i;
      while (frames[j].computed === null) j++;
      const a = frames[i - 1].computed, b = frames[j].computed;
      for (let k = i; k < j; k++) frames[k].computed = a + (b - a) * (k - i + 1) / (j - i + 1);
      i = j;
    }
  }

  // ----- interpolation ------------------------------------------------------

  const NUMBER = /[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?/gi;
  // Values whose text matches but for their numbers interpolate number by
  // number (lengths of one unit, opacity, transform functions, rgb()); any
  // other pair is discrete, flipping at the midpoint.
  function interpolate(from, to, p, prop) {
    if (p === 0) return from;
    if (p === 1) return to;
    if (prop === 'visibility') {
      if (from === 'visible' || to === 'visible') return p <= 0 ? from : p >= 1 ? to : 'visible';
    }
    const a = from.match(NUMBER) || [], b = to.match(NUMBER) || [];
    if (a.length && a.length === b.length && from.replace(NUMBER, '#') === to.replace(NUMBER, '#')) {
      let i = 0;
      return from.replace(NUMBER, () => {
        const x = +a[i], y = +b[i];
        i++;
        return fmt(x + (y - x) * p);
      });
    }
    return p < 0.5 ? from : to;
  }

  function underlying(target, prop) {
    try {
      return apply(getPropertyValue, computedStyle(target), [prop]);
    } catch {
      return '';
    }
  }

  // The animated value of each property of `effect` at its current progress:
  // Map(prop -> value). Empty when the effect is not in effect.
  function animatedValues(effect) {
    const out = new Map();
    const timing = computedTimingOf(effect);
    if (timing.progress === null || !effect.target) return out;
    const p = timing.progress;
    const props = new Map();
    for (const f of effect.frames) {
      for (const { prop, value } of f.values.values()) {
        if (!props.has(prop)) props.set(prop, []);
        props.get(prop).push({ offset: f.computed, value, easing: f.easing });
      }
    }
    for (const [prop, frames] of props) {
      let list = frames;
      const base = () => underlying(effect.target, prop);
      if (list[0].offset !== 0) list = [{ offset: 0, value: base(), easing: LINEAR }, ...list];
      if (list[list.length - 1].offset !== 1) list = [...list, { offset: 1, value: base(), easing: LINEAR }];
      let start, end;
      if (p < 0 && list.filter(f => f.offset === 0).length > 1) {
        start = end = list.find(f => f.offset === 0);
      } else if (p >= 1 && list.filter(f => f.offset === 1).length > 1) {
        start = end = list.filter(f => f.offset === 1).pop();
      } else {
        let i = 0;
        if (p >= 1) i = list.length - 2;
        else {
          for (let k = 0; k < list.length - 1; k++) {
            if (list[k].offset <= p && list[k + 1].offset > p) { i = k; break; }
            if (p < 0) { i = 0; break; }
          }
        }
        start = list[i];
        end = list[i + 1];
      }
      if (start === end) {
        out.set(prop, start.value);
        continue;
      }
      const span = end.offset - start.offset;
      const local = span === 0 ? 0 : (p - start.offset) / span;
      out.set(prop, interpolate(start.value, end.value, start.easing.fn(local, false), prop));
    }
    return out;
  }

  // ----- timelines ----------------------------------------------------------

  const timelines = new WeakMap();
  class AnimationTimeline {
    constructor() { throw illegal(); }
    get currentTime() {
      const st = brand(timelines, this, 'an AnimationTimeline');
      return now() - st.origin;
    }
  }
  class DocumentTimeline extends AnimationTimeline {
    constructor(options) {
      // AnimationTimeline's constructor is not callable: skip it.
      const self = Reflect.construct(Object, [], new.target);
      let origin = 0;
      if (options !== undefined && options !== null) {
        if (!isObject(options)) throw new TypeError('The provided value is not of type DocumentTimelineOptions');
        if (options.originTime !== undefined) origin = toDouble(options.originTime, 'originTime');
      }
      timelines.set(self, { origin });
      return self;
    }
  }
  Object.defineProperty(AnimationTimeline.prototype, Symbol.toStringTag, { value: 'AnimationTimeline', configurable: true });
  Object.defineProperty(DocumentTimeline.prototype, Symbol.toStringTag, { value: 'DocumentTimeline', configurable: true });
  const documentTimeline = new DocumentTimeline();
  const timelineTime = tl => (tl === null ? null : tl.currentTime);

  // ----- effects --------------------------------------------------------------

  const effects = new WeakMap();
  class AnimationEffect {
    constructor() { throw illegal(); }
    getTiming() {
      const t = brand(effects, this, 'an AnimationEffect').timing;
      return {
        delay: t.delay, endDelay: t.endDelay, fill: t.fill, iterationStart: t.iterationStart,
        iterations: t.iterations, duration: t.duration, direction: t.direction, easing: t.easing.text,
      };
    }
    getComputedTiming() {
      const st = brand(effects, this, 'an AnimationEffect');
      const t = st.timing;
      const c = computedTimingOf(st);
      return {
        delay: t.delay, endDelay: t.endDelay, fill: t.fill === 'auto' ? 'none' : t.fill,
        iterationStart: t.iterationStart, iterations: t.iterations, duration: iterationDuration(t),
        direction: t.direction, easing: t.easing.text, endTime: c.end, activeDuration: c.active,
        localTime: c.localTime, progress: c.progress, currentIteration: c.currentIteration,
      };
    }
    updateTiming(timing) {
      const st = brand(effects, this, 'an AnimationEffect');
      st.timing = updateTimingFrom(st.timing, timing, false);
      if (st.animation) timingChanged(st.animation);
    }
  }

  function computedTimingOf(effect) {
    const anim = effect.animation;
    const localTime = anim ? currentTimeOf(anim) : null;
    const rate = anim ? anim.rate : 1;
    const c = computeTiming(effect.timing, localTime, rate);
    c.localTime = localTime;
    return c;
  }

  class KeyframeEffect extends AnimationEffect {
    constructor(target, keyframes, options) {
      const self = Reflect.construct(Object, [], new.target);
      let st;
      if (arguments.length === 1 && effects.has(target) && effects.get(target).keyframe) {
        const src = effects.get(target);
        st = {
          keyframe: true, target: src.target, pseudo: src.pseudo, frames: src.frames, composite: src.composite,
          iterationComposite: src.iterationComposite, timing: { ...src.timing }, animation: null,
        };
      } else {
        if (target !== null && !isElement(target)) {
          throw new TypeError("Failed to construct 'KeyframeEffect': parameter 1 is not of type 'Element'.");
        }
        st = {
          keyframe: true, target, pseudo: null, frames: [], composite: 'replace', iterationComposite: 'replace',
          timing: defaultTiming(), animation: null,
        };
        st.timing = updateTimingFrom(st.timing, options, true);
        if (isObject(options)) {
          if (options.composite !== undefined) {
            const c = String(options.composite);
            if (!COMPOSITES.includes(c)) throw new TypeError(`'${c}' is not a valid value for composite`);
            st.composite = c;
          }
          if (options.pseudoElement !== undefined && options.pseudoElement !== null) {
            st.pseudo = pseudoElement(options.pseudoElement);
          }
        }
        st.frames = processKeyframes(keyframes);
      }
      st.object = self;
      effects.set(self, st);
      return self;
    }
    get target() { return brand(effects, this, 'a KeyframeEffect').target; }
    set target(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      if (value !== null && !isElement(value)) throw new TypeError("The provided value is not of type 'Element'.");
      st.target = value;
    }
    get pseudoElement() { return brand(effects, this, 'a KeyframeEffect').pseudo; }
    set pseudoElement(value) {
      brand(effects, this, 'a KeyframeEffect').pseudo = value === null ? null : pseudoElement(value);
    }
    get composite() { return brand(effects, this, 'a KeyframeEffect').composite; }
    set composite(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      const c = String(value);
      if (COMPOSITES.includes(c)) st.composite = c;
    }
    get iterationComposite() { return brand(effects, this, 'a KeyframeEffect').iterationComposite; }
    set iterationComposite(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      const c = String(value);
      if (COMPOSITES.includes(c)) st.iterationComposite = c;
    }
    getKeyframes() {
      const st = brand(effects, this, 'a KeyframeEffect');
      return st.frames.map(f => {
        const out = { offset: f.offset, computedOffset: f.computed, easing: f.easing.text, composite: f.composite };
        for (const [key, { value }] of f.values) out[key] = value;
        return out;
      });
    }
    setKeyframes(keyframes) {
      brand(effects, this, 'a KeyframeEffect').frames = processKeyframes(keyframes);
    }
  }
  function pseudoElement(value) {
    const s = String(value);
    if (/^::?(before|after|marker|first-line|first-letter|placeholder|selection|backdrop)$/i.test(s)) {
      return s.startsWith('::') ? s.toLowerCase() : ':' + s.toLowerCase();
    }
    throw new DOMEx(`'${s}' is not a valid pseudo-element.`, 'SyntaxError');
  }
  for (const [cls, name] of [[AnimationEffect, 'AnimationEffect'], [KeyframeEffect, 'KeyframeEffect']]) {
    Object.defineProperty(cls.prototype, Symbol.toStringTag, { value: name, configurable: true });
  }

  // ----- events -------------------------------------------------------------

  const eventTimes = new WeakMap();
  class AnimationPlaybackEvent extends Ev {
    constructor(type, init) {
      if (arguments.length < 1) {
        throw new TypeError("Failed to construct 'AnimationPlaybackEvent': 1 argument required, but only 0 present.");
      }
      super(type, init);
      const nullable = v => (v === undefined || v === null ? null : Number(v));
      eventTimes.set(this, {
        currentTime: isObject(init) ? nullable(init.currentTime) : null,
        timelineTime: isObject(init) ? nullable(init.timelineTime) : null,
      });
    }
    get currentTime() { return brand(eventTimes, this, 'an AnimationPlaybackEvent').currentTime; }
    get timelineTime() { return brand(eventTimes, this, 'an AnimationPlaybackEvent').timelineTime; }
  }
  Object.defineProperty(AnimationPlaybackEvent.prototype, Symbol.toStringTag, { value: 'AnimationPlaybackEvent', configurable: true });

  // ----- animations -----------------------------------------------------------

  const animations = new WeakMap();
  // Every animation that may be relevant (not idle), in composite order.
  const live = new Set();
  let sequence = 0;

  function deferred() {
    let resolve, reject;
    const promise = new P((y, n) => { resolve = y; reject = n; });
    return { promise, resolve, reject, settled: false };
  }
  function settle(d, value, rejected) {
    if (d.settled) return;
    d.settled = true;
    if (rejected) {
      apply(promiseThen, d.promise, [undefined, () => {}]);
      d.reject(value);
    } else {
      d.resolve(value);
    }
  }

  function currentTimeOf(a) {
    if (a.holdTime !== null) return a.holdTime;
    const t = timelineTime(a.timeline);
    if (t === null || a.startTime === null) return null;
    return (t - a.startTime) * a.rate;
  }
  const effectiveRate = a => (a.pendingRate !== null ? a.pendingRate : a.rate);
  const endOf = a => (a.effect ? endTime(a.effect.timing) : 0);
  function applyPendingRate(a) {
    if (a.pendingRate === null) return;
    a.rate = a.pendingRate;
    a.pendingRate = null;
  }

  function playStateOf(a) {
    const ct = currentTimeOf(a);
    if (ct === null && a.startTime === null && a.pending === null) return 'idle';
    if (a.pending === 'pause' || (a.startTime === null && a.pending !== 'play')) return 'paused';
    if (ct !== null && ((a.rate > 0 && ct >= endOf(a)) || (a.rate < 0 && ct <= 0))) return 'finished';
    return 'running';
  }

  function silentlySetCurrentTime(a, seek) {
    if (seek === null) {
      if (currentTimeOf(a) !== null) throw new TypeError('The current time cannot be set to null');
      return;
    }
    const tt = timelineTime(a.timeline);
    if (a.holdTime !== null || a.startTime === null || tt === null || a.rate === 0) a.holdTime = seek;
    else a.startTime = tt - seek / a.rate;
    if (tt === null) a.startTime = null;
    a.previousTime = null;
  }

  function cancelPending(a) {
    a.pending = null;
    a.pendingGen++;
  }

  function schedulePending(a, kind) {
    a.pending = kind;
    const gen = ++a.pendingGen;
    schedule(0, () => {
      if (a.pendingGen !== gen || a.pending !== kind) return;
      // The timeline must be active for the task to run.
      if (timelineTime(a.timeline) === null) return;
      if (kind === 'play') runPendingPlay(a); else runPendingPause(a);
    });
  }

  function runPendingPlay(a) {
    const ready = timelineTime(a.timeline);
    if (a.holdTime !== null) {
      applyPendingRate(a);
      a.startTime = a.rate === 0 ? ready : ready - a.holdTime / a.rate;
      if (a.rate !== 0) a.holdTime = null;
    } else if (a.startTime !== null && a.pendingRate !== null) {
      const match = (ready - a.startTime) * a.rate;
      applyPendingRate(a);
      if (a.rate === 0) a.holdTime = match;
      else a.startTime = ready - match / a.rate;
    }
    a.pending = null;
    settle(a.ready, a.self, false);
    updateFinished(a, false, false);
  }

  function runPendingPause(a) {
    const ready = timelineTime(a.timeline);
    if (a.startTime !== null && a.holdTime === null) a.holdTime = (ready - a.startTime) * a.rate;
    applyPendingRate(a);
    a.startTime = null;
    a.pending = null;
    settle(a.ready, a.self, false);
    updateFinished(a, false, false);
  }

  function play(a, autoRewind) {
    const abortedPause = a.pending === 'pause';
    let hasPendingReady = false;
    let seek = null;
    const rate = effectiveRate(a);
    const ct = currentTimeOf(a);
    const end = endOf(a);
    if (autoRewind && rate >= 0 && (ct === null || ct < 0 || ct >= end)) seek = 0;
    else if (autoRewind && rate < 0 && (ct === null || ct <= 0 || ct > end)) {
      if (end === Infinity) throw invalidState('Cannot play reversed Animation with infinite target effect end.');
      seek = end;
    } else if (rate === 0 && ct === null) seek = 0;
    if (seek !== null) a.holdTime = seek;
    if (a.holdTime !== null) a.startTime = null;
    if (a.pending !== null) {
      cancelPending(a);
      hasPendingReady = true;
    }
    if (a.holdTime === null && seek === null && !abortedPause && a.pendingRate === null) return;
    if (!hasPendingReady) a.ready = deferred();
    schedulePending(a, 'play');
    track(a);
    updateFinished(a, false, false);
  }

  function pause(a) {
    if (a.pending === 'pause') return;
    if (playStateOf(a) === 'paused') return;
    let seek = null;
    if (currentTimeOf(a) === null) {
      if (a.rate >= 0) seek = 0;
      else {
        if (endOf(a) === Infinity) throw invalidState('Cannot pause reversed Animation with infinite target effect end.');
        seek = endOf(a);
      }
    }
    if (seek !== null) a.holdTime = seek;
    let hasPendingReady = false;
    if (a.pending === 'play') {
      cancelPending(a);
      hasPendingReady = true;
    }
    if (!hasPendingReady) a.ready = deferred();
    schedulePending(a, 'pause');
    track(a);
    updateFinished(a, false, false);
  }

  function updateFinished(a, didSeek, sync) {
    let unconstrained;
    if (didSeek) unconstrained = currentTimeOf(a);
    else {
      const t = timelineTime(a.timeline);
      unconstrained = t === null || a.startTime === null ? null : (t - a.startTime) * a.rate;
    }
    if (unconstrained !== null && a.startTime !== null && a.pending === null) {
      const end = endOf(a);
      if (a.rate > 0 && unconstrained >= end) {
        a.holdTime = didSeek ? unconstrained : (a.previousTime === null ? end : Math.max(a.previousTime, end));
      } else if (a.rate < 0 && unconstrained <= 0) {
        a.holdTime = didSeek ? unconstrained : (a.previousTime === null ? 0 : Math.min(a.previousTime, 0));
      } else if (a.rate !== 0 && timelineTime(a.timeline) !== null) {
        if (didSeek && a.holdTime !== null) a.startTime = timelineTime(a.timeline) - a.holdTime / a.rate;
        a.holdTime = null;
      }
    }
    a.previousTime = currentTimeOf(a);
    const finished = playStateOf(a) === 'finished';
    if (finished && !a.finished.settled) {
      if (sync) {
        a.finishGen++;
        finishNotification(a);
      } else if (!a.finishQueued) {
        a.finishQueued = true;
        const gen = ++a.finishGen;
        microtask(() => {
          a.finishQueued = false;
          if (a.finishGen === gen) finishNotification(a);
        });
      }
    }
    if (!finished && a.finished.settled) a.finished = deferred();
    // Only animations getAnimations() may list stay referenced from here.
    if (isRelevant(a)) live.add(a.self); else live.delete(a.self);
    plan(a);
  }

  function finishNotification(a) {
    if (playStateOf(a) !== 'finished') return;
    settle(a.finished, a.self, false);
    queueEvent(a, 'finish', currentTimeOf(a));
    removeReplaced(a);
  }

  // Events are dispatched as tasks of their own, after the promise
  // reactions of the step that queued them.
  function queueEvent(a, type, currentTime) {
    const event = new AnimationPlaybackEvent(type, { currentTime, timelineTime: timelineTime(a.timeline) });
    schedule(0, () => { apply(dispatch, a.self, [event]); });
  }

  // One timer at the moment a playing animation reaches its end (or start,
  // backwards), so its finished state updates then; none otherwise.
  function plan(a) {
    const gen = ++a.timerGen;
    if (a.pending !== null || a.holdTime !== null || a.startTime === null || a.rate === 0) return;
    const ct = currentTimeOf(a);
    if (ct === null) return;
    const target = a.rate > 0 ? endOf(a) : 0;
    const ms = (target - ct) / a.rate;
    if (!(ms >= 0) || ms === Infinity) return;
    schedule(ms, () => {
      if (a.timerGen !== gen) return;
      updateFinished(a, false, false);
      // The clock's resolution may put the timer a little early.
      if (a.timerGen === gen + 1 && playStateOf(a) === 'running') {
        const left = (target - currentTimeOf(a)) / a.rate;
        if (left >= 0 && left < 1) schedule(1, () => { if (a.timerGen === gen + 1) updateFinished(a, false, false); });
      }
    });
  }

  function timingChanged(a) {
    updateFinished(a, false, false);
  }

  function track(a) {
    live.add(a.self);
  }

  function cancel(a) {
    if (playStateOf(a) !== 'idle') {
      if (a.pending !== null) {
        cancelPending(a);
        applyPendingRate(a);
        settle(a.ready, abortError(), true);
        a.ready = deferred();
        settle(a.ready, a.self, false);
      }
      settle(a.finished, abortError(), true);
      a.finished = deferred();
      queueEvent(a, 'cancel', null);
    }
    a.holdTime = null;
    a.startTime = null;
    a.timerGen++;
    a.finishGen++;
    live.delete(a.self);
  }

  // The effect is current or in effect: getAnimations() lists it.
  function isRelevant(a) {
    if (!a.effect || playStateOf(a) === 'idle' || a.replaceState === 'removed') return false;
    const c = computedTimingOf(a.effect);
    if (c.activeTime !== null) return true;
    const rate = a.rate;
    return c.phase === 'active' || (c.phase === 'before' && rate >= 0) || (c.phase === 'after' && rate < 0);
  }

  // Animations replaced by later finished, filling ones of the same target
  // (every property they animate covered) are removed (`remove` event)
  // unless persisted.
  function removeReplaced(trigger) {
    const target = trigger.effect && trigger.effect.target;
    if (!target) return;
    const list = [...live].map(o => animations.get(o)).filter(a =>
      a.effect && a.effect.target === target && a.replaceState === 'active' && playStateOf(a) === 'finished' &&
      computedTimingOf(a.effect).activeTime !== null);
    const propsOf = a => new Set(a.effect.frames.flatMap(f => [...f.values.values()].map(v => v.prop)));
    const covered = new Set();
    for (let i = list.length - 1; i >= 0; i--) {
      const a = list[i];
      const props = propsOf(a);
      if (i < list.length - 1 && props.size && [...props].every(p => covered.has(p))) {
        a.replaceState = 'removed';
        live.delete(a.self);
        queueEvent(a, 'remove', currentTimeOf(a));
      }
      for (const p of props) covered.add(p);
    }
  }

  const HANDLERS = ['finish', 'cancel', 'remove'];
  class Animation extends ET {
    constructor(effect, timeline) {
      super();
      if (effect !== undefined && effect !== null && !(effects.has(effect))) {
        throw new TypeError("Failed to construct 'Animation': parameter 1 is not of type 'AnimationEffect'.");
      }
      if (timeline !== undefined && timeline !== null && !timelines.has(timeline)) {
        throw new TypeError("Failed to construct 'Animation': parameter 2 is not of type 'AnimationTimeline'.");
      }
      const a = {
        self: this, id: '', effect: null, timeline: timeline === undefined ? documentTimeline : timeline,
        startTime: null, holdTime: null, rate: 1, pendingRate: null, pending: null, pendingGen: 0, timerGen: 0,
        finishGen: 0, finishQueued: false, previousTime: null, ready: deferred(), finished: deferred(),
        replaceState: 'active', handlers: {}, seq: ++sequence,
      };
      settle(a.ready, this, false);
      animations.set(this, a);
      if (effect) setEffect(a, effects.get(effect));
    }
    get id() { return brand(animations, this, 'an Animation').id; }
    set id(v) { brand(animations, this, 'an Animation').id = String(v); }
    get effect() {
      const a = brand(animations, this, 'an Animation');
      return a.effect ? a.effect.object : null;
    }
    set effect(v) {
      const a = brand(animations, this, 'an Animation');
      if (v !== null && !effects.has(v)) throw new TypeError("The provided value is not of type 'AnimationEffect'.");
      setEffect(a, v === null ? null : effects.get(v));
    }
    get timeline() { return brand(animations, this, 'an Animation').timeline; }
    set timeline(v) {
      const a = brand(animations, this, 'an Animation');
      if (v !== null && !timelines.has(v)) throw new TypeError("The provided value is not of type 'AnimationTimeline'.");
      if (v === a.timeline) return;
      const previous = playStateOf(a);
      const ct = currentTimeOf(a);
      a.timeline = v;
      if (a.startTime !== null) a.holdTime = null;
      if (previous === 'finished' || previous === 'running') {
        if (ct !== null) silentlySetCurrentTime(a, ct);
      }
      updateFinished(a, false, false);
    }
    get startTime() { return brand(animations, this, 'an Animation').startTime; }
    set startTime(v) {
      const a = brand(animations, this, 'an Animation');
      const start = v === null ? null : toDouble(v, 'startTime');
      const tt = timelineTime(a.timeline);
      if (tt === null && start !== null) a.holdTime = null;
      const previous = currentTimeOf(a);
      applyPendingRate(a);
      a.startTime = start;
      if (start !== null) a.holdTime = a.rate !== 0 ? null : previous;
      else a.holdTime = previous;
      if (a.pending !== null) {
        cancelPending(a);
        settle(a.ready, this, false);
      }
      track(a);
      updateFinished(a, true, false);
    }
    get currentTime() { return currentTimeOf(brand(animations, this, 'an Animation')); }
    set currentTime(v) {
      const a = brand(animations, this, 'an Animation');
      const seek = v === null ? null : toDouble(v, 'currentTime');
      silentlySetCurrentTime(a, seek);
      if (a.pending === 'pause') {
        a.holdTime = seek;
        applyPendingRate(a);
        a.startTime = null;
        cancelPending(a);
        settle(a.ready, this, false);
      }
      track(a);
      updateFinished(a, true, false);
    }
    get playbackRate() { return brand(animations, this, 'an Animation').rate; }
    set playbackRate(v) {
      const a = brand(animations, this, 'an Animation');
      const rate = toDouble(v, 'playbackRate');
      a.pendingRate = null;
      const previous = currentTimeOf(a);
      a.rate = rate;
      if (previous !== null) this.currentTime = previous;
      else updateFinished(a, false, false);
    }
    get playState() { return playStateOf(brand(animations, this, 'an Animation')); }
    get replaceState() { return brand(animations, this, 'an Animation').replaceState; }
    get pending() { return brand(animations, this, 'an Animation').pending !== null; }
    get ready() { return brand(animations, this, 'an Animation').ready.promise; }
    get finished() { return brand(animations, this, 'an Animation').finished.promise; }
    get overallProgress() {
      const a = brand(animations, this, 'an Animation');
      const ct = currentTimeOf(a);
      if (!a.effect || ct === null) return null;
      const end = endOf(a);
      if (end === 0) return ct < 0 ? 0 : 1;
      if (end === Infinity) return 0;
      return Math.min(Math.max(ct / end, 0), 1);
    }
    play() { play(brand(animations, this, 'an Animation'), true); }
    pause() { pause(brand(animations, this, 'an Animation')); }
    finish() {
      const a = brand(animations, this, 'an Animation');
      const rate = effectiveRate(a);
      if (rate === 0) throw invalidState('Cannot finish Animation with a playbackRate of 0.');
      if (rate > 0 && endOf(a) === Infinity) throw invalidState('Cannot finish Animation with an infinite target effect end.');
      applyPendingRate(a);
      const limit = a.rate > 0 ? endOf(a) : 0;
      silentlySetCurrentTime(a, limit);
      const tt = timelineTime(a.timeline);
      if (a.startTime === null && tt !== null) a.startTime = tt - limit / a.rate;
      if (a.pending === 'pause' && a.startTime !== null) {
        a.holdTime = null;
        cancelPending(a);
        settle(a.ready, this, false);
      }
      if (a.pending === 'play' && a.startTime !== null) {
        cancelPending(a);
        settle(a.ready, this, false);
      }
      track(a);
      updateFinished(a, true, true);
    }
    cancel() { cancel(brand(animations, this, 'an Animation')); }
    reverse() {
      const a = brand(animations, this, 'an Animation');
      if (timelineTime(a.timeline) === null) throw invalidState('Cannot reverse an animation with no active timeline');
      const original = a.pendingRate;
      a.pendingRate = -effectiveRate(a);
      try {
        play(a, true);
      } catch (e) {
        a.pendingRate = original;
        throw e;
      }
    }
    updatePlaybackRate(v) {
      const a = brand(animations, this, 'an Animation');
      const rate = toDouble(v, 'playbackRate');
      const previous = playStateOf(a);
      a.pendingRate = rate;
      if (a.pending !== null) return;
      if (previous === 'idle' || previous === 'paused') {
        applyPendingRate(a);
        updateFinished(a, false, false);
      } else if (previous === 'finished') {
        const tt = timelineTime(a.timeline);
        const unconstrained = tt !== null && a.startTime !== null ? (tt - a.startTime) * a.rate : null;
        if (rate === 0) a.startTime = tt;
        else if (unconstrained !== null) a.startTime = tt - unconstrained / rate;
        applyPendingRate(a);
        updateFinished(a, false, false);
      } else {
        play(a, false);
      }
    }
    persist() {
      const a = brand(animations, this, 'an Animation');
      if (a.replaceState === 'removed') track(a);
      a.replaceState = 'persisted';
    }
    commitStyles() {
      const a = brand(animations, this, 'an Animation');
      const effect = a.effect;
      if (!effect || !effect.keyframe || effect.pseudo !== null) return;
      const target = effect.target;
      if (!target) return;
      if (!(target instanceof HTMLElement || (typeof SVGElement === 'function' && target instanceof SVGElement))) {
        throw new DOMEx('Target element does not have a style attribute.', 'NoModificationAllowedError');
      }
      if (!isConnected(target) || underlying(target, 'display') === 'none') {
        throw invalidState('Target element is not rendered.');
      }
      const values = animatedValues(effect);
      const style = inlineStyle(target);
      for (const [prop, value] of values) apply(setProperty, style, [prop, value]);
    }
  }
  for (const type of HANDLERS) {
    Object.defineProperty(Animation.prototype, 'on' + type, {
      get() { const h = brand(animations, this, 'an Animation').handlers[type]; return h ? h.fn : null; },
      set(v) {
        const a = brand(animations, this, 'an Animation');
        const fn = isObject(v) ? v : null;
        if (!a.handlers[type]) {
          if (fn === null) return;
          const slot = a.handlers[type] = { fn };
          const self = this;
          apply(addListener, this, [type, function (e) {
            if (typeof slot.fn === 'function') return apply(slot.fn, self, [e]);
          }]);
        }
        a.handlers[type].fn = fn;
      },
      enumerable: true,
      configurable: true,
    });
  }
  Object.defineProperty(Animation.prototype, Symbol.toStringTag, { value: 'Animation', configurable: true });

  function setEffect(a, effect) {
    if (a.effect === effect) return;
    if (effect && effect.animation && effect.animation !== a) {
      const other = effect.animation;
      other.effect = null;
      updateFinished(other, false, false);
    }
    if (a.effect) a.effect.animation = null;
    a.effect = effect;
    if (effect) effect.animation = a;
    updateFinished(a, false, false);
  }

  // CSSAnimation and CSSTransition: the CSS animations and transitions the
  // engine keeps (animation_events.rs), made when getAnimations() first
  // lists them. Their effects have the CSS timing and no keyframes: the
  // layout settles CSS animations at their end state.
  const CSS_TRANSITION = 0, CSS_ANIMATION = 1;
  const cssRuns = new Map();
  class CSSAnimation extends Animation {
    constructor() { throw illegal(); }
    get animationName() { return brand(animations, this, 'a CSSAnimation').cssName; }
  }
  class CSSTransition extends Animation {
    constructor() { throw illegal(); }
    get transitionProperty() { return brand(animations, this, 'a CSSTransition').cssName; }
  }
  function cssRun([id, target, transition, name, start, delay, duration, iterations]) {
    const timing = { delay, duration, iterations, fill: transition ? 'backwards' : 'none' };
    const effect = new KeyframeEffect(target, null, timing);
    const object = Reflect.construct(Animation, [effect, documentTimeline], transition ? CSSTransition : CSSAnimation);
    const a = animations.get(object);
    a.css = transition ? CSS_TRANSITION : CSS_ANIMATION;
    a.cssName = name;
    object.startTime = start;
    return object;
  }
  // The engine's runs now ([id, target, transition, name, start, delay,
  // duration, iterations]); those it dropped (canceled) leave the list.
  function cssSync(runs) {
    const seen = new Set();
    for (const run of runs) {
      seen.add(run[0]);
      if (!cssRuns.has(run[0])) cssRuns.set(run[0], cssRun(run));
    }
    for (const [id, object] of cssRuns) {
      if (seen.has(id)) continue;
      cssRuns.delete(id);
      live.delete(object);
    }
  }
  Object.defineProperty(CSSAnimation.prototype, Symbol.toStringTag, { value: 'CSSAnimation', configurable: true });
  Object.defineProperty(CSSTransition.prototype, Symbol.toStringTag, { value: 'CSSTransition', configurable: true });

  // ----- Animatable and DocumentOrShadowRoot -------------------------------

  function animate(target, keyframes, options) {
    const effect = new KeyframeEffect(target, keyframes, options);
    let timeline = documentTimeline;
    let id = '';
    if (isObject(options)) {
      if (options.id !== undefined) id = String(options.id);
      if (options.timeline !== undefined) timeline = options.timeline;
    }
    const animation = new Animation(effect, timeline);
    animations.get(animation).id = id;
    animation.play();
    return animation;
  }

  // In composite order: the order the animations were made.
  function relevant(filter) {
    const out = [];
    for (const o of [...live]) {
      const a = animations.get(o);
      if (!isRelevant(a)) {
        live.delete(o);
        continue;
      }
      const target = a.effect.target;
      if (target && filter(target, a)) out.push(a);
    }
    // CSS transitions, then CSS animations, then script animations.
    const rank = a => (a.css === undefined ? 2 : a.css);
    out.sort((x, y) => rank(x) - rank(y) || x.seq - y.seq);
    return out.map(a => a.self);
  }

  function elementAnimations(el, options) {
    let subtree = false;
    if (isObject(options) && options.subtree !== undefined) subtree = !!options.subtree;
    if (!isConnected(el)) return [];
    return relevant((target, a) => {
      if (target === el) return a.effect.pseudo === null || subtree;
      return subtree && apply(contains, el, [target]);
    });
  }

  function rootAnimations(root) {
    return relevant(target => isConnected(target) && apply(getRootNode, target, []) === root);
  }

  return {
    Animation, AnimationEffect, KeyframeEffect, AnimationTimeline, DocumentTimeline, AnimationPlaybackEvent,
    CSSAnimation, CSSTransition,
    animate, elementAnimations, rootAnimations, cssSync, timeline: () => documentTimeline,
  };
})
