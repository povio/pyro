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
// Animated values reach the cascade through the engine: every change of an
// animation's state is reported to it (`report`, animation_frame.rs), which
// samples the keyframes into the layout engine's cascade at the time a
// layout, style read or capture happens. A CSSAnimation or CSSTransition is
// reported once script controls it (until then the engine's own timeline of
// the CSS animation or transition is the same). `commitStyles()` writes the
// animated values (computed here, see `animatedValues`) into the inline
// style.
(native => {
  'use strict';
  const { now, schedule: scheduleTask, cssName, report: reportState, reportKey } = native;
  // (Counts the tasks asked for, so that a pending task may join the one queued last only while no
  // other task was queued after it: the order of tasks stays as it was asked for.)
  let scheduled = 0;
  const schedule = (ms, fn) => { scheduled++; return scheduleTask(ms, fn); };
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
  const comparePosition = Node.prototype.compareDocumentPosition;
  const htmlInlineStyle = getter(HTMLElement.prototype, 'style');
  const SVGElement_ = typeof SVGElement === 'function' ? SVGElement : null;
  const svgInlineStyle = SVGElement_ !== null ? getter(SVGElement_.prototype, 'style') : null;
  // The element's own `style` getter: HTMLElement's rejects an SVG element.
  const inlineStyle = el => (SVGElement_ !== null && el instanceof SVGElement_ ? svgInlineStyle : htmlInlineStyle)(el);
  const setProperty = CSSStyleDeclaration.prototype.setProperty;
  const getPropertyValue = CSSStyleDeclaration.prototype.getPropertyValue;
  const computedStyle = globalThis.getComputedStyle;
  // An element of any realm (another document's, an iframe's): its native `nodeType`.
  const nodeType = Object.getOwnPropertyDescriptor(Node.prototype, 'nodeType').get;
  const isElement = v => {
    if (v === null || typeof v !== 'object') return false;
    if (v instanceof Element) return true;
    try { return apply(nodeType, v, []) === 1; } catch (_) { return false; }
  };
  const isObject = v => v !== null && (typeof v === 'object' || typeof v === 'function');
  const illegal = () => new TypeError('Illegal constructor');
  const brand = (map, o, what) => {
    const st = map.get(o);
    if (!st) throw new TypeError(`Illegal invocation: not ${what}`);
    return st;
  };
  const invalidState = msg => new DOMEx(msg, 'InvalidStateError');
  const requestFrame = typeof globalThis.requestAnimationFrame === 'function' ? globalThis.requestAnimationFrame : null;
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
    // CSS escapes in the keyword/function name ('Ease\2d in-out').
    const text = String(value).trim().replace(/\\([0-9a-f]{1,6})[ \t\n\r\f]?|\\([^0-9a-f\n])/gi, (_, hex, ch) => {
      if (ch !== undefined) return ch;
      const cp = parseInt(hex, 16);
      return cp === 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff) ? '\ufffd' : String.fromCodePoint(cp);
    });
    const lower = text.toLowerCase();
    if (lower === 'linear') return { text: 'linear', fn: x => x };
    if (lower in KEYWORDS) {
      const parsed = parseEasing(KEYWORDS[lower]);
      // Chrome serializes the step keywords as their steps() equivalents.
      return { text: lower === 'step-start' || lower === 'step-end' ? parsed.text : lower, fn: parsed.fn };
    }
    let m = BEZIER.exec(text);
    if (m) {
      const [x1, y1, x2, y2] = m.slice(1).map(Number);
      if (x1 >= 0 && x1 <= 1 && x2 >= 0 && x2 <= 1) {
        return { text: `cubic-bezier(${x1}, ${y1}, ${x2}, ${y2})`, fn: bezier(x1, y1, x2, y2) };
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

  // A CSSNumberish timing member: a percentage (progress-based timelines; a CSSUnitValue) or a
  // number of milliseconds. `pct` tells which, so getTiming() hands back what was set.
  const isPercentValue = v => typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue && v.unit === 'percent';
  function timingNumber(v, what) {
    if (isPercentValue(v)) return { n: v.value, pct: true };
    return { n: toDouble(v, what), pct: false };
  }

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
      if (next.pct) next.pct = { ...next.pct, duration: false };
      return next;
    }
    if (input === undefined || input === null) return next;
    if (!isObject(input)) throw new TypeError('The provided value is not of type EffectTiming');
    const has = k => input[k] !== undefined;
    // Read in WebIDL dictionary (alphabetical) order.
    const pct = { ...(timing.pct || {}) };
    next.pct = pct;
    if (has('delay')) { const r = timingNumber(input.delay, 'delay'); next.delay = r.n; pct.delay = r.pct; }
    if (has('direction')) {
      const d = String(input.direction);
      if (!DIRECTIONS.includes(d)) throw new TypeError(`'${d}' is not a valid value for direction`);
      next.direction = d;
    }
    if (has('duration')) {
      const d = input.duration;
      pct.duration = false;
      if (isPercentValue(d)) {
        if (!(d.value >= 0)) throw new TypeError('duration must be a non-negative number');
        next.duration = d.value;
        pct.duration = true;
      } else if (typeof d === 'string') {
        if (d !== 'auto') throw new TypeError(`'${d}' is not a valid value for duration`);
        next.duration = 'auto';
      } else {
        const n = Number(d);
        if (Number.isNaN(n) || n < 0) throw new TypeError('duration must be a non-negative number');
        next.duration = n;
      }
    }
    if (has('easing')) next.easing = parseEasing(input.easing);
    if (has('endDelay')) { const r = timingNumber(input.endDelay, 'endDelay'); next.endDelay = r.n; pct.endDelay = r.pct; }
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
  // (`bounds`: on a progress-based timeline, whether its scroll position is at the start and at the
  // end of its range, where the active interval's boundary on that side is inclusive: Web
  // Animations 2, "at progress timeline boundary".)
  function computeTiming(t, localTime, rate, bounds) {
    const active = activeDuration(t);
    const end = endTime(t);
    const fill = t.fill === 'auto' ? 'none' : t.fill;
    const out = { phase: 'none', activeTime: null, progress: null, currentIteration: null, end, active };
    if (localTime === null) return out;
    const backwards = rate < 0;
    const beforeActive = Math.max(Math.min(t.delay, end), 0);
    const activeAfter = Math.max(Math.min(t.delay + active, end), 0);
    const [atStart, atEnd] = bounds || [false, false];
    const at = (a, b) => (atStart || atEnd ? Math.abs(a - b) < 1e-9 : a === b);
    if (localTime < beforeActive || (backwards && at(localTime, beforeActive) && !atStart)) out.phase = 'before';
    else if (localTime > activeAfter || (!backwards && at(localTime, activeAfter) && !atEnd)) out.phase = 'after';
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
    out.directed = directed;
    let before = (out.phase === 'before' && !backwards) || (out.phase === 'after' && backwards);
    // The limit side follows the current direction: when the directed progress runs backwards the
    // before/after phases approach from the other side.
    if (!forwards && out.phase !== 'active') before = !before;
    out.before = before;
    out.progress = t.easing.fn(directed, before);
    return out;
  }

  // ----- keyframes ----------------------------------------------------------

  const COMPOSITES = ['replace', 'add', 'accumulate'];
  // IterationCompositeOperation (Web Animations 2): `add` is a composite operation only.
  const ITERATION_COMPOSITES = ['replace', 'accumulate'];
  // Properties no animation type is defined for (CSS Animations / Transitions, contain, writing
  // mode and the like): members naming them are not read when processing keyframes.
  const NOT_ANIMATABLE = /^(?:animation|transition|scroll-timeline|view-timeline|timeline-scope)(?:-|$)|^(?:contain|direction|text-combine-upright|text-orientation|unicode-bidi|will-change|writing-mode)$/;

  // A keyframe member name to its CSS property, or null for anything that is
  // not an animatable property.
  function propertyOf(member) {
    if (member === 'offset' || member === 'easing' || member === 'composite') return null;
    if (member === 'cssFloat') return 'float';
    if (member === 'cssOffset') return 'offset';
    if (member === 'float' || member === 'offset') return null;
    if (member.startsWith('--')) return member;
    if (/[A-Z]/.test(member[0]) || member.includes('-')) return null;
    const name = cssName(member);
    return name !== null && NOT_ANIMATABLE.test(name) ? null : name;
  }
  function stringValue(v) {
    if (typeof v === 'symbol') throw new TypeError('Cannot convert a Symbol value to a string');
    return String(v);
  }
  // A keyframe offset at a position of a timeline range (`cover 25%`, Scroll-driven Animations 1
  // "named range keyframes"): `{rangeName, pct}`; its computed offset depends on the animation's
  // timeline (`frameOffset`).
  const KEYFRAME_RANGE_NAMES = ['cover', 'contain', 'entry', 'exit', 'entry-crossing', 'exit-crossing'];
  const isRangeOffset = v => v !== null && typeof v === 'object' && typeof v.rangeName === 'string';
  const NUMBER_TEXT = '[+-]?(?:\\d+\\.?\\d*|\\.\\d+)(?:e[+-]?\\d+)?';
  const RANGE_TEXT = new RegExp(`^(${KEYFRAME_RANGE_NAMES.join('|')})\\s+(${NUMBER_TEXT})%$`, 'i');
  const PERCENT_TEXT = new RegExp(`^(${NUMBER_TEXT})%$`, 'i');
  // `calc()` of numbers and percentages joined by + and -: its number.
  function calcOffset(text) {
    const m = /^calc\((.*)\)$/i.exec(text);
    if (!m) return null;
    const re = new RegExp(`\\s*([+-])?\\s*(${NUMBER_TEXT})(%?)\\s*`, 'iy');
    let sum = 0, at = 0, first = true;
    while (at < m[1].length) {
      re.lastIndex = at;
      const t = re.exec(m[1]);
      if (!t || (!first && !t[1])) return null;
      const value = Number(t[2]) / (t[3] ? 100 : 1);
      sum += t[1] === '-' ? -value : value;
      at = re.lastIndex;
      first = false;
    }
    return first ? null : sum;
  }
  const offsetValue = v => {
    if (v === null || v === undefined) return null;
    if (isObject(v) && !(typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue)) {
      // A TimelineRangeOffset dictionary `{rangeName, offset}`.
      if (!('rangeName' in v)) {
        const n = Number(v);
        if (!Number.isFinite(n)) throw new TypeError('offset must be a finite number');
        if (n < 0 || n > 1) throw new TypeError('Offsets must be null or in the range [0,1]');
        return n;
      }
      const rangeName = String(v.rangeName), offset = v.offset;
      if (!KEYFRAME_RANGE_NAMES.includes(rangeName)) throw new TypeError(`'${rangeName}' is not a valid timeline range name`);
      if (!isPercentValue(offset)) throw new TypeError('A timeline range offset must be a percentage');
      return { rangeName, pct: offset.value };
    }
    if (typeof v === 'string') {
      const text = v.trim();
      let m = RANGE_TEXT.exec(text);
      if (m) return { rangeName: m[1].toLowerCase(), pct: Number(m[2]) };
      // (A string is a <number>, a <percentage> or a calc() of them.)
      const n = PERCENT_TEXT.test(text) ? parseFloat(text) / 100 : calcOffset(text);
      if (n !== null) {
        if (n < 0 || n > 1) throw new TypeError('Offsets must be null or in the range [0,1]');
        return n;
      }
      if (/[^\d\s.eE+-]/.test(text) || text === '') throw new TypeError('offset must be a finite number');
    }
    if (typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue) {
      if (v.unit !== 'number' && v.unit !== 'percent') throw new TypeError('offset must be a number or a percentage');
      v = v.unit === 'percent' ? v.value / 100 : v.value;
    }
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
    let easingError = null;
    if (iterator !== undefined && iterator !== null) {
      if (typeof iterator !== 'function') throw new TypeError('@@iterator is not callable');
      for (const kf of { [Symbol.iterator]: () => apply(iterator, input, []) }) {
        if (kf !== null && kf !== undefined && !isObject(kf)) {
          throw new TypeError('Keyframes must be objects');
        }
        const obj = kf || {};
        const values = new Map();
        // Dictionary members are read in lexicographic order: composite, easing, offset.
        const composite = compositeValue(obj.composite);
        // An invalid easing is reported only after every keyframe and property was read.
        let easing = LINEAR;
        try { const rawEasing = obj.easing; easing = parseEasing(rawEasing === undefined ? 'linear' : rawEasing); } catch (e) { easingError ??= e; }
        const offset = offsetValue(obj.offset);
        for (const { key, prop } of propertyMembers(obj)) {
          values.set(key, { prop, value: stringValue(obj[key]) });
        }
        frames.push({ offset, easing, composite, values });
      }
      if (easingError !== null) throw easingError;
      let last = -Infinity;
      for (const f of frames) {
        if (f.offset === null || isRangeOffset(f.offset)) continue;
        if (f.offset < last) throw new TypeError('Offsets must be monotonically non-decreasing');
        last = f.offset;
      }
    } else {
      // A property-indexed keyframe.
      const list = v => (isObject(v) && typeof v[Symbol.iterator] === 'function' ? Array.from(v) : [v]);
      const rawComposite = input.composite, rawEasing = input.easing, rawOffset = input.offset;
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
        if (f.offset === null || isRangeOffset(f.offset)) continue;
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

  function computeOffsets(allFrames) {
    allFrames.forEach(f => { f.computed = isRangeOffset(f.offset) ? null : f.offset; });
    if (allFrames.length === 0) return;
    if (allFrames.every(f => f.implicit !== undefined && f.offset === null)) {
      allFrames.forEach(f => { f.computed = f.implicit; });
      return;
    }
    // The keyframes at timeline range offsets take no part in spreading the others' offsets (their
    // own depend on the animation's timeline: `frameOffset`).
    const frames = allFrames.filter(f => !isRangeOffset(f.offset));
    if (frames.length === 0) return;
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

  // A frame's computed offset for `effect`: a timeline range offset resolves against the
  // animation's view timeline and attachment range (a fraction of it, outside 0..1 for positions
  // beyond it); null when it cannot (no such timeline, inactive, a zero-length range).
  function frameOffset(effect, f) {
    if (!isRangeOffset(f.offset)) return f.computed;
    const anim = effect.animation;
    if (!anim || !progressBased(anim)) return null;
    const st = timelines.get(anim.timeline);
    if (st.subject === null) return null;
    const bounds = rangeOffsets(anim);
    const at = rangeBoundary(st, f.offset.rangeName, f.offset.pct);
    if (bounds === null || at === null || !(bounds[1] > bounds[0])) return null;
    return (at - bounds[0]) / (bounds[1] - bounds[0]);
  }
  // The frames of `effect` with their computed offsets, in offset order, without the ones that
  // have none (a range offset the effect's timeline cannot resolve).
  function sampledFrames(effect) {
    if (!effect.frames.some(f => isRangeOffset(f.offset))) return effect.frames.map(f => ({ frame: f, offset: f.computed }));
    return effect.frames.map(f => ({ frame: f, offset: frameOffset(effect, f) })).filter(x => x.offset !== null)
      .sort((a, b) => a.offset - b.offset);
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
    for (const { frame: f, offset } of sampledFrames(effect)) {
      for (const { prop, value } of f.values.values()) {
        if (!props.has(prop)) props.set(prop, []);
        props.get(prop).push({ offset, value, easing: f.easing });
      }
    }
    for (const [prop, frames] of props) {
      let list = frames;
      const base = () => underlying(effect.target, prop);
      if (!list.some(f => f.offset === 0)) list = [...list.filter(f => f.offset < 0), { offset: 0, value: base(), easing: LINEAR }, ...list.filter(f => f.offset >= 0)];
      if (!list.some(f => f.offset === 1)) list = [...list.filter(f => f.offset <= 1), { offset: 1, value: base(), easing: LINEAR }, ...list.filter(f => f.offset > 1)];
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
      out.set(prop, interpolate(start.value, end.value, start.easing.fn(local, timing.before === true), prop));
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
    // A time-based timeline has no fixed duration (progress timelines override this).
    get duration() {
      brand(timelines, this, 'an AnimationTimeline');
      return null;
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
  // (A scroll or view progress timeline's time is a percentage: animations on one stay unresolved
  // for now.)
  const timelineTime = tl => {
    if (tl === null) return null;
    if (!timelines.get(tl).scroll) return tl.currentTime;
    // A scroll or view progress timeline's time: a percentage (the times of animations on it
    // are percentages too, Web Animations 2 "progress-based timelines").
    const t = tl.currentTime;
    return t === null ? null : t.value;
  };
  const progressBased = a => a.timeline !== null && timelines.get(a.timeline).scroll === true;
  // ----- scroll and view progress timelines (Scroll-driven Animations 1) ----

  // A timeline's times are percentages (`CSSUnitValue`s, CSS Typed OM). The scroll position
  // and the subject's place in its scrollport come from the engine's last layout
  // (`timelineState`). Animations do not run on these timelines yet (their time stays
  // unresolved, as on an inactive timeline); CSS animations with `animation-timeline` are
  // driven by the engine.
  const { timelineState, pushScroll } = native;
  const AXES = ['block', 'inline', 'x', 'y'];
  const percent = v => new CSSUnitValue(v, 'percent');
  const pxValue = v => new CSSUnitValue(v, 'px');
  const scrollingElement = () => document.scrollingElement;
  const isScrollTimelineState = st => st && st.scroll === true;
  function axisOf(options, what) {
    if (!isObject(options) || options.axis === undefined) return 'block';
    const axis = String(options.axis);
    if (!AXES.includes(axis)) {
      throw new TypeError(`Failed to construct '${what}': The provided value '${axis}' is not a valid enum value of type ScrollAxis.`);
    }
    return axis;
  }
  // One inset: 'auto', a length or a percentage (a CSSNumericValue or its text): [kind, value]
  // with kind 0 auto, 1 px, 2 fraction of the scrollport.
  function insetPart(v) {
    if (v === undefined || v === null) return [0, 0];
    if (typeof v === 'string') {
      const t = v.trim().toLowerCase();
      if (t === 'auto') return [0, 0];
      let m = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)(px|%)$/.exec(t);
      if (m) return m[2] === 'px' ? [1, Number(m[1])] : [2, Number(m[1]) / 100];
      if (/^[+-]?0$/.test(t)) return [1, 0];
      throw new TypeError(`'${v}' is not a valid inset`);
    }
    if (typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue) {
      if (v.unit === 'px') return [1, v.value];
      if (v.unit === 'percent') return [2, v.value / 100];
      if (v.unit === 'number' && v.value === 0) return [1, 0];
    }
    if (typeof CSSKeywordValue === 'function' && v instanceof CSSKeywordValue && v.value === 'auto') return [0, 0];
    // A math value of one unit type (`new CSSMathNegate(CSS.px(144))`): its px or percent sum.
    if (typeof CSSNumericValue === 'function' && v instanceof CSSNumericValue) {
      try { return [1, v.to('px').value]; } catch { /* not a length */ }
      try { return [2, v.to('percent').value / 100]; } catch { /* not a percentage */ }
    }
    throw new TypeError('Invalid inset');
  }
  function insetOf(options) {
    if (!isObject(options) || options.inset === undefined) return [[0, 0], [0, 0]];
    const inset = options.inset;
    if (typeof inset === 'string') {
      const parts = inset.trim().split(/\s+/).filter(Boolean);
      if (parts.length < 1 || parts.length > 2) throw new TypeError('Invalid inset');
      const start = insetPart(parts[0]);
      return [start, parts.length > 1 ? insetPart(parts[1]) : start];
    }
    if (Array.isArray(inset)) {
      if (inset.length < 1 || inset.length > 2) throw new TypeError('Invalid inset');
      const start = insetPart(inset[0]);
      return [start, inset.length > 1 ? insetPart(inset[1]) : start];
    }
    throw new TypeError('Invalid inset');
  }
  function stateOf(st) {
    const [[sk, sv], [ek, ev]] = st.inset;
    if (st.subject === null && st.source === null) return null;
    return timelineState(st.subject === null ? st.source : null, st.subject, AXES.indexOf(st.axis), sk, sv, ek, ev, st.subjectPseudo || 0);
  }
  // ----- animation ranges (rangeStart / rangeEnd) -------------------------

  const RANGE_NAMES = ['cover', 'contain', 'entry', 'exit', 'entry-crossing', 'exit-crossing', 'scroll'];
  // A range boundary: 'normal', or { rangeName (null: none), offset: [value, unit] } with unit
  // 'percent' or 'px'.
  function parseRange(v, isEnd, what) {
    const dflt = isEnd ? [100, 'percent'] : [0, 'percent'];
    if (v === undefined || v === null) return 'normal';
    const lp = t => {
      const m = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)(%|px)$/i.exec(t);
      if (m) return [Number(m[1]), m[2] === '%' ? 'percent' : 'px'];
      if (/^[+-]?0$/.test(t)) return [0, 'px'];
      return null;
    };
    if (typeof v === 'string') {
      const t = v.trim().toLowerCase();
      if (t === 'normal') return 'normal';
      const parts = t.split(/\s+/);
      if (parts.length === 1) {
        if (RANGE_NAMES.includes(parts[0])) return { rangeName: parts[0], offset: dflt };
        const o = lp(parts[0]);
        if (o) return { rangeName: null, offset: o };
      } else if (parts.length === 2 && RANGE_NAMES.includes(parts[0])) {
        const o = lp(parts[1]);
        if (o) return { rangeName: parts[0], offset: o };
      }
      throw new TypeError(`Failed to set the '${what}' property: '${v}' is not a valid timeline range`);
    }
    if (typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue) {
      if (v.unit === 'percent' || v.unit === 'px') return { rangeName: null, offset: [v.value, v.unit] };
      throw new TypeError(`Failed to set the '${what}' property: invalid unit`);
    }
    if (isObject(v)) {
      const name = v.rangeName === undefined || v.rangeName === null ? null : String(v.rangeName);
      if (name !== null && !RANGE_NAMES.includes(name)) throw new TypeError(`'${name}' is not a valid timeline range name`);
      let offset = dflt;
      if (v.offset !== undefined) {
        if (typeof CSSUnitValue === 'function' && v.offset instanceof CSSUnitValue && (v.offset.unit === 'percent' || v.offset.unit === 'px')) {
          offset = [v.offset.value, v.offset.unit];
        } else throw new TypeError('Invalid range offset');
      }
      return { rangeName: name, offset };
    }
    throw new TypeError(`Failed to set the '${what}' property: invalid timeline range`);
  }
  function rangeValue(r) {
    if (r === 'normal') return 'normal';
    return { rangeName: r.rangeName, offset: new CSSUnitValue(r.offset[0], r.offset[1]) };
  }
  // The animation's attachment range as percentages of its progress-based timeline ([start, end];
  // [0, 100] for 'normal'); null while the timeline is inactive.
  // The place of `pct`% of the named range `rangeName` of view timeline `st`, in percent of the
  // timeline (0: cover start); null while it is inactive.
  function rangeBoundary(st, rangeName, pct) {
    const state = stateOf(st);
    if (state === null) return null;
    const [, start, end, , c0, c1, size] = state;
    const length = end - start;
    if (!(length > 0)) return null;
    const named = {
      cover: [start, end], contain: [c0, c1], entry: [start, c0], exit: [c1, end],
      'entry-crossing': [start, start + size], 'exit-crossing': [end - size, end],
    };
    const [lo, hi] = named[rangeName];
    return (lo + (hi - lo) * pct / 100 - start) / length * 100;
  }
  function rangeOffsets(a) {
    const st = timelines.get(a.timeline);
    const rs = a.rangeStart === undefined ? 'normal' : a.rangeStart;
    const re = a.rangeEnd === undefined ? 'normal' : a.rangeEnd;
    if (rs === 'normal' && re === 'normal') return [0, 100];
    const state = stateOf(st);
    if (state === null) return null;
    const [, start, end, , c0, c1, size] = state;
    const length = end - start;
    if (!(length > 0)) return [0, 100];
    const named = {
      cover: [start, end], contain: [c0, c1], entry: [start, c0], exit: [c1, end],
      'entry-crossing': [start, start + size], 'exit-crossing': [end - size, end], scroll: [start, end],
    };
    const at = (r, dflt) => {
      if (r === 'normal') return dflt;
      const [lo, hi] = r.rangeName === null || st.subject === null ? [start, end] : named[r.rangeName];
      const px = r.offset[1] === 'percent' ? lo + (hi - lo) * r.offset[0] / 100 : lo + r.offset[0];
      return (px - start) / length * 100;
    };
    return [at(rs, 0), at(re, 100)];
  }

  class ScrollTimeline extends AnimationTimeline {
    constructor(options) {
      const self = Reflect.construct(Object, [], new.target);
      if (options !== undefined && options !== null && !isObject(options)) {
        throw new TypeError("Failed to construct 'ScrollTimeline': The provided value is not of type 'ScrollTimelineOptions'.");
      }
      let source = scrollingElement();
      if (isObject(options) && options.source !== undefined) {
        if (options.source !== null && !isElement(options.source)) {
          throw new TypeError("Failed to construct 'ScrollTimeline': Failed to read the 'source' property from 'ScrollTimelineOptions': The provided value is not of type 'Element'.");
        }
        source = options.source;
      }
      const axis = axisOf(options, 'ScrollTimeline');
      timelines.set(self, { scroll: true, source, subject: null, axis, inset: [[0, 0], [0, 0]] });
      return self;
    }
    get source() {
      const st = brand(timelines, this, 'a ScrollTimeline');
      if (st.subject !== null) {
        const state = stateOf(st);
        return state ? (state[3] === null ? scrollingElement() : state[3]) : null;
      }
      return st.source;
    }
    get axis() { return brand(timelines, this, 'a ScrollTimeline').axis; }
    get currentTime() {
      const st = brand(timelines, this, 'a ScrollTimeline');
      if (!isScrollTimelineState(st)) return null;
      const state = stateOf(st);
      if (state === null) return null;
      const [offset, start, end] = state;
      if (!(end > start)) return st.subject === null ? null : percent(offset >= end ? 100 : 0);
      return percent((offset - start) / (end - start) * 100);
    }
    get duration() {
      brand(timelines, this, 'a ScrollTimeline');
      return percent(100);
    }
  }
  class ViewTimeline extends ScrollTimeline {
    constructor(options) {
      const self = Reflect.construct(Object, [], new.target);
      if (options !== undefined && options !== null && !isObject(options)) {
        throw new TypeError("Failed to construct 'ViewTimeline': The provided value is not of type 'ViewTimelineOptions'.");
      }
      let subject = null;
      if (isObject(options) && options.subject !== undefined && options.subject !== null) {
        if (!isElement(options.subject)) {
          throw new TypeError("Failed to construct 'ViewTimeline': Failed to read the 'subject' property from 'ViewTimelineOptions': The provided value is not of type 'Element'.");
        }
        subject = options.subject;
      }
      const axis = axisOf(options, 'ViewTimeline');
      const inset = insetOf(options);
      timelines.set(self, { scroll: true, source: null, subject, axis, inset, view: true });
      return self;
    }
    get subject() { return brand(timelines, this, 'a ViewTimeline').subject; }
    get startOffset() {
      const st = brand(timelines, this, 'a ViewTimeline');
      const state = st.subject === null ? null : stateOf(st);
      return state ? pxValue(state[1]) : null;
    }
    get endOffset() {
      const st = brand(timelines, this, 'a ViewTimeline');
      const state = st.subject === null ? null : stateOf(st);
      return state ? pxValue(state[2]) : null;
    }
  }
  Object.defineProperty(ScrollTimeline.prototype, Symbol.toStringTag, { value: 'ScrollTimeline', configurable: true });
  Object.defineProperty(ViewTimeline.prototype, Symbol.toStringTag, { value: 'ViewTimeline', configurable: true });


  // ----- effects --------------------------------------------------------------

  const effects = new WeakMap();
  class AnimationEffect {
    constructor() { throw illegal(); }
    getTiming() {
      const t = brand(effects, this, 'an AnimationEffect').timing;
      const p = t.pct || {};
      return {
        delay: p.delay ? percent(t.delay) : t.delay, endDelay: p.endDelay ? percent(t.endDelay) : t.endDelay, fill: t.fill,
        iterationStart: t.iterationStart, iterations: t.iterations, duration: p.duration ? percent(t.duration) : t.duration, direction: t.direction, easing: t.easing.text,
      };
    }
    getComputedTiming() {
      const st = brand(effects, this, 'an AnimationEffect');
      const t = timingOn(st);
      const c = computedTimingOf(st);
      const onProgress = st.animation && progressBased(st.animation);
      const time = v => (onProgress && v !== null ? percent(v) : v);
      // (The delays as specified, as Chromium reports them; the rest in percentages.)
      return {
        delay: st.timing.pct && st.timing.pct.delay ? percent(st.timing.delay) : st.timing.delay,
        endDelay: st.timing.pct && st.timing.pct.endDelay ? percent(st.timing.endDelay) : st.timing.endDelay, fill: t.fill === 'auto' ? 'none' : t.fill,
        iterationStart: t.iterationStart, iterations: t.iterations, duration: time(iterationDuration(t)),
        direction: t.direction, easing: t.easing.text, endTime: time(c.end), activeDuration: time(c.active),
        localTime: time(c.localTime), progress: c.progress, currentIteration: c.currentIteration,
      };
    }
    updateTiming(timing) {
      const st = brand(effects, this, 'an AnimationEffect');
      st.timing = updateTimingFrom(st.timing, timing, false);
      if (st.animation) {
        st.animation.scripted = true;
        // (CSS no longer sets the timing properties script set: CSS Animations 2 § 4.2.)
        if (isObject(timing)) {
          for (const f of ['delay', 'endDelay', 'fill', 'iterationStart', 'iterations', 'duration', 'direction', 'easing']) {
            if (timing[f] !== undefined) override(st.animation, 'timing:' + f);
          }
        }
        timingChanged(st.animation);
      }
    }
  }

  // An effect's timing on a progress-based timeline, in percentages of it: an `auto` duration
  // fills it (iterations share it); times are proportions of delay + duration x iterations +
  // end delay (Web Animations 2, "converting time values to proportions").
  function timingOn(effect) {
    const t = effect.timing;
    const anim = effect.animation;
    if (!anim || !progressBased(anim)) return t;
    // (Spread over the animation's attachment range, `rangeStart` to `rangeEnd`.)
    const range = rangeOffsets(anim) || [0, 100];
    const span = Math.max(range[1] - range[0], 0);
    const p = t.pct;
    if (p && (p.delay || p.endDelay || p.duration)) {
      // Percentages are of the attachment range; an `auto` duration is what the delays leave of it.
      const delay = p.delay ? t.delay * span / 100 : t.delay, endDelay = p.endDelay ? t.endDelay * span / 100 : t.endDelay;
      let duration = t.duration;
      if (duration === 'auto') duration = t.iterations > 0 && t.iterations !== Infinity ? Math.max(span - delay - endDelay, 0) / t.iterations : 0;
      else if (p.duration) duration = duration * span / 100;
      return { ...t, delay, endDelay, duration };
    }
    if (t.duration === 'auto') {
      return { ...t, delay: 0, endDelay: 0, duration: t.iterations > 0 && t.iterations !== Infinity ? span / t.iterations : 0 };
    }
    const total = t.delay + t.duration * t.iterations + t.endDelay;
    if (!(total > 0) || total === Infinity) return { ...t, delay: 0, endDelay: 0, duration: 0 };
    const k = span / total;
    return { ...t, delay: t.delay * k, endDelay: t.endDelay * k, duration: t.duration * k };
  }

  function computedTimingOf(effect) {
    const anim = effect.animation;
    const localTime = anim ? currentTimeOf(anim) : null;
    const rate = anim ? anim.rate : 1;
    let bounds = null;
    if (anim && progressBased(anim)) {
      const st = timelines.get(anim.timeline);
      const state = stateOf(st);
      if (state) bounds = [state[0] <= 0, st.subject === null && state[0] >= state[2]];
    }
    const c = computeTiming(timingOn(effect), localTime, rate, bounds);
    c.localTime = localTime;
    return c;
  }

  function keyframeMember(property) {
    if (property.startsWith('--')) return property;
    if (property === 'offset') return 'cssOffset';
    if (property === 'float') return 'cssFloat';
    return property.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  }

  function exposedFrames(st) {
    if (!st.cssFrameSource) return st.cssFrames ?? st.frames;
    const { target, name, easing, pseudo } = st.cssFrameSource;
    const steps = native.animationFrames(target, name, pseudo);
    if (!steps) return st.cssFrames ?? [];
    const frames = [];
    const underlying = new Map();
    for (const [offset, timing, composition, properties] of steps) {
      const frameEasing = timing ?? easing, composite = composition ?? 'auto';
      let frame = frames.find(f => f.offset === offset && f.easing === frameEasing && f.composite === composite);
      if (!frame) {
        frame = { offset, easing: frameEasing, composite };
        frames.push(frame);
      }
      for (const [property, value, base] of properties) {
        const member = keyframeMember(property);
        frame[member] = value;
        underlying.set(member, base);
      }
    }
    if (!underlying.size) return st.cssFrames = [];
    for (const offset of [0, 1]) {
      const missing = [...underlying].filter(([key]) => !frames.some(f => f.offset === offset && Object.hasOwn(f, key)));
      if (missing.length) {
        const endpoint = frames.find(f => f.offset === offset && f.easing === easing &&
          (f.composite === 'auto' || f.composite === 'replace'));
        if (endpoint) Object.assign(endpoint, Object.fromEntries(missing));
        else {
          const neutral = { offset, easing, composite: 'replace', ...Object.fromEntries(missing) };
          const index = frames.findIndex(f => f.offset > offset);
          frames.splice(index < 0 ? frames.length : index, 0, neutral);
        }
      }
    }
    return st.cssFrames = processKeyframes(frames);
  }

  class KeyframeEffect extends AnimationEffect {
    constructor(target, keyframes, options) {
      const self = Reflect.construct(Object, [], new.target);
      let st;
      if (arguments.length === 1 && effects.has(target) && effects.get(target).keyframe) {
        const src = effects.get(target);
        st = {
          keyframe: true, target: src.target, pseudo: src.pseudo, frames: exposedFrames(src), composite: src.composite,
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
          // (Dictionary members in lexicographic order: composite, iterationComposite, pseudoElement.)
          if (options.iterationComposite !== undefined) {
            const c = String(options.iterationComposite);
            if (!ITERATION_COMPOSITES.includes(c)) throw new TypeError(`'${c}' is not a valid value for iterationComposite`);
            st.iterationComposite = c;
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
      effectChanged(st);
    }
    get pseudoElement() { return brand(effects, this, 'a KeyframeEffect').pseudo; }
    set pseudoElement(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      st.pseudo = value === null ? null : pseudoElement(value);
      effectChanged(st);
    }
    get composite() { return brand(effects, this, 'a KeyframeEffect').composite; }
    set composite(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      const c = String(value);
      if (COMPOSITES.includes(c)) st.composite = c;
      effectChanged(st);
    }
    get iterationComposite() { return brand(effects, this, 'a KeyframeEffect').iterationComposite; }
    set iterationComposite(value) {
      const st = brand(effects, this, 'a KeyframeEffect');
      const c = String(value);
      if (ITERATION_COMPOSITES.includes(c)) st.iterationComposite = c;
      effectChanged(st);
    }
    getKeyframes() {
      const st = brand(effects, this, 'a KeyframeEffect');
      return exposedFrames(st).map(f => {
        const out = {
          offset: isRangeOffset(f.offset) ? { rangeName: f.offset.rangeName, offset: percent(f.offset.pct) } : f.offset,
          computedOffset: frameOffset(st, f), easing: f.easing.text, composite: f.composite,
        };
        for (const [key, { prop, value }] of f.values) {
          if (st.cssFrames) out[key] = value;
          else {
            const serialized = native.keyframeValue(prop, value);
            if (serialized !== null) out[key] = serialized;
          }
        }
        return out;
      });
    }
    setKeyframes(keyframes) {
      const st = brand(effects, this, 'a KeyframeEffect');
      st.frames = processKeyframes(keyframes);
      st.cssFrames = null;
      st.cssFrameSource = null;
      effectChanged(st);
    }
  }
  // The effect's animation draws something else now.
  function effectChanged(st) {
    if (!st.animation) return;
    st.animation.scripted = true;
    report(st.animation);
  }
  function pseudoElement(value) {
    const s = String(value);
    if (/^::view-transition(?:$|-)/i.test(s) && native.validViewPseudo(s)) return s;
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
      const nullable = v => (v === undefined || v === null ? null : (typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue ? v : Number(v)));
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
    if (a.holdTime !== null) return a.holdTime + 0;
    const t = timelineTime(a.timeline);
    if (t === null || a.startTime === null) return null;
    return (t - a.startTime) * a.rate + 0;
  }
  const effectiveRate = a => (a.pendingRate !== null ? a.pendingRate : a.rate);
  const endOf = a => (a.effect ? endTime(timingOn(a.effect)) : 0);
  function applyPendingRate(a) {
    if (a.pendingRate === null) return;
    a.rate = a.pendingRate;
    a.pendingRate = null;
  }

  function playStateOf(a) {
    const ct = currentTimeOf(a);
    if (ct === null && a.startTime === null && a.pending === null) return 'idle';
    if (a.pending === 'pause' || (a.startTime === null && a.pending !== 'play')) return 'paused';
    if (ct !== null && ((effectiveRate(a) > 0 && ct >= endOf(a)) || (effectiveRate(a) < 0 && ct <= 0))) return 'finished';
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

  function schedulePending(a, kind, generationTime) {
    a.pending = kind;
    const gen = ++a.pendingGen;
    let tries = 0;
    const attempt = () => {
      if (a.pendingGen !== gen || a.pending !== kind) return;
      // A CSS run's pending task first takes the engine's state: a style change event may have
      // canceled it (a transition then goes idle with `ready` rejected, not resolved here).
      if (a.cssOwner) {
        native.flushPlayback();
        if (a.pendingGen !== gen || a.pending !== kind) return;
      }
      // (On a progress-based timeline the task runs at the frame: the timeline takes its scroll
      // position first.)
      if (progressBased(a)) pushScroll();
      // The timeline must be active for the task to run. A progress-based one may become active
      // later (laid out, shown, made scrollable): tried again at the next frames for a while, and
      // at every scroll (`progressChanged`).
      if (timelineTime(a.timeline) === null) {
        if (progressBased(a) && tries++ < 120) {
          watchProgress(a);
          schedule(16, attempt);
        }
        return;
      }
      if (kind === 'play') runPendingPlay(a, generationTime); else runPendingPause(a);
    };
    // A progress-based timeline's pending task runs after the next frame, as the timeline moves.
    // (At the start of that frame, before its callbacks that script requested later, as the
    // frame's "update animations" step.)
    if (progressBased(a) && requestFrame !== null) apply(requestFrame, globalThis, [attempt]);
    else if (pendingBatch !== null && pendingBatch.at === scheduled) pendingBatch.run.push(attempt);
    else {
      // Pending play and pause tasks of animations made together run together, in one task that
      // reads the timeline once: they start at one time, as the animations made in one frame do
      // when the next "update animations and send events" runs their tasks (Web Animations 1,
      // "play an animation": "schedule a task to run as soon as the animation is ready").
      const run = [attempt];
      schedule(0, () => {
        if (pendingBatch !== null && pendingBatch.run === run) pendingBatch = null;
        // (One that throws does not keep the others from running, as separate tasks would not.)
        let failure = null;
        for (const task of run) {
          try { task(); } catch (e) { if (failure === null) failure = { e }; }
        }
        if (failure !== null) throw failure.e;
      });
      pendingBatch = { run, at: scheduled };
    }
  }
  let pendingBatch = null;

  // Animations on progress-based timelines whose state may change as the page scrolls (their
  // finished state, a pending task waiting for an active timeline): looked at again after each
  // scroll, as a browser does at the frame after it.
  const progressWatched = new Set();
  let progressListening = false;
  let progressQueued = false;
  function watchProgress(a) {
    progressWatched.add(a);
    if (progressListening) return;
    progressListening = true;
    apply(addListener, globalThis, ['scroll', progressChanged, { capture: true, passive: true }]);
  }
  function progressChanged() {
    if (progressQueued) return;
    progressQueued = true;
    // After the next frame: a scroll moves the timelines then (their times hold still until it).
    const later = fn => (requestFrame !== null ? apply(requestFrame, globalThis, [() => schedule(0, fn)]) : schedule(16, fn));
    later(() => {
      progressQueued = false;
      // (A frame passed since the scroll: scroll timelines take its scroll position.)
      pushScroll();
      for (const a of [...progressWatched]) {
        if (a.pending !== null) {
          if (timelineTime(a.timeline) !== null) {
            if (a.pending === 'play') runPendingPlay(a); else runPendingPause(a);
          }
          continue;
        }
        // (Its finished state first: one that left its range finishes now.) An idle one stops
        // being watched; a finished one stays (scrolling back plays it again).
        updateFinished(a, false, false);
        if (playStateOf(a) === 'idle' || !a.effect) progressWatched.delete(a);
      }
    });
  }

  function runPendingPlay(a, ready = timelineTime(a.timeline)) {
    // On a progress-based timeline the start time aligns with its start (or end, playing
    // backwards): the animation's progress is the timeline's (Web Animations 2, auto-aligned
    // start time), unless script seeked it.
    if (progressBased(a) && a.autoAlign) {
      applyPendingRate(a);
      const range = rangeOffsets(a) || [0, 100];
      a.startTime = a.rate >= 0 ? range[0] : range[1];
      a.holdTime = null;
      a.autoAlign = false;
      a.pending = null;
      settle(a.ready, a.self, false);
      // (The aligned start time places it as a seek would: past its end it holds the time the
      // timeline gives, not the one it had; Chromium, WPT scroll-animations
      // `view-timelines/range-boundary`.)
      updateFinished(a, true, false);
      return;
    }
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
    // (A progress-based timeline's play always aligns the start time with the timeline.)
    if (progressBased(a) && autoRewind) a.autoAlign = true;
    if (autoRewind && rate >= 0 && (ct === null || ct < 0 || ct >= end)) seek = 0;
    else if (autoRewind && rate < 0 && (ct === null || ct <= 0 || ct > end)) {
      if (end === Infinity) throw invalidState('Cannot play reversed Animation with infinite target effect end.');
      seek = end;
    } else if (rate === 0 && ct === null) seek = 0;
    // (An auto-aligned start time leaves the start and hold times unresolved until the pending
    // play aligns it with an active timeline: on an inactive one the animation shows nothing,
    // Web Animations 2 "play an animation"; WPT scroll-animations view-timelines/
    // view-timeline-on-display-none-element.)
    const aligning = progressBased(a) && a.autoAlign && seek !== null;
    if (seek !== null) a.holdTime = aligning ? null : seek;
    if (a.holdTime !== null || aligning) a.startTime = null;
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
        a.holdTime = (didSeek ? unconstrained : (a.previousTime === null ? 0 : Math.min(a.previousTime, 0))) + 0;
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
        finishNotification(a, didSeek);
      } else if (!a.finishQueued) {
        a.finishQueued = true;
        const gen = ++a.finishGen;
        microtask(() => {
          a.finishQueued = false;
          if (a.finishGen === gen) finishNotification(a, didSeek);
        });
      }
    }
    if (!finished && a.finished.settled) a.finished = deferred();
    if (progressBased(a) && playStateOf(a) !== 'idle') watchProgress(a);
    // Only animations getAnimations() may list stay referenced from here.
    if (isRelevant(a)) live.add(a.self); else live.delete(a.self);
    plan(a);
    report(a);
  }

  // Hands the engine the animation's state (animation_frame.rs), which
  // samples its keyframes into the cascade; `null` once it has no effect and
  // none to come without script.
  function report(a) {
    if (a.viewRun) return;
    if (a.css !== undefined && !a.scripted) return;
    const e = a.effect;
    if (a.key === undefined) {
      if (!e && a.css === undefined) return;
      a.key = reportKey();
    }
    // A CSS animation's or transition's object whose effect script removed shows nothing: its
    // CSS run gives no values (the engine's `Scripted.gone`).
    if (!e) {
      reportState(a.key, [
        null, a.cssId, [], a.startTime, a.holdTime, a.rate, a.timeline && !timelines.get(a.timeline).scroll ? timelines.get(a.timeline).origin : null,
        0, 0, 'none', 0, 1, 0, 'normal', null, 'replace', 1, 0,
      ]);
      return;
    }
    // (An effect on a list item's `::marker`, or on an element's `::before`/`::after`, is
    // reported with the element and the pseudo-element (1, 2, 3): the engine samples it into the
    // pseudo-element's style. Other pseudo-elements' effects show nothing.)
    const pseudo = !e || e.pseudo === null ? 0
      : ({ '::marker': 1, '::before': 2, ':before': 2, '::after': 3, ':after': 3, '::backdrop': 4 })[e.pseudo] || -1;
    const target = e && e.keyframe && pseudo >= 0 ? e.target : null;
    const timing = e && computedTimingOf(e);
    const inEffect = timing && timing.activeTime !== null;
    const coming = a.pending !== null || playStateOf(a) === 'running';
    const gone = !target || a.replaceState === 'removed' || (!inEffect && !coming) ||
      (a.timeline === null && a.holdTime === null);
    // A script animation that shows nothing leaves; a CSS animation's or
    // transition's object stays, so that its CSS run shows nothing either
    // (canceled, finished without fill, sought out of its interval).
    if (gone && a.css === undefined) {
      reportState(a.key, null);
      return;
    }
    if (!e) return;
    // (On a progress-based timeline: the timing in percentages, and the timeline the engine
    // reads the scroll position of.)
    const t = timingOn(e);
    const tst = a.timeline !== null ? timelines.get(a.timeline) : null;
    const scrollRef = tst && tst.scroll
      ? [tst.subject === null ? tst.source : null, tst.subject, AXES.indexOf(tst.axis), tst.inset[0][0], tst.inset[0][1], tst.inset[1][0], tst.inset[1][1], tst.subject === null && tst.source === null ? 1 : 0, tst.subjectPseudo || 0]
      : null;
    // A CSS animation's or transition's own effect has no keyframes (the
    // engine's are the CSS ones); one script gave it has.
    const frames = sampledFrames(e).map(({ frame: f, offset }) => {
      let text = '';
      for (const { prop, value } of f.values.values()) text += `${prop}: ${value}; `;
      return [offset, f.easing.text === 'linear' ? null : f.easing.text, f.composite === 'auto' ? null : f.composite, text];
    });
    const origin = a.timeline && !timelines.get(a.timeline).scroll ? timelines.get(a.timeline).origin : null;
    reportState(a.key, [
      target, a.css !== undefined ? a.cssId : -1, frames, a.startTime, a.holdTime, a.rate, origin,
      t.delay, t.endDelay, t.fill === 'auto' ? 'none' : t.fill, t.iterationStart, t.iterations,
      iterationDuration(t), t.direction, t.easing.text === 'linear' ? null : t.easing.text, e.composite,
      gone ? 1 : 0, pseudo, scrollRef,
      // A CSS run's object whose keyframes script replaced (even with none).
      a.css !== undefined && e.cssFrames == null && !e.cssFrameSource ? 1 : 0,
      // (Only a keyframe effect has an iteration composite operation; a CSS run's own keeps `replace`.)
      e.iterationComposite === 'accumulate' ? 1 : 0,
    ]);
  }

  function finishNotification(a, seeked) {
    if (playStateOf(a) !== 'finished') return;
    settle(a.finished, a.self, false);
    queueEvent(a, 'finish', currentTimeOf(a));
    // (A finish the timeline's ticking reached replaces right here; one a seek or a timing change
    // caused waits for the next "update animations and send events".)
    if (seeked) replaceAtNextFrame(a); else removeReplaced(a);
  }

  const replaceTriggers = new Set();
  function replaceAtNextFrame(a) {
    if (!a.effect) return;
    if (replaceTriggers.size === 0) {
      const run = () => {
        const list = [...replaceTriggers];
        replaceTriggers.clear();
        for (const t of list) if (t.effect) removeReplaced(t);
      };
      if (requestFrame !== null) apply(requestFrame, globalThis, [run]); else schedule(16, run);
    }
    replaceTriggers.add(a);
  }

  // Events are dispatched as tasks of their own, after the promise
  // reactions of the step that queued them.
  function queueEvent(a, type, currentTime, time = timelineTime(a.timeline)) {
    let tt = time;
    // (On a progress-based timeline the times are percentages: CSSNumberish.)
    if (progressBased(a)) {
      if (currentTime !== null) currentTime = percent(currentTime);
      if (tt !== null) tt = percent(tt);
    }
    const event = new AnimationPlaybackEvent(type, { currentTime, timelineTime: tt });
    schedule(0, () => { apply(dispatch, a.self, [event]); });
  }

  // One timer at the moment a playing animation reaches its end (or start,
  // backwards), so its finished state updates then; none otherwise.
  function plan(a) {
    const gen = ++a.timerGen;
    if (a.pending !== null || a.holdTime !== null || a.startTime === null || a.rate === 0) return;
    // (A progress-based timeline moves with scrolling, not with time.)
    if (progressBased(a)) return;
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
    if (a.effect && playStateOf(a) === 'finished') replaceAtNextFrame(a);

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
    report(a);
  }

  // The effect is current or in effect: getAnimations() lists it.
  function isRelevant(a) {
    if (!a.effect || playStateOf(a) === 'idle' || a.replaceState === 'removed') return false;
    // (On a progress-based timeline it is current while not idle: the timeline moves either way
    // as the page scrolls; Web Animations 2, Chromium.)
    if (progressBased(a)) return true;
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
      computedTimingOf(a.effect).activeTime !== null).sort((x, y) => x.seq - y.seq);
    const propsOf = a => new Set(a.effect.frames.flatMap(f => [...f.values.values()].map(v => v.prop)));
    const covered = new Set();
    const removed = [];
    for (let i = list.length - 1; i >= 0; i--) {
      const a = list[i];
      const props = propsOf(a);
      if (i < list.length - 1 && props.size && [...props].every(p => covered.has(p))) {
        a.replaceState = 'removed';
        live.delete(a.self);
        report(a);
        removed.push(a);
      }
      for (const p of props) covered.add(p);
    }
    // The remove events go out in composite order, after the finish events of the same tick (one
    // more task hop, so the finishes of animations ending together are dispatched first).
    // (Their timeline time is the one of the update that removed them, as the finish events'.)
    for (let i = removed.length - 1; i >= 0; i--) {
      const a = removed[i], ct = currentTimeOf(a), tt = timelineTime(a.timeline);
      schedule(0, () => queueEvent(a, 'remove', ct, tt));
    }
  }

  const HANDLERS = ['finish', 'cancel', 'remove'];
  // A time given to an animation (CSSNumberish): a percentage on a progress-based timeline, else
  // milliseconds (a CSSUnitValue in ms or s, or a number).
  function timeArg(a, v, what) {
    const unit = typeof CSSUnitValue === 'function' && v instanceof CSSUnitValue ? v.unit : null;
    if (progressBased(a)) {
      if (unit === 'percent') return v.value;
      throw new TypeError(`Failed to set the '${what}' property on 'Animation': CSSNumberish must be a percentage for progress-based animations.`);
    }
    if (unit === 'ms' || unit === 'number') return v.value;
    if (unit === 's') return v.value * 1000;
    if (unit !== null) throw new TypeError(`Failed to set the '${what}' property on 'Animation': Invalid unit`);
    return toDouble(v, what);
  }

  // Sets the current time (in the animation's units) as the `currentTime` setter does.
  function seekTo(a, seek) {
    silentlySetCurrentTime(a, seek);
    if (a.pending === 'pause') {
      a.holdTime = seek;
      applyPendingRate(a);
      a.startTime = null;
      cancelPending(a);
      settle(a.ready, a.self, false);
    }
    track(a);
    updateFinished(a, true, false);
  }

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
      a.scripted = true;
      if (v !== null && !effects.has(v)) throw new TypeError("The provided value is not of type 'AnimationEffect'.");
      override(a, 'effect');
      setEffect(a, v === null ? null : effects.get(v));
    }
    get timeline() { return brand(animations, this, 'an Animation').timeline; }
    set timeline(v) {
      const a = brand(animations, this, 'an Animation');
      a.scripted = true;
      if (v !== null && !timelines.has(v)) throw new TypeError("The provided value is not of type 'AnimationTimeline'.");
      override(a, 'timeline');
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
    get startTime() {
      const a = brand(animations, this, 'an Animation');
      return progressBased(a) && a.startTime !== null ? percent(a.startTime) : a.startTime;
    }
    set startTime(v) {
      const a = brand(animations, this, 'an Animation');
      a.scripted = true;
      a.autoAlign = false;
      const start = v === null ? null : timeArg(a, v, 'startTime');
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
    get currentTime() {
      const a = brand(animations, this, 'an Animation');
      const ct = currentTimeOf(a);
      return progressBased(a) && ct !== null ? percent(ct) : ct;
    }
    set currentTime(v) {
      const a = brand(animations, this, 'an Animation');
      a.scripted = true;
      a.autoAlign = false;
      seekTo(a, v === null ? null : timeArg(a, v, 'currentTime'));
    }
    get rangeStart() { return rangeValue(brand(animations, this, 'an Animation').rangeStart || 'normal'); }
    set rangeStart(v) {
      const a = brand(animations, this, 'an Animation');
      a.rangeStart = parseRange(v, false, 'rangeStart');
      override(a, 'rangeStart');
      rangeChanged(a);
    }
    get rangeEnd() { return rangeValue(brand(animations, this, 'an Animation').rangeEnd || 'normal'); }
    set rangeEnd(v) {
      const a = brand(animations, this, 'an Animation');
      a.rangeEnd = parseRange(v, true, 'rangeEnd');
      override(a, 'rangeEnd');
      rangeChanged(a);
    }
    get playbackRate() { return brand(animations, this, 'an Animation').rate; }
    set playbackRate(v) {
      const a = brand(animations, this, 'an Animation');
      a.scripted = true;
      const rate = toDouble(v, 'playbackRate');
      a.pendingRate = null;
      const previous = currentTimeOf(a);
      a.rate = rate;
      if (previous !== null) seekTo(a, previous);
      else updateFinished(a, false, false);
    }
    get playState() { return playStateOf(playbackState(this)); }
    get replaceState() { return brand(animations, this, 'an Animation').replaceState; }
    get pending() { return playbackState(this).pending !== null; }
    get ready() { return playbackState(this).ready.promise; }
    get finished() { return playbackState(this).finished.promise; }
    get overallProgress() {
      const a = brand(animations, this, 'an Animation');
      const ct = currentTimeOf(a);
      if (!a.effect || ct === null) return null;
      const end = endOf(a);
      if (end === 0) return ct < 0 ? 0 : 1;
      if (end === Infinity) return 0;
      return Math.min(Math.max(ct / end, 0), 1);
    }
    play() { const a = scripted(playbackState(this)); play(a, true); override(a, 'playState'); }
    pause() { const a = scripted(playbackState(this)); pause(a); override(a, 'playState'); }
    finish() {
      const a = scripted(brand(animations, this, 'an Animation'));
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
    cancel() { cancel(scripted(brand(animations, this, 'an Animation'))); }
    reverse() {
      const a = override(scripted(brand(animations, this, 'an Animation')), 'playState');
      if (timelineTime(a.timeline) === null) throw invalidState('Cannot reverse an animation with no active timeline');
      const original = a.pendingRate;
      a.pendingRate = effectiveRate(a) === 0 ? 0 : -effectiveRate(a);
      try {
        play(a, true);
      } catch (e) {
        a.pendingRate = original;
        throw e;
      }
    }
    updatePlaybackRate(v) {
      const a = scripted(brand(animations, this, 'an Animation'));
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
      const wasRemoved = a.replaceState === 'removed';
      if (wasRemoved) track(a);
      a.replaceState = 'persisted';
      if (wasRemoved) report(a);
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

  // Script controls the animation: a CSS animation or transition follows its
  // object's state from now on (reported).
  function scripted(a) {
    a.scripted = true;
    return a;
  }
  // Script set one of a CSS animation's properties: CSS no longer overrides that one (CSS
  // Animations 2 § 4.2: `playState` after play() or pause(), `timeline`, `rangeStart`/`rangeEnd`,
  // `effect`, a timing property after updateTiming()); its other properties still follow CSS.
  function override(a, what) {
    (a.overrides || (a.overrides = new Set())).add(what);
    return a;
  }

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
  // engine samples the CSS keyframes (or the transition's values) itself, by
  // its own timeline until script controls the object, by the object's
  // state after.
  const CSS_TRANSITION = 0, CSS_ANIMATION = 1;
  const cssRuns = new Map();
  function playbackState(object) {
    const a = brand(animations, object, 'an Animation');
    if (a.cssOwner) native.flushPlayback();
    return a;
  }
  class CSSAnimation extends Animation {
    constructor() { throw illegal(); }
    get animationName() { return brand(animations, this, 'a CSSAnimation').cssName; }
  }
  class CSSTransition extends Animation {
    constructor() { throw illegal(); }
    get transitionProperty() { return brand(animations, this, 'a CSSTransition').cssName; }
  }
  // The timeline of a scroll-driven CSS animation (`animation-timeline`, serialized): an anonymous
  // `scroll()` or `view()` timeline, or the scroll or view timeline a name declares on the target
  // or an ancestor; `null` (inactive) when none does.
  const isScrollContainer = el => {
    const cs = computedStyle(el);
    return /^(auto|scroll|hidden)$/.test(cs.overflowX) || /^(auto|scroll|hidden)$/.test(cs.overflowY);
  };
  // Whether `cs`'s element is the containing block of `position` descendants.
  const containsPositioned = (cs, position) => {
    if (position === 'absolute' && cs.position !== 'static') return true;
    return cs.transform !== 'none' || cs.perspective !== 'none' || cs.filter !== 'none' || /layout|paint|strict|content/.test(cs.contain) ||
      /transform|perspective|filter/.test(cs.willChange);
  };
  // (A pseudo-element's nearest scroll container may be its originating element: `self`.)
  function nearestScroller(el, includeSelf) {
    // An absolutely positioned box moves with its containing block's scroll containers, not with
    // the ones it sits inside in the tree.
    const position = computedStyle(el).position;
    if (!includeSelf && (position === 'absolute' || position === 'fixed')) {
      let cb = el.parentElement;
      while (cb !== null && cb !== document.documentElement && !containsPositioned(computedStyle(cb), position)) cb = cb.parentElement;
      el = cb;
      includeSelf = true;
      if (el === null || el === document.documentElement) return scrollingElement();
    }
    for (let p = includeSelf ? el : el.parentElement; p !== null; p = p.parentElement) {
      if (p === document.documentElement || p === document.body) break;
      if (isScrollContainer(p)) return p;
    }
    return scrollingElement();
  }
  function cssTimeline(text, target, pseudo) {
    const t = String(text).trim();
    const axisIn = s => (/\b(inline|x|y)\b/.exec(s) || ['block'])[0];
    let m = /^scroll\((.*)\)$/.exec(t);
    if (m) {
      const args = m[1];
      const source = /\bself\b/.test(args) ? target : /\broot\b/.test(args) ? scrollingElement() : nearestScroller(target, pseudo !== null && pseudo !== undefined);
      return new ScrollTimeline({ source, axis: axisIn(args) });
    }
    m = /^view\((.*)\)$/.exec(t);
    if (m) {
      const args = m[1].replace(/\b(block|inline|x|y)\b/, '').trim();
      const timeline = new ViewTimeline({ subject: target, axis: axisIn(m[1]), inset: args === '' ? 'auto' : args });
      // (A pseudo-element's `view()`: the pseudo-element's box is the subject; 2 `::before`, 3 `::after`.)
      const code = ({ '::before': 2, ':before': 2, '::after': 3, ':after': 3 })[pseudo];
      if (code) timelines.get(timeline).subjectPseudo = code;
      return timeline;
    }
    if (t.startsWith('--')) {
      // The nearest ancestor-or-self declaring it (a scroll progress timeline before a view one on
      // the same element); an ancestor's `timeline-scope` naming it (or `all`) makes the one
      // element below it declaring it visible, none or several leave it unresolved (Scroll-driven
      // Animations 1 § 4.5; the engine's `scroll_timeline.rs` `named`).
      for (let el = target; el !== null; el = parentOf(el)) {
        const found = declaredTimeline(el, null, t);
        if (found) return timelineOf(found);
        let scope = '';
        try { scope = String(computedStyle(el).getPropertyValue('timeline-scope') || ''); } catch {}
        const scoped = scope.split(',').map(x => x.trim());
        if (scoped.some(n => n === t || n === 'all')) {
          const all = [];
          scopedDeclarations(el, t, all);
          return all.length === 1 ? timelineOf(all[0]) : null;
        }
      }
    }
    return null;
  }
  // An element's parent, across a shadow root's boundary to its host.
  function parentOf(el) {
    if (el.parentElement !== null) return el.parentElement;
    const root = apply(getRootNode, el, []);
    return root && root.host ? root.host : null;
  }
  // The timeline `name` that `el` (or its `pseudo`-element) declares itself, as
  // `{pseudo, el, scroll, axis, inset}`; null when it declares none of that name.
  function declaredTimeline(el, pseudo, name) {
    let cs;
    try { cs = pseudo ? computedStyle(el, pseudo) : computedStyle(el); } catch { return null; }
    const names = v => String(v || '').split(',').map(x => x.trim());
    const scrollNames = names(cs.getPropertyValue('scroll-timeline-name'));
    let i = scrollNames.lastIndexOf(name);
    if (i >= 0) {
      const axes = names(cs.getPropertyValue('scroll-timeline-axis'));
      return { pseudo, el, scroll: true, axis: axes[i % axes.length] || 'block' };
    }
    const viewNames = names(cs.getPropertyValue('view-timeline-name'));
    i = viewNames.lastIndexOf(name);
    if (i >= 0) {
      const axes = names(cs.getPropertyValue('view-timeline-axis'));
      const insets = names(cs.getPropertyValue('view-timeline-inset'));
      return { pseudo, el, scroll: false, axis: axes[i % axes.length] || 'block', inset: insets[i % insets.length] || 'auto' };
    }
    return null;
  }
  // The declarations of `name` below `scope` (its own pseudo-elements and those of the elements
  // below it included, shadow trees too), stopping at a second.
  function scopedDeclarations(scope, name, out) {
    const visit = el => {
      if (out.length > 1) return;
      if (el !== scope) { const d = declaredTimeline(el, null, name); if (d) out.push(d); }
      for (const pseudo of ['::before', '::after']) { const d = declaredTimeline(el, pseudo, name); if (d) out.push(d); }
      if (el.shadowRoot) for (const c of el.shadowRoot.children) visit(c);
      for (const c of el.children) visit(c);
    };
    visit(scope);
  }
  // The timeline object of a declaration. (A pseudo-element's timeline reports its originating
  // element as the source or subject, as browsers do: the pseudo-element is never exposed.)
  function timelineOf(d) {
    if (d.scroll) return new ScrollTimeline({ source: d.el, axis: d.axis });
    const timeline = new ViewTimeline({ subject: d.el, axis: d.axis, inset: d.inset });
    const code = ({ '::before': 2, '::after': 3 })[d.pseudo];
    if (code) timelines.get(timeline).subjectPseudo = code;
    return timeline;
  }
  function cssRun([id, target, transition, name, start, delay, duration, iterations, direction, fill, easing, held, pseudo, timelineText, trigger, keyframeEasing, generation]) {
    // The engine hands over computed easings; one it could not compute is
    // the initial `ease`, as an easing invalid at computed-value time is.
    let parsed;
    try { parsed = parseEasing(easing); } catch { parsed = parseEasing('ease'); }
    const timing = { delay, duration, iterations, fill, direction, easing: parsed.text };
    // (A run on the target's `::before`/`::after`: its effect's pseudo-element.)
    if (pseudo !== null && pseudo !== undefined) timing.pseudoElement = pseudo;
    const scrollDriven = timelineText !== null && timelineText !== undefined;
    if (scrollDriven && timing.duration === 0) timing.duration = 'auto';
    const [timelineValue, rangeStartText, rangeEndText] = scrollDriven ? timelineText : [null, 'normal', 'normal'];
    const effect = new KeyframeEffect(target, null, timing);
    // (A view-transition pseudo-element's run has no id: its transition root keeps its keyframes.)
    const viewPseudo = typeof pseudo === 'string' && pseudo.startsWith('::view-transition');
    if (!transition && (viewPseudo || (id != null && !pseudo))) effects.get(effect).cssFrameSource = { target, name, easing: keyframeEasing, pseudo: viewPseudo ? pseudo : undefined };
    if (transition && id != null) {
      const frames = native.transitionFrames(id).map(([offset, property, value]) => {
        const member = keyframeMember(property);
        return { offset, [member]: value };
      });
      effects.get(effect).cssFrames = processKeyframes(frames);
    }
    // A scroll-driven one: on its scroll or view timeline, its `auto` duration filling it, its
    // start time at the timeline's start.
    const timeline = scrollDriven ? cssTimeline(timelineValue, target, pseudo) : documentTimeline;
    const object = Reflect.construct(Animation, [effect, timeline], transition ? CSSTransition : CSSAnimation);
    const a = animations.get(object);
    a.css = transition ? CSS_TRANSITION : CSS_ANIMATION;
    a.cssName = name;
    a.cssId = id;
    // A transition's generation: the style change event that started it (one event's
    // transitions share it, and list by property name; two events in one task differ).
    a.cssGeneration = generation;
    a.cssOwner = id == null ? null : target;
    a.cssPaused = held !== null;
    a.cssPseudoOrder = pseudo === '::marker' ? 1 : pseudo === '::before' ? 2 : pseudo === '::after' ? 3 : 0;
    a.cssTimelineKey = scrollDriven ? JSON.stringify(timelineText) : null;
    if (trigger !== null && trigger !== undefined && !scrollDriven) {
      armTrigger(a, trigger, held);
      return object;
    }
    if (scrollDriven) {
      a.scrollDriven = true;
      // (Its `animation-range`, as the engine places it.)
      try { a.rangeStart = parseRange(rangeStartText, false, 'rangeStart'); } catch { a.rangeStart = 'normal'; }
      try { a.rangeEnd = parseRange(rangeEndText, true, 'rangeEnd'); } catch { a.rangeEnd = 'normal'; }
      a.startTime = timeline !== null && timelineTime(timeline) !== null ? (rangeOffsets(a) || [0, 100])[0] : null;
      a.holdTime = held !== null && a.startTime !== null ? null : (a.startTime === null ? 0 : null);
      a.ready = deferred();
      if (a.startTime !== null) settle(a.ready, object, false); else { a.pending = null; schedulePending(a, 'play'); a.autoAlign = true; }
      track(a);
      updateFinished(a, true, false);
      return object;
    }
    // As the engine's timeline: started then, or held by
    // `animation-play-state: paused`. One the style started this frame
    // waits for the frame to start it (play-pending, its time held at 0),
    // as a CSS animation or transition that starts does: listed by
    // getAnimations() however short it is, `ready` once it runs.
    const age = now() - start;
    // (When the engine's timeline ends: `endedWhilePending`.)
    a.cssStart = start;
    a.cssEnd = start + delay + duration * iterations;
    if (held !== null) {
      a.startTime = null;
      a.holdTime = held;
    } else if (age < FRAME_MS) {
      a.startTime = null;
      a.holdTime = 0;
      a.ready = deferred();
      schedulePending(a, 'play', id == null ? undefined : start);
    } else {
      a.startTime = start;
      a.holdTime = null;
    }
    track(a);
    updateFinished(a, true, false);
    return object;
  }
  // ----- animation triggers (CSS Animations 2 `animation-trigger`, `timeline-trigger`) ----
  // pyro: a CSS animation with an `animation-trigger` naming a timeline trigger waits, held
  // just before its start (its backwards fill shows), until the trigger's timeline enters the
  // trigger's activation range: then its first action runs; leaving the active range (the
  // `/`-given one, else the activation range) runs its second. Actions: play, play-once (once),
  // play-forwards, play-backwards, pause, reset, replay, none. Its object controls it from then
  // on (script's: `playState` overridden). `trigger-scope` is not supported.
  const triggered = new Set();
  let triggerListening = false;
  function parseTriggerRanges(text) {
    // `<range-start> <range-end>`: each `[<name>] [<length-percentage>]`; a lone name covers it.
    const tokens = text.trim().split(/\s+/).filter(Boolean);
    const groups = [];
    for (const tok of tokens) {
      const last = groups[groups.length - 1];
      if (RANGE_NAMES.includes(tok) || last === undefined || last.offset !== null) groups.push({ name: null, offset: null });
      const g = groups[groups.length - 1];
      if (RANGE_NAMES.includes(tok)) g.name = tok; else g.offset = tok;
    }
    const text1 = g => [g.name, g.offset].filter(x => x !== null).join(' ');
    if (groups.length === 0) return ['normal', 'normal'];
    if (groups.length === 1) {
      const g = groups[0];
      if (g.offset === null) return [g.name + ' 0%', g.name + ' 100%'];
      return [text1(g), 'normal'];
    }
    return [text1(groups[0]), text1(groups[1])];
  }
  function parseTrigger(decl, source) {
    let rest = decl.trim().replace(/^\S+\s*/, '');
    let timelineText;
    const fn = /^(view|scroll)\(/.exec(rest);
    if (fn) {
      const close = rest.indexOf(')');
      timelineText = rest.slice(0, close + 1);
      rest = rest.slice(close + 1);
    } else {
      const m = /^(\S+)\s*/.exec(rest);
      timelineText = m ? m[1] : 'auto';
      rest = rest.slice(m ? m[0].length : 0);
    }
    const [enter, exit] = rest.split('/');
    const parse = (pair, isEnd) => {
      try { return parseRange(pair, isEnd, isEnd ? 'rangeEnd' : 'rangeStart'); } catch { return 'normal'; }
    };
    const [es, ee] = parseTriggerRanges(enter || '');
    const range = [parse(es, false), parse(ee, true)];
    let exitRange = range;
    if (exit !== undefined) {
      const [xs, xe] = parseTriggerRanges(exit);
      exitRange = [parse(xs, false), parse(xe, true)];
    }
    return { timeline: cssTimeline(timelineText, source, null), range, exitRange };
  }
  function armTrigger(a, [actionsText, source, decl], held) {
    const words = actionsText.trim().split(/\s+/).slice(1);
    let t;
    try { t = parseTrigger(decl, source); } catch { t = null; }
    a.trigger = { enter: words[0] || 'play', exit: words[1] || 'none', state: 'idle', spec: t, once: false };
    a.triggerKey = JSON.stringify([actionsText, decl]);
    a.triggerSource = source;
    a.scripted = true;
    override(a, 'playState');
    // Untriggered: just before its start (Chromium shows only its backwards fill).
    a.startTime = null;
    a.holdTime = held !== null && held < 0 ? held : -1e-6;
    track(a);
    updateFinished(a, true, false);
    triggered.add(a);
    evaluateTriggers();
    if (!triggerListening) {
      triggerListening = true;
      apply(addListener, globalThis, ['scroll', () => {
        const later = fn => (requestFrame !== null ? apply(requestFrame, globalThis, [() => schedule(0, fn)]) : schedule(16, fn));
        later(evaluateTriggers);
      }, { capture: true, passive: true }]);
    }
    // (At the next frame: its timeline has the layout then.)
    if (requestFrame !== null) apply(requestFrame, globalThis, [evaluateTriggers]); else schedule(16, evaluateTriggers);
  }
  function evaluateTriggers() {
    pushScroll();
    for (const a of [...triggered]) {
      if (!a.effect || !isConnected(a.effect.target)) { triggered.delete(a); continue; }
      const tr = a.trigger;
      const spec = tr.spec;
      if (!spec || spec.timeline === null) continue;
      const now = timelineTime(spec.timeline);
      if (now === null) continue;
      const within = ([rs, re]) => {
        const r = rangeOffsets({ timeline: spec.timeline, rangeStart: rs, rangeEnd: re }) || [0, 100];
        return now >= r[0] && now <= r[1];
      };
      if (tr.state !== 'active' && within(spec.range)) {
        if (tr.state === 'idle' && a.holdTime === -1e-6) a.holdTime = 0;
        tr.state = 'active';
        runTriggerAction(a, tr.enter);
      } else if (tr.state === 'active' && !within(spec.exitRange)) {
        tr.state = 'inactive';
        runTriggerAction(a, tr.exit);
      }
    }
  }
  function runTriggerAction(a, action) {
    const tr = a.trigger;
    switch (action) {
      case 'play': play(a, false); break;
      case 'play-once': if (!tr.once) { tr.once = true; play(a, false); } break;
      case 'play-forwards':
        if (effectiveRate(a) < 0) a.pendingRate = -effectiveRate(a);
        play(a, false);
        break;
      case 'play-backwards':
        if (effectiveRate(a) > 0) a.pendingRate = -effectiveRate(a);
        play(a, false);
        break;
      case 'pause': pause(a); break;
      case 'reset':
        pause(a);
        silentlySetCurrentTime(a, effectiveRate(a) < 0 ? endOf(a) : 0);
        updateFinished(a, true, false);
        break;
      case 'replay':
        silentlySetCurrentTime(a, effectiveRate(a) < 0 ? endOf(a) : 0);
        play(a, false);
        break;
      default: break;
    }
  }
  // A frame of the engine's (animation_frame.rs, `FRAME_MS`).
  const FRAME_MS = 16;
  // The engine's runs now ([id, target, transition, name, start, delay,
  // duration, iterations, direction, fill, easing, held, pseudo]); those it dropped
  // (canceled) leave the list.
  function cssSync(runs) {
    const seen = new Set();
    for (const [index, run] of runs.entries()) {
      seen.add(run[0]);
      const known = cssRuns.get(run[0]);
      if (known !== undefined) {
        animations.get(known).cssListOrder = index;
        cssFollow(known, run);
        continue;
      }
      // One run that cannot be made is left out; the others are listed.
      try {
        const object = cssRun(run);
        animations.get(object).cssListOrder = index;
        cssRuns.set(run[0], object);
      } catch {}
    }
    for (const [id, object] of cssRuns) {
      if (seen.has(id)) continue;
      cssRuns.delete(id);
      live.delete(object);
      // (A run the style dropped no longer waits for its trigger.)
      const gone = animations.get(object);
      if (gone) {
        gone.cssOwner = null;
        triggered.delete(gone);
        if (endedWhilePending(gone)) finishEndedCss(gone);
        else if (gone.css === CSS_ANIMATION || playStateOf(gone) !== 'finished') cancel(gone);
      }
    }
  }
  // A transition the engine dropped at its end (its time passed: the engine's clock is this one)
  // while its object still waited for its pending play (a busy task, then a style change event
  // before that play's task): it ended, so it finishes, as in Chrome, and is not canceled.
  function endedWhilePending(a) {
    return a.css === CSS_TRANSITION && a.pending === 'play' && !a.scripted && a.cssEnd !== undefined && now() >= a.cssEnd;
  }
  function finishEndedCss(a) {
    cancelPending(a);
    a.startTime = a.cssStart;
    a.holdTime = null;
    settle(a.ready, a.self, false);
    updateFinished(a, true, false);
  }

  function viewRuns(target, entries) {
    const objects = new Map();
    function sync(entries) {
      const seen = new Set();
      for (const [box, index, pseudo, name, delay, duration, iterations, direction, fill, easing, paused] of entries) {
        const key = `${box}:${index}:${name}`;
        seen.add(key);
        const existing = objects.get(key);
        const state = existing && animations.get(existing);
        const time = now();
        const held = paused ? (state && currentTimeOf(state)) ?? 0 : null;
        const start = state ? state.startTime ?? (time - (state.holdTime ?? 0)) : time;
        const run = [null, target, false, name, start, delay, duration, iterations, direction, fill, easing, held, pseudo, null, null];
        if (existing) {
          if (playStateOf(state) !== 'idle') cssFollow(existing, run);
        } else {
          const object = cssRun(run);
          animations.get(object).viewRun = true;
          animations.get(object).viewSlot = [box, index];
          objects.set(key, object);
        }
      }
      for (const [key, object] of objects) {
        if (!seen.has(key)) { cancel(animations.get(object)); objects.delete(key); }
      }
    }
    sync(entries);
    return {
      sync,
      sample() {
        const values = [];
        for (const object of objects.values()) {
          const a = animations.get(object);
          if (!a.effect || playStateOf(a) === 'idle') continue;
          const progress = computedTimingOf(a.effect).directed;
          if (progress !== undefined) values.push(a.viewSlot[0], a.viewSlot[1], progress);
        }
        return values;
      },
      active() { for (const object of objects.values()) { const state = playStateOf(animations.get(object)); if (state !== 'finished' && state !== 'idle') return true; } return false; },
      release() { for (const object of objects.values()) cancel(animations.get(object)); objects.clear(); },
    };
  }
  // An object script does not control follows its run's play state
  // (`animation-play-state` paused or running again).
  // pyro: and the CSS properties script did not override follow their CSS values, script
  // controlled or not (CSS Animations 2 § 4.2): `animation-timeline` (and range), the timing
  // properties, `animation-play-state`.
  function cssFollow(object, run) {
    const a = animations.get(object);
    if (a.css !== CSS_ANIMATION) return cssFollowPlayState(a, run);
    const ov = a.overrides || new Set();
    const [, target, , , , delay, duration, iterations, direction, fill, , held, pseudo, timelineText, trigger] = run;
    // A new `animation-trigger` or timeline trigger (its text, or the element declaring it: the
    // trigger's timeline follows that element) binds the run to it anew; none, or a run that
    // became scroll-driven (no trigger applies to it), unbinds it.
    const wanted = trigger && !timelineText ? trigger : null;
    const bound = wanted
      ? a.trigger && a.triggerKey === JSON.stringify([wanted[0], wanted[2]]) && a.triggerSource === wanted[1]
      : !a.trigger;
    if (!bound) {
      if (wanted) {
        armTrigger(a, wanted, null);
      } else if (a.trigger) {
        triggered.delete(a);
        a.trigger = null;
        a.triggerKey = null;
        a.triggerSource = null;
        if (a.overrides) a.overrides.delete('playState');
        play(a, false);
      }
    }
    const scrollDriven = timelineText !== null && timelineText !== undefined;
    // The timeline (Web Animations 2, "setting the timeline of an animation").
    const key = scrollDriven ? JSON.stringify(timelineText) : null;
    if (key !== a.cssTimelineKey && !ov.has('timeline')) {
      const wasProgress = a.scrollDriven === true;
      a.cssTimelineKey = key;
      if (scrollDriven) {
        if (!ov.has('rangeStart')) { try { a.rangeStart = parseRange(timelineText[1], false, 'rangeStart'); } catch { a.rangeStart = 'normal'; } }
        if (!ov.has('rangeEnd')) { try { a.rangeEnd = parseRange(timelineText[2], true, 'rangeEnd'); } catch { a.rangeEnd = 'normal'; } }
      }
      const next = scrollDriven ? cssTimeline(timelineText[0], target, pseudo) : documentTimeline;
      a.scrollDriven = scrollDriven;
      if (wasProgress || scrollDriven) setTimelineOf(a, next);
    } else if (scrollDriven && a.scrollDriven) {
      // A new `animation-range`.
      let moved = false;
      for (const [k, i, isEnd] of [['rangeStart', 1, false], ['rangeEnd', 2, true]]) {
        if (ov.has(k)) continue;
        let r;
        try { r = parseRange(timelineText[i], isEnd, k); } catch { r = 'normal'; }
        if (JSON.stringify(r) !== JSON.stringify(a[k])) { a[k] = r; moved = true; }
      }
      if (moved) rangeChanged(a);
    }
    // The timing properties.
    if (a.effect && !ov.has('effect')) {
      const want = {
        delay, iterations, direction, fill,
        duration: a.scrollDriven && duration === 0 ? 'auto' : duration,
      };
      let changed = false;
      for (const f of Object.keys(want)) {
        if (ov.has('timing:' + f) || a.effect.timing[f] === want[f]) continue;
        a.effect.timing = { ...a.effect.timing, [f]: want[f] };
        changed = true;
      }
      if (changed) timingChanged(a);
    }
    // `animation-play-state`: the engine's run times one CSS controls alone; script's (or a
    // progress-based one) is paused or played.
    if (ov.has('playState')) return;
    if (a.cssOwner && !a.scrollDriven && !a.trigger) {
      const paused = held !== null;
      if (a.cssPaused !== paused) {
        a.cssPaused = paused;
        if (playStateOf(a) !== 'idle') {
          if (paused) pause(a); else play(a, false);
        }
      }
      return;
    }
    // pyro: a view transition's run follows `animation-play-state` once script controls it only
    // when the property changes (CSS Animations 1 § 3): one script finished while paused stays
    // finished, and its transition ends (Chrome; WPT css-view-transitions `scoped/run_in_parallel`).
    if (a.viewRun) {
      const paused = held !== null;
      const changed = a.cssPaused !== paused;
      a.cssPaused = paused;
      if (a.scripted && !a.scrollDriven && !changed) return;
    }
    if (!a.scripted && !a.scrollDriven) return cssFollowPlayState(a, run);
    const state = playStateOf(a);
    if (held !== null) {
      if (state !== 'paused' && state !== 'idle' && a.pending !== 'pause') pause(a);
    } else if (state === 'paused' && a.pending !== 'play') {
      play(a, false);
    }
  }
  // Web Animations 2, "setting the timeline of an animation" (as CSS sets it: not script's).
  function setTimelineOf(a, next) {
    if (next === a.timeline) return;
    const previous = playStateOf(a);
    const ct = currentTimeOf(a);
    const endBefore = endOf(a);
    const progress = ct === null ? null : endBefore > 0 && endBefore !== Infinity ? ct / endBefore : 0;
    a.timeline = next;
    const toFinite = next !== null && timelines.get(next).scroll;
    if (toFinite) {
      applyPendingRate(a);
      a.autoAlign = true;
      a.startTime = null;
      a.holdTime = null;
      if (previous === 'running' || previous === 'finished') {
        if (a.pending === null) a.ready = deferred();
        cancelPending(a);
        schedulePending(a, 'play');
      } else if (previous === 'paused' && progress !== null) {
        a.holdTime = progress * endOf(a);
      }
    } else if (progress !== null) {
      a.autoAlign = false;
      if (a.startTime !== null) a.holdTime = null;
      silentlySetCurrentTime(a, progress * endOf(a));
    }
    track(a);
    updateFinished(a, false, false);
  }
  // A run CSS alone controls: its start or held time from the engine.
  function cssFollowPlayState(a, run) {
    if (a.scripted || a.pending !== null || a.scrollDriven) return;
    const start = run[4], held = run[11];
    if (held !== null) {
      if (a.startTime === null && a.holdTime === held) return;
      a.startTime = null;
      a.holdTime = held;
    } else {
      if (a.holdTime === null && a.startTime === start) return;
      a.startTime = start;
      a.holdTime = null;
    }
    updateFinished(a, true, false);
  }
  Object.defineProperty(CSSAnimation.prototype, Symbol.toStringTag, { value: 'CSSAnimation', configurable: true });
  Object.defineProperty(CSSTransition.prototype, Symbol.toStringTag, { value: 'CSSTransition', configurable: true });

  // ----- Animatable and DocumentOrShadowRoot -------------------------------

  // A new range on a progress-based timeline: an auto-aligned start time follows it.
  function rangeChanged(a) {
    if (progressBased(a) && a.startTime !== null && a.holdTime === null && !a.scripted) {
      const range = rangeOffsets(a) || [0, 100];
      a.startTime = a.rate >= 0 ? range[0] : range[1];
    }
    updateFinished(a, false, false);
  }

  function animate(target, keyframes, options) {
    const effect = new KeyframeEffect(target, keyframes, options);
    let timeline = documentTimeline;
    let id = '';
    let rangeStart = 'normal';
    let rangeEnd = 'normal';
    if (isObject(options)) {
      if (options.id !== undefined) id = String(options.id);
      if (options.timeline !== undefined) timeline = options.timeline;
      if (options.rangeStart !== undefined) rangeStart = parseRange(options.rangeStart, false, 'rangeStart');
      if (options.rangeEnd !== undefined) rangeEnd = parseRange(options.rangeEnd, true, 'rangeEnd');
    }
    const animation = new Animation(effect, timeline);
    animations.get(animation).id = id;
    animations.get(animation).rangeStart = rangeStart;
    animations.get(animation).rangeEnd = rangeEnd;
    animation.play();
    return animation;
  }

  // In composite order.
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
    const rank = a => a.cssOwner ? a.css : a.viewRun ? 1.5 : 2;
    out.sort((x, y) => {
      const type = rank(x) - rank(y);
      if (type || !x.cssOwner) return type || x.seq - y.seq;
      if (x.cssOwner !== y.cssOwner) {
        const pos = apply(comparePosition, x.cssOwner, [y.cssOwner]);
        return pos & 1 ? x.seq - y.seq : pos & 4 ? -1 : 1;
      }
      // (Transitions: by generation, then by property name in code point order, as Chrome;
      // WPT css-transitions Element-getAnimations 'sorts simultaneous transitions by name'.)
      return x.cssPseudoOrder - y.cssPseudoOrder ||
        (x.css === CSS_ANIMATION ? x.cssListOrder - y.cssListOrder :
          (x.cssGeneration - y.cssGeneration) || (x.cssName < y.cssName ? -1 : x.cssName > y.cssName ? 1 : 0) || x.seq - y.seq);
    });
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

  // WebIDL: the attributes and operations of an interface are enumerable properties of its prototype.
  for (const C of [Animation, AnimationEffect, KeyframeEffect, AnimationTimeline, DocumentTimeline, AnimationPlaybackEvent,
    ScrollTimeline, ViewTimeline, CSSAnimation, CSSTransition]) {
    for (const key of Reflect.ownKeys(C.prototype)) {
      if (key === 'constructor' || typeof key === 'symbol') continue;
      const d = Object.getOwnPropertyDescriptor(C.prototype, key);
      if (d && d.configurable && !d.enumerable) Object.defineProperty(C.prototype, key, { enumerable: true });
    }
  }

  return {
    Animation, AnimationEffect, KeyframeEffect, AnimationTimeline, DocumentTimeline, AnimationPlaybackEvent,
    ScrollTimeline, ViewTimeline,
    CSSAnimation, CSSTransition,
    validateEventAnimation(value, transition) {
      if (animations.get(value)?.css !== (transition ? CSS_TRANSITION : CSS_ANIMATION))
        throw new TypeError(transition ? 'Expected a CSSTransition' : 'Expected a CSSAnimation');
    },
    cssObject: id => cssRuns.get(id) ?? null,
    animate, elementAnimations, rootAnimations, cssSync, viewRuns, timeline: () => documentTimeline,
  };
})
