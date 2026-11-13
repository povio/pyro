// `page.clock` / `context.clock`: Playwright's fake clock. On the first call
// a controller in each document replaces Date, Intl.DateTimeFormat, timers,
// requestAnimationFrame, requestIdleCallback, performance.now,
// AbortSignal.timeout and Event.timeStamp, with the clock running.
//
// The controller keeps two times, as Playwright's does: monotonic `ticks`,
// which drive timers and performance.now, and wall `time`, which Date reads.
// runFor, fastForward, pauseAt and real time advance both; setSystemTime
// and setFixedTime only move (or freeze) the wall time.
//
// Every call runs in every frame of every open page, and is also logged as
// an init script with the wall time it was made at: a new document replays
// the log (state only, no timers) and lands on the same fake time. Each call
// has a sequence number, so a document that already has a call (its log was
// replayed) skips the live one. Fake timers are not the browser's, so a
// paused clock does not hold up an action's settling.
import type { BrowserContext } from './browser.js';

/** Runs in the page: `globalThis.__lpClock`. */
function controller() {
  const g = globalThis as any;
  if (g.__lpClock) return;
  const real = {
    Date: g.Date as DateConstructor,
    setTimeout: g.setTimeout.bind(g) as (f: () => void, ms?: number) => number,
    clearTimeout: g.clearTimeout.bind(g) as (id: number) => void,
    perf: g.performance ? g.performance.now.bind(g.performance) : () => real.Date.now(),
    DTF: g.Intl?.DateTimeFormat as typeof Intl.DateTimeFormat | undefined,
  };
  type Kind = 'Timeout' | 'Interval' | 'AnimationFrame' | 'IdleCallback';
  type NativeTimer = { at: number; cancel: () => void; running?: Promise<void> };
  type Timer = { id: number; kind: Kind; fn: unknown; args: unknown[]; delay: number; at: number; createdAt: number };
  const maxTimeout = 2 ** 31 - 1;
  const s = {
    time: 0, // wall time, what Date reads
    ticks: 0, // monotonic, what timers and performance.now read
    fixed: false,
    origin: -1, // the first wall time set: performance.timeOrigin
    realTime: undefined as { last: number } | undefined, // set while running
    native: undefined as NativeTimer | undefined,
    duringTick: false,
    timers: new Map<number, Timer>(),
    nextId: 1e12,
    applied: 0, // the last call (sequence number) this document has
    log: [] as { type: string; wall: number; param?: number }[],
  };
  const realTicks = () => Math.ceil(real.perf());
  const advance = (to: number) => {
    if (s.ticks > to) return;
    if (!s.fixed) s.time += to - s.ticks;
    s.ticks = to;
  };
  const setTime = (t: number) => {
    s.time = t;
    s.fixed = false;
    if (s.origin < 0) s.origin = t;
  };
  const syncRealTime = () => {
    if (!s.realTime) return;
    const n = realTicks();
    if (n > s.realTime.last) {
      advance(s.ticks + n - s.realTime.last);
      s.realTime.last = n;
    }
  };
  const replay = () => {
    if (!s.log.length) return;
    let last = -1;
    let paused = false;
    for (const { type, wall, param } of s.log) {
      if (!paused && last !== -1) advance(s.ticks + wall - last);
      last = wall;
      if (type === 'install' || type === 'setSystemTime') setTime(param!);
      else if (type === 'fastForward' || type === 'runFor') advance(s.ticks + param!);
      else if (type === 'pauseAt') {
        // As Playwright's replay: the time is set even where the live call
        // threw for a time in the past.
        paused = true;
        setTime(param!);
      } else if (type === 'resume') paused = false;
      else if (type === 'setFixedTime') {
        setTime(param!);
        s.fixed = true;
      }
    }
    s.log.length = 0;
    if (paused) {
      stopRealTime();
    } else {
      if (last > 0) advance(s.ticks + real.Date.now() - last);
      startRealTime();
    }
  };
  const now = () => {
    replay();
    syncRealTime();
    return s.time;
  };
  const perfNow = () => {
    replay();
    syncRealTime();
    return s.ticks;
  };
  const firstTimer = (until?: number) => {
    let first: Timer | undefined;
    for (const t of s.timers.values()) {
      if (until !== undefined && t.at > until) continue;
      if (!first || t.at < first.at || (t.at === first.at && (t.createdAt < first.createdAt || (t.createdAt === first.createdAt && t.id < first.id)))) first = t;
    }
    return first;
  };
  /** Fires the timers due by `to` in order (re-reading the live map), then moves to `to`; throws the first exception. */
  const runTo = async (to: number) => {
    to = Math.ceil(to);
    if (s.ticks > to) return;
    let failed: { e: unknown } | undefined;
    for (let t = firstTimer(to); t; t = firstTimer(to)) {
      advance(t.at);
      if (t.kind === 'Interval') t.at += Math.max(1, t.delay);
      else s.timers.delete(t.id);
      s.duringTick = true;
      try {
        if (typeof t.fn !== 'function') (0, eval)(String(t.fn));
        else if (t.kind === 'AnimationFrame') t.fn(s.ticks);
        else if (t.kind === 'IdleCallback') t.fn({ didTimeout: false, timeRemaining: () => 0 });
        else t.fn(...t.args);
      } catch (e) {
        failed ??= { e };
      }
      try {
        await new Promise<void>((r) => real.setTimeout(r));
      } finally {
        s.duringTick = false;
      }
    }
    advance(to);
    if (failed) throw failed.e;
  };
  /** While running: one native timer for the next fake one. */
  const arm = () => {
    if (!s.realTime || s.native?.running) return;
    syncRealTime();
    const first = firstTimer();
    if (s.native && (!first || first.at < s.native.at)) {
      s.native.cancel();
      s.native = undefined;
    }
    if (!first || s.native) return;
    const n: NativeTimer = {
      at: first.at,
      cancel: () => real.clearTimeout(id),
    };
    const id = real.setTimeout(() => {
      syncRealTime();
      // A timer's exception while running goes to the console, as Playwright's.
      n.running = runTo(s.ticks).catch((e) => console.error(e));
      void n.running.then(() => {
        if (s.native === n) s.native = undefined;
        arm();
      });
    }, Math.max(0, first.at - s.ticks));
    s.native = n;
  };
  const startRealTime = () => {
    s.realTime = { last: realTicks() };
    arm();
  };
  const stopRealTime = async () => {
    s.realTime = undefined;
    const n = s.native;
    s.native = undefined;
    if (n) {
      n.cancel();
      await n.running;
    }
  };
  const withoutRealTime = async (fn: () => Promise<void>) => {
    if (!s.realTime) return fn();
    await stopRealTime();
    try {
      await fn();
    } finally {
      startRealTime();
    }
  };
  const fastForwardTo = async (to: number) => {
    if (to < s.ticks) throw new Error('Cannot fast-forward to the past');
    for (const t of s.timers.values()) if (to > t.at) t.at = to;
    await runTo(to);
  };
  const addTimer = (kind: Kind, fn: unknown, delay: unknown, args: unknown[] = []) => {
    replay();
    syncRealTime();
    if (!fn) {
      if (kind === 'AnimationFrame' || kind === 'IdleCallback') throw new Error(`Callback must be provided to request${kind} calls`);
      if (delay === undefined) throw new Error('Callback must be provided to timer calls');
    }
    let ms = delay ? +delay : 0;
    if (!Number.isFinite(ms)) ms = 0;
    ms = Math.max(0, ms > maxTimeout ? 1 : ms);
    const id = s.nextId++;
    s.timers.set(id, { id, kind, fn, args, delay: ms, at: s.ticks + (ms || (s.duringTick ? 1 : 0)), createdAt: s.ticks });
    arm();
    return id;
  };
  const clearTimer = (id: unknown, kind: Kind) => {
    replay();
    if (!id) return;
    const t = s.timers.get(Number(id));
    if (!t) return;
    const compatible = t.kind === kind || (t.kind !== 'AnimationFrame' && t.kind !== 'IdleCallback' && (kind === 'Timeout' || kind === 'Interval'));
    if (!compatible) {
      const name = (k: Kind, verb: [string, string]) => (k === 'AnimationFrame' || k === 'IdleCallback' ? `${verb[0]}${k}` : `${verb[1]}${k}`);
      throw new Error(`Cannot clear timer: timer created with ${name(t.kind, ['request', 'set'])}() but cleared with ${name(kind, ['cancel', 'clear'])}()`);
    }
    s.timers.delete(t.id);
  };

  // ---- the fakes ----
  // Seven named parameters: `Date.length` is 7, as the native one's.
  function ClockDate(this: unknown, year?: unknown, month?: unknown, date?: unknown, hour?: unknown, minute?: unknown, second?: unknown, ms?: unknown) {
    if (!new.target) return new real.Date(now()).toString();
    const a = [year, month, date, hour, minute, second, ms].slice(0, arguments.length);
    return Reflect.construct(real.Date, a.length ? a : [now()], new.target);
  }
  ClockDate.prototype = real.Date.prototype;
  Object.assign(ClockDate, { now: () => now(), parse: real.Date.parse, UTC: real.Date.UTC, isFake: true, toString: () => real.Date.toString() });
  Object.defineProperty(ClockDate, 'name', { value: 'Date' });
  g.Date = ClockDate;
  if (real.DTF) {
    const Native = real.DTF;
    const at = (d: unknown) => (d === undefined ? now() : d) as number;
    const ClockDTF = function (this: unknown, ...a: unknown[]) {
      return Reflect.construct(Native, a, new.target ?? ClockDTF);
    } as unknown as typeof Intl.DateTimeFormat;
    (ClockDTF as any).prototype = Object.create(Native.prototype, {
      constructor: { value: ClockDTF, writable: true, configurable: true },
      format: {
        configurable: true,
        get(this: Intl.DateTimeFormat) {
          const f = Reflect.get(Native.prototype, 'format', this) as (d?: unknown) => string;
          return (d?: unknown) => f(at(d));
        },
      },
      formatToParts: {
        configurable: true,
        writable: true,
        value(this: Intl.DateTimeFormat, d?: unknown) {
          return Native.prototype.formatToParts.call(this, at(d));
        },
      },
    });
    Object.defineProperty(ClockDTF, 'name', { value: 'DateTimeFormat' });
    (ClockDTF as any).supportedLocalesOf = Native.supportedLocalesOf.bind(Native);
    g.Intl.DateTimeFormat = ClockDTF;
  }
  g.setTimeout = (fn: unknown, d?: unknown, ...a: unknown[]) => addTimer('Timeout', fn, d, a);
  g.setInterval = (fn: unknown, d?: unknown, ...a: unknown[]) => addTimer('Interval', fn, d, a);
  g.clearTimeout = (id: unknown) => clearTimer(id, 'Timeout');
  g.clearInterval = (id: unknown) => clearTimer(id, 'Interval');
  if (g.requestAnimationFrame) {
    g.requestAnimationFrame = (fn: unknown) => addTimer('AnimationFrame', fn, 16 - (perfNow() % 16));
    g.cancelAnimationFrame = (id: unknown) => clearTimer(id, 'AnimationFrame');
  }
  if (g.requestIdleCallback) {
    g.requestIdleCallback = (fn: unknown, o?: { timeout?: number }) => {
      const next = s.timers.size ? 50 : 0;
      return addTimer('IdleCallback', fn, o?.timeout ? Math.min(o.timeout, next) : next);
    };
    g.cancelIdleCallback = (id: unknown) => clearTimer(id, 'IdleCallback');
  }
  if (g.performance) {
    try {
      g.performance.now = perfNow;
      Object.defineProperty(g.performance, 'timeOrigin', { configurable: true, get: () => (replay(), Math.max(0, s.origin)) });
    } catch {}
  }
  if (g.AbortSignal) {
    try {
      Object.defineProperty(g.AbortSignal, 'timeout', {
        configurable: true,
        writable: true,
        value(ms: number) {
          const c = new AbortController();
          addTimer('Timeout', () => c.abort(new DOMException('signal timed out', 'TimeoutError')), ms);
          return c.signal;
        },
      });
    } catch {}
  }
  if (g.Event) {
    const stamp = Symbol('pyro.clock.timeStamp');
    try {
      Object.defineProperty(g.Event.prototype, 'timeStamp', {
        configurable: true,
        get(this: any) {
          return (this[stamp] ??= perfNow());
        },
      });
    } catch {}
  }

  const api: Record<string, (p: any) => unknown> = {
    install: (time: number) => setTime(time),
    setSystemTime: (time: number) => setTime(time),
    setFixedTime: (time: number) => {
      setTime(time);
      s.fixed = true;
    },
    runFor: (ms: number) => {
      if (ms < 0) throw new TypeError('Negative ticks are not supported');
      return withoutRealTime(() => runTo(s.ticks + ms));
    },
    fastForward: (ms: number) => withoutRealTime(() => fastForwardTo(s.ticks + (ms | 0))),
    pauseAt: async (time: number) => {
      await stopRealTime();
      await fastForwardTo(s.ticks + time - s.time);
    },
    resume: () => startRealTime(),
  };
  g.__lpClock = {
    /** A call the test made, logged by an init script: replayed on first use. */
    log(seq: number, type: string, wall: number, param?: number) {
      if (seq <= s.applied) return;
      s.applied = seq;
      s.log.push({ type, wall, param });
    },
    /** A call the test makes now. */
    async call(seq: number, method: string, param?: number) {
      if (seq <= s.applied) return;
      s.applied = seq;
      replay();
      syncRealTime();
      await api[method]!(param);
    },
  };
  startRealTime();
}

