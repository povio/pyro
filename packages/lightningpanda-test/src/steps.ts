// Locators and steps as plain data: `page.run([...])` sends them to the
// browser in one LP.run command. Builders are Playwright-shaped
// (`button("Save")`, `dialog().hasText("…").button("OK")`, `.first()`), but
// nothing here talks to the browser.

export type TextMatch = string | RegExp;
type Pattern = string | { regex: string; flags?: string };

const pattern = (t: TextMatch): Pattern => {
  if (typeof t === 'string') return t;
  const flags = t.flags.replace(/[gyu]/g, '');
  return flags ? { regex: t.source, flags } : { regex: t.source };
};

/** The wire form of a locator (see `crates/lp-cdp/src/engine/driver.rs`). */
export type LocatorSpec = {
  css?: string;
  testId?: string;
  testIdAttribute?: string;
  text?: Pattern;
  role?: string;
  name?: Pattern;
  label?: Pattern;
  placeholder?: Pattern;
  exact?: boolean;
  within?: LocatorSpec;
  filter?: {
    visible?: boolean;
    hasText?: Pattern;
    hasNotText?: Pattern;
    has?: LocatorSpec;
    hasNot?: LocatorSpec;
  };
  nth?: number;
};

export type Step = Record<string, unknown>;
type Exact = { exact?: boolean };

/** Anything that becomes one step of a run. */
export interface StepLike {
  toStep(): Step;
}

export class Locator {
  constructor(readonly spec: LocatorSpec) {}

  // ---- narrowing (each returns a new locator) ----

  private with(filter: NonNullable<LocatorSpec['filter']>): Locator {
    // The browser filters the matches, then picks nth.
    if (this.spec.nth !== undefined) throw new Error('filter before first() / last() / nth()');
    return new Locator({ ...this.spec, filter: { ...this.spec.filter, ...filter } });
  }
  visible(visible = true): Locator {
    return this.with({ visible });
  }
  hasText(text: TextMatch): Locator {
    return this.with({ hasText: pattern(text) });
  }
  hasNotText(text: TextMatch): Locator {
    return this.with({ hasNotText: pattern(text) });
  }
  has(inner: Locator): Locator {
    return this.with({ has: inner.spec });
  }
  hasNot(inner: Locator): Locator {
    return this.with({ hasNot: inner.spec });
  }
  nth(index: number): Locator {
    return new Locator({ ...this.spec, nth: index });
  }
  first(): Locator {
    return this.nth(0);
  }
  last(): Locator {
    return this.nth(-1);
  }

  // ---- locators inside this one ----

  locator(css: string): Locator {
    return new Locator({ css, within: this.spec });
  }
  role(role: string, name?: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ ...roleSpec(role, name, opts), within: this.spec });
  }
  button(name?: TextMatch, opts?: Exact): Locator {
    return this.role('button', name, opts);
  }
  link(name?: TextMatch, opts?: Exact): Locator {
    return this.role('link', name, opts);
  }
  textbox(name?: TextMatch, opts?: Exact): Locator {
    return this.role('textbox', name, opts);
  }
  text(text: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ ...textSpec(text, opts), within: this.spec });
  }
  label(text: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ label: pattern(text), ...exact(opts), within: this.spec });
  }
  placeholder(text: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ placeholder: pattern(text), ...exact(opts), within: this.spec });
  }
  testId(id: string): Locator {
    return new Locator({ testId: id, within: this.spec });
  }

  // ---- steps ----

  click(opts: { settle?: boolean; timeout?: number } = {}): StepLike {
    return act(this, 'click', undefined, opts);
  }
  fill(value: string, opts: { settle?: boolean; timeout?: number } = {}): StepLike {
    return act(this, 'fill', value, opts);
  }
  press(key: string, opts: { settle?: boolean; timeout?: number } = {}): StepLike {
    return act(this, 'press', key, opts);
  }
  check(opts: { settle?: boolean; timeout?: number } = {}): StepLike {
    return act(this, 'check', undefined, opts);
  }
  uncheck(opts: { settle?: boolean; timeout?: number } = {}): StepLike {
    return act(this, 'uncheck', undefined, opts);
  }
  focus(): StepLike {
    return act(this, 'focus', undefined, { settle: false });
  }
  /** Reads a value into the run's `values[as]`. */
  read(as: string, what: 'text' | 'value' | 'count' | 'visible' | 'box' = 'text'): StepLike {
    return { toStep: () => ({ read: { locator: this.spec, read: what, as } }) };
  }
}

const exact = (opts: Exact) => (opts.exact ? { exact: true } : {});
const roleSpec = (role: string, name: TextMatch | undefined, opts: Exact): LocatorSpec => ({
  role,
  ...(name === undefined ? {} : { name: pattern(name) }),
  ...exact(opts),
});
const textSpec = (text: TextMatch, opts: Exact): LocatorSpec => ({ text: pattern(text), ...exact(opts) });

function act(loc: Locator, action: string, value: string | undefined, opts: { settle?: boolean; timeout?: number }): StepLike {
  return { toStep: () => ({ act: { locator: loc.spec, action, ...(value === undefined ? {} : { value }), ...opts } }) };
}

