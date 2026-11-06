// Locators and steps as plain data: `page.run([...])` sends them to the
// browser in one LP.run command. Builders are Playwright-shaped
// (`button("Save")`, `dialog().hasText("…").button("OK")`, `.first()`), but
// nothing here talks to the browser. The wire shapes are those of
// `crates/lp-cdp/src/engine/driver.rs` (StepParams and friends).

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
  /** Search inside the document of the `<iframe>` this locator matches (in the parent document). */
  frame?: LocatorSpec;
  /** An XPath expression (relative to `within`, else the document). */
  xpath?: string;
  /** Another locator's matches as the base set (a filter or `nth` after an `nth`). */
  of?: LocatorSpec;
  /** Joined with these locators' matches, in document order (Playwright's `or`). */
  or?: LocatorSpec[];
  /** Kept only where each of these matches too (Playwright's `and`). */
  and?: LocatorSpec[];
  /** `role` states (Playwright's `getByRole` options). */
  checked?: boolean | 'mixed';
  pressed?: boolean | 'mixed';
  disabled?: boolean;
  expanded?: boolean;
  selected?: boolean;
  level?: number;
  includeHidden?: boolean;
};

export type Step = Record<string, unknown>;
type Exact = { exact?: boolean };

/** Anything that becomes one step of a run. */
export interface StepLike {
  toStep(): Step;
}

export type ActOptions = { settle?: boolean; timeout?: number };
/**
 * Pointer options of `click`, `dblclick`, `hover` and `wheel` (engine 0.0.10):
 * `button` (left), `position` relative to the element's top-left corner
 * (its center), `force` skips the actionability checks.
 */