type TimeLike = number | string | Date;
/** Playwright's `parseTime`: epoch ms, a date string or a Date. */
const toTime = (t: TimeLike): number => {
  if (typeof t === 'number') return t;
  if (!t) return 0;
  const v = typeof t === 'string' ? new Date(t).getTime() : t.getTime();
  if (!Number.isFinite(v)) throw new Error(`Invalid date: ${String(t)}`);
  return v;
};
/** Playwright's `parseTicks`: ms, `"ss"`, `"mm:ss"` or `"hh:mm:ss"`. */
const toTicks = (t: number | string): number => {
  if (typeof t === 'number') return t;
  if (!t) return 0;
  const parts = t.split(':');
  if (parts.length > 3 || !/^(\d\d:){0,2}\d\d?$/.test(t)) throw new Error(`Clock only understands numbers, 'mm:ss' and 'hh:mm:ss'`);
  let ms = 0;
  for (const p of parts) {
    const n = parseInt(p, 10);
    if (n >= 60) throw new Error(`Invalid time ${t}`);
    ms = ms * 60 + n;
  }
  return ms * 1000;
};

/** Playwright's Clock, shared by a context and its pages. */
export class Clock {
  private _seq = 0;
  /** The init scripts so far: the controller, then a log line per call. */
  private _scripts: string[] = [];
  constructor(private readonly _context: BrowserContext) {}