// ---- top-level locators ----

export const css = (selector: string) => new Locator({ css: selector });
export const testId = (id: string, attribute?: string) =>
  new Locator(attribute ? { testId: id, testIdAttribute: attribute } : { testId: id });
export const text = (t: TextMatch, opts: Exact = {}) => new Locator(textSpec(t, opts));
export const label = (t: TextMatch, opts: Exact = {}) => new Locator({ label: pattern(t), ...exact(opts) });
export const placeholder = (t: TextMatch, opts: Exact = {}) => new Locator({ placeholder: pattern(t), ...exact(opts) });
export const role = (r: string, name?: TextMatch, opts: Exact = {}) => new Locator(roleSpec(r, name, opts));
export const button = (name?: TextMatch, opts?: Exact) => role('button', name, opts);
export const link = (name?: TextMatch, opts?: Exact) => role('link', name, opts);
export const dialog = (name?: TextMatch, opts?: Exact) => role('dialog', name, opts);
export const heading = (name?: TextMatch, opts?: Exact) => role('heading', name, opts);
export const textbox = (name?: TextMatch, opts?: Exact) => role('textbox', name, opts);
export const checkbox = (name?: TextMatch, opts?: Exact) => role('checkbox', name, opts);
export const combobox = (name?: TextMatch, opts?: Exact) => role('combobox', name, opts);
export const option = (name?: TextMatch, opts?: Exact) => role('option', name, opts);
export const row = (name?: TextMatch, opts?: Exact) => role('row', name, opts);
export const cell = (name?: TextMatch, opts?: Exact) => role('cell', name, opts);
export const tab = (name?: TextMatch, opts?: Exact) => role('tab', name, opts);
export const menuitem = (name?: TextMatch, opts?: Exact) => role('menuitem', name, opts);

// ---- checks: each waits in the browser until it holds ----

type CheckOpts = { timeout?: number };

class Check {
  constructor(
    private loc: Locator,
    private negate = false,
  ) {}
  get not(): Check {
    return new Check(this.loc, !this.negate);
  }
  private step(assert: string, expected?: unknown, opts: CheckOpts = {}): StepLike {
    const body = {
      locator: this.loc.spec,
      assert,
      ...(expected === undefined ? {} : { expected }),
      ...(this.negate ? { not: true } : {}),
      ...opts,
    };
    return { toStep: () => ({ check: body }) };
  }
  visible(opts?: CheckOpts) {
    return this.step('visible', undefined, opts);
  }
  hidden(opts?: CheckOpts) {
    return this.step('hidden', undefined, opts);
  }
  text(expected: string, opts?: CheckOpts) {
    return this.step('text', expected, opts);
  }
  containsText(expected: string, opts?: CheckOpts) {
    return this.step('containsText', expected, opts);
  }
  value(expected: string, opts?: CheckOpts) {
    return this.step('value', expected, opts);
  }
  count(expected: number, opts?: CheckOpts) {
    return this.step('count', expected, opts);
  }
  checked(expected = true, opts?: CheckOpts) {
    return this.step('checked', expected, opts);
  }
}

class RequestCheck implements StepLike {
  constructor(private m: Record<string, unknown>) {}
  status(status: number): RequestCheck {
    return new RequestCheck({ ...this.m, status });
  }
  count(count: number): RequestCheck {
    return new RequestCheck({ ...this.m, count });
  }
  timeout(timeout: number): RequestCheck {
    return new RequestCheck({ ...this.m, timeout });
  }
  toStep(): Step {
    return { request: this.m };
  }
}

/**
 * `check(locator).visible()`, `.text("…")`, `.count(n)`, `.not.visible()`;
 * `check.request("POST", "/path").status(200)` checks the run's request log
 * (`path` is exact, a RegExp tests the full URL).
 */
export const check = Object.assign((loc: Locator) => new Check(loc), {
  request: (method: string, path: string | RegExp) =>
    new RequestCheck(typeof path === 'string' ? { method, path } : { method, url: pattern(path) }),
});

// ---- navigation and escape hatches ----

export const goto = (url: string, waitUntil: 'load' | 'domcontentloaded' = 'load'): StepLike => ({
  toStep: () => ({ goto: { url, waitUntil } }),
});
/** Same-document navigation (pushState + popstate), as a client-side router sees a link. */
export const route = (url: string): StepLike => ({ toStep: () => ({ route: { url } }) });
/** Evaluates an expression in the page; its JSON value lands in `values[as]`. */
export const evaluate = (expression: string, as?: string): StepLike => ({
  toStep: () => ({ eval: as ? { expression, as } : { expression } }),
});
export const loaded = (): StepLike => ({ toStep: () => ({ loaded: {} }) });
/**
 * Waits until the page is quiet: no queued task, request in flight, timeout due
 * within 50 ms, pending animation frame or unfinished navigation. Never fails;
 * when `timeout` passes first its result has `settled: false` and `pending`.
 */
export const idle = (opts: { timeout?: number } = {}): StepLike => ({
  toStep: () => ({ idle: opts.timeout === undefined ? {} : { timeout: opts.timeout } }),
});