export type PointerOptions = ActOptions & {
  button?: 'left' | 'right' | 'middle';
  position?: { x: number; y: number };
  force?: boolean;
};
/** A file for `upload`: inline contents (`text` or `base64`) and an optional type. */
export type UploadFile = { name: string; text?: string; base64?: string; mimeType?: string };
export type ReadWhat = 'text' | 'value' | 'count' | 'visible' | 'box' | 'css' | 'name' | 'focused' | 'enabled';

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

  /** `spec` scoped to this locator; a `frame` stays on the outermost locator. */
  private inner(spec: LocatorSpec): Locator {
    const { frame, ...within } = this.spec;
    return new Locator({ ...spec, within, ...(frame ? { frame } : {}) });
  }
  locator(css: string): Locator {
    return this.inner({ css });
  }
  role(role: string, name?: TextMatch, opts: Exact = {}): Locator {
    return this.inner(roleSpec(role, name, opts));
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
    return this.inner(textSpec(text, opts));
  }
  label(text: TextMatch, opts: Exact = {}): Locator {
    return this.inner({ label: pattern(text), ...exact(opts) });
  }
  placeholder(text: TextMatch, opts: Exact = {}): Locator {
    return this.inner({ placeholder: pattern(text), ...exact(opts) });
  }
  testId(id: string): Locator {
    return this.inner({ testId: id });
  }

  // ---- frames ----

  /**
   * Locators inside the document of the `<iframe>` this locator matches:
   * `css('#editor').frame().button('Bold')` (Playwright's `frameLocator`).
   */
  frame(): FrameScope {
    return new FrameScope(this);
  }
  /** This locator, searched in the document of the `<iframe>` `frame` matches. */
  inFrame(frame: Locator): Locator {
    return new Locator({ ...this.spec, frame: frame.spec });
  }

  // ---- steps ----

  click(opts: PointerOptions = {}): StepLike {
    return act(this, 'click', undefined, opts);
  }
  /** Two clicks at the center (the second with click count 2). */
  dblclick(opts: PointerOptions = {}): StepLike {
    return act(this, 'dblclick', undefined, opts);
  }
  /** Moves the pointer to the element (tooltips, hover toolbars). */
  hover(opts: PointerOptions = {}): StepLike {
    return act(this, 'hover', undefined, opts);
  }
  fill(value: string, opts: ActOptions = {}): StepLike {
    return act(this, 'fill', value, opts);
  }
  /** Focuses, then one key press per character (Playwright's `pressSequentially`). */
  type(text: string, opts: ActOptions = {}): StepLike {
    return act(this, 'type', text, opts);
  }
  press(key: string, opts: ActOptions = {}): StepLike {
    return act(this, 'press', key, opts);
  }
  check(opts: ActOptions = {}): StepLike {
    return act(this, 'check', undefined, opts);
  }
  uncheck(opts: ActOptions = {}): StepLike {
    return act(this, 'uncheck', undefined, opts);
  }
  focus(): StepLike {
    return act(this, 'focus', undefined, { settle: false });
  }
  blur(): StepLike {
    return act(this, 'blur', undefined, { settle: false });
  }
  /**
   * Sets the files of an `<input type=file>`: inline files, or a path (several:
   * one per line) on the machine running Lightningpanda.
   */
  upload(files: UploadFile[] | string, opts: ActOptions = {}): StepLike {
    return typeof files === 'string'
      ? act(this, 'upload', files, opts)
      : act(this, 'upload', undefined, { ...opts, files });
  }
  dropFiles(files: UploadFile[] | string, opts: ActOptions = {}): StepLike {
    return typeof files === 'string'
      ? act(this, 'dropFiles', files, opts)
      : act(this, 'dropFiles', undefined, { ...opts, files });
  }
  /** Drags the element onto `to`: native drag and drop when it is a drag source, else a pointer drag. */
  drag(to: Locator, opts: ActOptions = {}): StepLike {
    return act(this, 'drag', undefined, { ...opts, to: to.spec });
  }
  /** Scrolls with the mouse wheel over the element (engine 0.0.10). */
  wheel(deltaX: number, deltaY: number, opts: ActOptions = {}): StepLike {
    return act(this, 'wheel', undefined, { ...opts, deltaX, deltaY });
  }
  /** Compares this element's box with the baseline `name` (see `matchScreenshot`). */
  matchScreenshot(name: string, opts: ScreenshotOptions = {}): ScreenshotStep {
    return new ScreenshotStep(name, { ...opts, locator: this.spec }, callerDir());
  }
  /**
   * Reads a value into the run's `values[as]`: `read('n', 'css', 'color')`
   * for a computed style property, `'name'` for the accessible name.
   */
  read(as: string, what: ReadWhat = 'text', property?: string): StepLike {
    if (what === 'css' && !property) throw new Error("read(as, 'css', property) needs the property");
    const body = { locator: this.spec, read: what, as, ...(property === undefined ? {} : { property }) };
    return { toStep: () => ({ read: body }) };
  }
}

/** Locators inside an `<iframe>`'s document (`loc.frame()`). */
export class FrameScope {
  constructor(private frameLocator: Locator) {}
  locator(css: string): Locator {
    return css_(css).inFrame(this.frameLocator);
  }
  role(role: string, name?: TextMatch, opts: Exact = {}): Locator {
    return new Locator(roleSpec(role, name, opts)).inFrame(this.frameLocator);
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
    return new Locator(textSpec(text, opts)).inFrame(this.frameLocator);
  }
  label(text: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ label: pattern(text), ...exact(opts) }).inFrame(this.frameLocator);
  }
  placeholder(text: TextMatch, opts: Exact = {}): Locator {
    return new Locator({ placeholder: pattern(text), ...exact(opts) }).inFrame(this.frameLocator);
  }
  testId(id: string): Locator {
    return new Locator({ testId: id }).inFrame(this.frameLocator);
  }
  /** A nested `<iframe>` inside this one. */
  frame(inner: Locator): FrameScope {
    return inner.inFrame(this.frameLocator).frame();
  }
}

const exact = (opts: Exact) => (opts.exact ? { exact: true } : {});
const roleSpec = (role: string, name: TextMatch | undefined, opts: Exact): LocatorSpec => ({
  role,
  ...(name === undefined ? {} : { name: pattern(name) }),
  ...exact(opts),
});
const textSpec = (text: TextMatch, opts: Exact): LocatorSpec => ({ text: pattern(text), ...exact(opts) });