  /**
   * What a document that has no clock yet runs to catch up: the controller
   * and the log (`BrowserContext` runs it in a new target's first document,
   * which its init scripts came too late for). Undefined before the first call.
   */
  _bundle(): string | undefined {
    return this._scripts.length ? this._scripts.join('\n') : undefined;
  }

  private async _call(api: string, method: string, param?: number): Promise<void> {
    const seq = ++this._seq;
    const wall = Date.now();
    const scripts: string[] = [];
    if (!this._scripts.length) scripts.push(`(${controller})();`);
    scripts.push(`globalThis.__lpClock.log(${seq}, ${JSON.stringify(method)}, ${wall}${param === undefined ? '' : `, ${param}`});`);
    // The bundle grows first: a page that attaches meanwhile gets this call too.
    this._scripts.push(...scripts);
    for (const content of scripts) await this._context.addInitScript({ content });
    const bundle = this._scripts.join('\n');
    const expression = `(async () => {
      try {
        if (!globalThis.__lpClock) { ${bundle} }
        await globalThis.__lpClock.call(${seq}, ${JSON.stringify(method)}${param === undefined ? '' : `, ${param}`});
        return null;
      } catch (e) {
        // As Playwright's evaluate errors: "Error: message".
        return { message: e instanceof Error ? e.name + ': ' + e.message : String(e) };
      }
    })()`;
    // Every frame of every page that has a target; a frame that goes away meanwhile is skipped.
    const pages = this._context.pages().filter((p) => p._isAttached && !p.isClosed());
    const results = await Promise.all(pages.flatMap((p) => p.frames().map((f) => f.evaluate(expression).catch(() => null))));
    const failed = results.find((r): r is { message: string } => !!r);
    if (failed) throw new Error(`clock.${api}: ${failed.message}`);
  }
  /** Parses an argument, with the `clock.<api>: ` prefix on its error. */
  private _parse<T, R>(api: string, parse: (v: T) => R, v: T): R {
    try {
      return parse(v);
    } catch (e) {
      throw new Error(`clock.${api}: ${(e as Error).message}`);
    }
  }
  async install(options: { time?: TimeLike } = {}): Promise<void> {
    await this._call('install', 'install', options.time === undefined ? Date.now() : this._parse('install', toTime, options.time));
  }
  // A negative or overflowing value is rejected in the page, as Playwright's.
  async fastForward(ticks: number | string): Promise<void> {
    await this._call('fastForward', 'fastForward', this._parse('fastForward', toTicks, ticks));
  }
  async runFor(ticks: number | string): Promise<void> {
    await this._call('runFor', 'runFor', this._parse('runFor', toTicks, ticks));
  }
  async pauseAt(time: TimeLike): Promise<void> {
    await this._call('pauseAt', 'pauseAt', this._parse('pauseAt', toTime, time));
  }
  async resume(): Promise<void> {
    await this._call('resume', 'resume');
  }
  async setFixedTime(time: TimeLike): Promise<void> {
    await this._call('setFixedTime', 'setFixedTime', this._parse('setFixedTime', toTime, time));
  }
  async setSystemTime(time: TimeLike): Promise<void> {
    await this._call('setSystemTime', 'setSystemTime', this._parse('setSystemTime', toTime, time));
  }
}