function act(loc: Locator, action: string, value: string | undefined, opts: Record<string, unknown>): StepLike {
  return { toStep: () => ({ act: { locator: loc.spec, action, ...(value === undefined ? {} : { value }), ...opts } }) };
}

// ---- top-level locators ----

const css_ = (selector: string) => new Locator({ css: selector });
export const css = css_;
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
/** Locators inside the `<iframe>` `frame` matches: `inFrame(css('#editor')).button('Bold')`. */
export const inFrame = (frame: Locator) => frame.frame();

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
  private step(assert: string, expected?: unknown, opts: CheckOpts = {}, extra: Record<string, unknown> = {}): StepLike {
    const body = {
      locator: this.loc.spec,
      assert,
      ...(expected === undefined ? {} : { expected }),
      ...(this.negate ? { not: true } : {}),
      ...extra,
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
  /** A computed style property: `check(button('Save')).css('background-color', 'rgb(1, 2, 3)')`. */
  css(property: string, expected: string, opts?: CheckOpts) {
    return this.step('css', expected, opts, { property });
  }
  /** The element is the document's focused element. */
  focused(expected = true, opts?: CheckOpts) {
    return this.step('focused', expected, opts);
  }
  /** The accessible name (exact after whitespace normalization). */
  name(expected: string, opts?: CheckOpts) {
    return this.step('name', expected, opts);
  }
  enabled(expected = true, opts?: CheckOpts) {
    return this.step('enabled', expected, opts);
  }
  disabled(opts?: CheckOpts) {
    return this.step('enabled', false, opts);
  }
}

class RequestCheck implements StepLike {
  constructor(private m: Record<string, unknown>) {}
  /** One status, or any of several (`[200, 304]`). */
  status(status: number | number[]): RequestCheck {
    return new RequestCheck({ ...this.m, status });
  }
  count(count: number): RequestCheck {
    return new RequestCheck({ ...this.m, count });
  }
  /** The request body contains (or matches) `body` (engine 0.0.10). */
  body(body: TextMatch): RequestCheck {
    return new RequestCheck({ ...this.m, body: pattern(body) });
  }
  /** The request body is a JSON object with exactly these top-level keys (engine 0.0.10). */
  bodyKeys(keys: string[]): RequestCheck {
    return new RequestCheck({ ...this.m, bodyKeys: keys });
  }
  /** Request headers (case-insensitive names) whose values contain (or match) these (engine 0.0.10). */
  headers(headers: Record<string, TextMatch>): RequestCheck {
    const wire = Object.fromEntries(Object.entries(headers).map(([k, v]) => [k, pattern(v)]));
    return new RequestCheck({ ...this.m, headers: wire });
  }
  /**
   * `started` also counts requests still in flight (`count` included);
   * `finished` (the default) only those with a response or an error (engine 0.0.10).
   */
  state(state: 'started' | 'finished'): RequestCheck {
    return new RequestCheck({ ...this.m, state });
  }
  /** Include prior runs of this page only; batch scope is the default. */
  scope(scope: 'batch' | 'context'): RequestCheck {
    return new RequestCheck({ ...this.m, scope });
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
/**
 * Same-document navigation (pushState + popstate), as a client-side router
 * sees a link. Settles on the requests it starts unless `settle: false`.
 */
export const route = (url: string, opts: ActOptions = {}): StepLike => ({ toStep: () => ({ route: { url, ...opts } }) });
/**
 * Evaluates an expression in the page; its JSON value lands in `values[as]`.
 * A returned promise is awaited (within `timeout`); a rejection fails the step.
 */
export const evaluate = (expression: string, as?: string, opts: CheckOpts = {}): StepLike => ({
  toStep: () => ({ eval: { expression, ...(as ? { as } : {}), ...opts } }),
});
/**
 * Waits for the current document's `load`. On a page taken from a pool while
 * still booting (`lp.page()` adds it), it waits for the profile URL's
 * navigation to commit and load, never for the `about:blank` before it.
 */
export const loaded = (opts: CheckOpts = {}): StepLike => ({ toStep: () => ({ loaded: opts }) });
/**
 * Waits until the page is quiet: no queued task, request in flight, timeout due
 * within 50 ms, pending animation frame or unfinished navigation. Never fails;
 * when `timeout` passes first its result has `settled: false` and `pending`.
 */
export const idle = (opts: { timeout?: number } = {}): StepLike => ({
  toStep: () => ({ idle: opts.timeout === undefined ? {} : { timeout: opts.timeout } }),
});
/** Reloads the current document and waits for its `load`. */
export const reload = (opts: CheckOpts = {}): StepLike => ({ toStep: () => ({ reload: opts }) });
/** Waits until the page URL contains (or matches) `url` (Playwright's `waitForURL`). */
export const waitForUrl = (url: TextMatch, opts: CheckOpts = {}): StepLike => ({
  toStep: () => ({ url: { url: pattern(url), ...opts } }),
});

// ---- network mocks ----

export type MockOptions = {
  method?: string;
  status?: number;
  /** Serialized as the body (content type `application/json`). */
  json?: unknown;
  body?: string;
  contentType?: string;
  headers?: Record<string, string>;
  /** Milliseconds before the response (loading states). */
  delay?: number;
  /** Fail the request instead, with a `Network.ErrorReason` such as `Failed` or `ConnectionRefused`. */
  abort?: string;
  /** Answer only the first n matching requests. */
  times?: number;
  /** Let matching requests reach the network unchanged (an exception to a broader mock; engine 0.0.10). */
  passthrough?: boolean;
  /** Hold matching requests under this name until a `release(name)` step (engine 0.0.10). */
  hold?: string;
  /** Let the request reach the server, then rewrite the fields given of its response (engine 0.0.10). */
  patch?: { status?: number; headers?: Record<string, string>; json?: unknown; body?: string };
};

/**
 * Answers requests of this page's context whose URL contains `url` (or
 * matches the RegExp) with the given response (Playwright's `page.route` +
 * `fulfill`): `mock('/api/items', { json: [] })`.
 */
export const mock = (url: TextMatch, opts: MockOptions = {}): StepLike => ({
  toStep: () => ({ mock: { url: pattern(url), ...opts } }),
});
/** Removes the mocks matching `url` (all of them without one). */
export const unmock = (url?: TextMatch): StepLike => ({
  toStep: () => ({ unmock: url === undefined ? {} : { url: pattern(url) } }),
});
/** Lets the requests held by `mock(url, { hold: name })` go on (engine 0.0.10). */
export const release = (hold: string): StepLike => ({ toStep: () => ({ release: { hold } }) });

// ---- emulation ----

/** Resizes the viewport mid-test; layout and `resize` listeners see the new size (engine 0.0.10). */
export const viewport = (width: number, height: number): StepLike => ({
  toStep: () => ({ viewport: { width, height } }),
});
export type MediaOptions = {
  colorScheme: 'light' | 'dark' | 'no-preference';
  reducedMotion?: 'reduce' | 'no-preference';
  /** `screen` or `print`. */
  media?: string;
};
/** Emulates media features (`prefers-color-scheme` and friends) (engine 0.0.10). */
export const media = (opts: MediaOptions): StepLike => ({ toStep: () => ({ media: opts }) });

// ---- downloads, popups and the raw mouse ----

/** Reads this page's native initiated-download count without consuming download records. */
export const downloadCount = (as: string): StepLike => ({
  toStep: () => ({ downloadCount: { as } }),
});
/**
 * Waits for the next download the page starts during the run; its
 * `{ url, filename, text, ... }` lands in `values[as]`, and the file in
 * `saveTo` (a directory) when given.
 */
export const download = (opts: { saveTo?: string; as?: string; timeout?: number } = {}): StepLike => ({
  toStep: () => ({ download: opts }),
});
/**
 * The following steps drive another page of the context (engine 0.0.10):
 * the `as` name of an earlier `popup(as)`, a target id, or `'main'` for the
 * page the run started on. Add `loaded()` or `waitForUrl()` after switching
 * to a fresh popup.
 */
export const switchTo = (target: string): StepLike => ({ toStep: () => ({ page: { target } }) });
/** Waits for the next popup the page opens during the run; its target id lands in `values[as]`. */
export const popup = (as?: string, opts: CheckOpts = {}): StepLike => ({
  toStep: () => ({ popup: { ...(as ? { as } : {}), ...opts } }),
});

export type MouseAction = 'click' | 'dblclick' | 'move' | 'down' | 'up' | 'wheel';
export type MouseOptions = {
  /** Viewport point; required for `click`, `dblclick` and `move`, else the pointer stays where it is. */
  x?: number;
  y?: number;
  button?: 'left' | 'right' | 'middle';
  /** `wheel`: scroll deltas. */
  deltaX?: number;
  deltaY?: number;
  /** `move`: intermediate `mousemove` events. */
  steps?: number;
  settle?: boolean;
  timeout?: number;
};
/** A raw mouse event at viewport coordinates (engine 0.0.10): `mouse('click', { x: 10, y: 20 })`. */
export const mouse = (action: MouseAction, opts: MouseOptions = {}): StepLike => ({
  toStep: () => ({ mouse: { action, ...opts } }),
});
/** Mouse wheel over an element (engine 0.0.10). */
export const wheel = (loc: Locator, deltaX: number, deltaY: number, opts: ActOptions = {}): StepLike =>
  loc.wheel(deltaX, deltaY, opts);

// ---- screenshots ----

export type ScreenshotOptions = {
  /** 0-1: how far a pixel's channels may differ and still match (default 0: exact). */
  threshold?: number;
  /** Differing pixels allowed (default 0). */
  maxDiffPixels?: number;
  /** Differing pixels allowed as a fraction of the image. */
  maxDiffPixelRatio?: number;
  /** A rectangle of the document, CSS px. */
  clip?: { x: number; y: number; width: number; height: number };
  /** The whole document, not only the viewport. */
  fullPage?: boolean;
  /** Where baselines live (default: `__screenshots__` beside the test file). */
  dir?: string;
  timeout?: number;
};

/**
 * A `matchScreenshot` step: the page renders in the browser and is compared
 * there with the baseline `<dir>/<name>.png`. `page.run` reads the baseline
 * (sent once per connection), writes it when missing or in update mode, and
 * writes `<name>.actual.png` and `<name>.diff.png` on a mismatch.
 */
export class ScreenshotStep implements StepLike {
  /** Baseline fields `page.run` adds (`baseline`, `sceneHash`, `update`). */
  wire: Record<string, unknown> = {};

  constructor(
    readonly name: string,
    readonly opts: ScreenshotOptions & { locator?: LocatorSpec },
    /** Directory of the test file that built the step. */
    readonly testDir: string,
    /** Off for Playwright's snapshot paths, already resolved to `opts.dir` + `name`. */
    checkName = true,
    /** Path stem a mismatch writes `-actual.png`, `-expected.png` and `-diff.png` to (Playwright's test output dir); beside the baseline when unset. */
    readonly outputStem?: string,
  ) {
    if (checkName && (!/^[\w.@-][\w.@/-]*$/.test(name) || name.includes('..'))) {
      throw new Error(`screenshot name '${name}': use letters, digits, '.', '-', '_', '@' and '/'`);
    }
  }

  toStep(): Step {
    const { dir: _dir, ...opts } = this.opts;
    return { matchScreenshot: { name: this.name, ...opts, ...this.wire } };
  }
}

/** Compares the viewport (or `clip`, or `fullPage`) with the baseline `name`. */
export const matchScreenshot = (name: string, opts: ScreenshotOptions = {}): ScreenshotStep =>
  new ScreenshotStep(name, opts, callerDir());

/** Directory of the first stack frame outside this package (the test file). */
function callerDir(): string {
  const here = import.meta.url.replace(/^file:\/\//, '').replace(/[^/]*$/, '');
  for (const line of (new Error().stack ?? '').split('\n').slice(1)) {
    const m = /(?:\(|at )(?:file:\/\/)?(\/[^():]+):\d+(?::\d+)?\)?\s*$/.exec(line);
    if (m && !m[1].startsWith(here)) return m[1].replace(/\/[^/]*$/, '');
  }
  return process.cwd();
}
