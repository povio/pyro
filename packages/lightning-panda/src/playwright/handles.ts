// Playwright's JSHandle, ElementHandle and Download on lightningpanda.
//
// Handles are CDP remote objects (`Runtime.*`). A handle to an element a
// locator found comes from an LP.run `eval` step that hands the element over
// (`Page._handleFrom`). Element handle actions run as native LP.run actions
// on a locator for that element: it carries a `data-lp-handle` attribute
// for the length of the action.
import { copyFileSync, createReadStream, mkdirSync, rmSync } from 'node:fs';
import { dirname } from 'node:path';
import type { LocatorSpec } from '../steps.js';
import type { Frame, Locator, Page } from './page.js';

// ---- Playwright's function-argument typing (PageFunction, Unboxed, SmartHandle) ----

type NoHandles<Arg> = Arg extends JSHandle ? never : Arg extends object ? { [Key in keyof Arg]: NoHandles<Arg[Key]> } : Arg;
/** What a handle in an `evaluate` argument becomes in the page: its value. */
export type Unboxed<Arg> = Arg extends ElementHandle<infer T>
  ? T
  : Arg extends JSHandle<infer T>
    ? T
    : Arg extends NoHandles<Arg>
      ? Arg
      : Arg extends [infer A0]
        ? [Unboxed<A0>]
        : Arg extends [infer A0, infer A1]
          ? [Unboxed<A0>, Unboxed<A1>]
          : Arg extends [infer A0, infer A1, infer A2]
            ? [Unboxed<A0>, Unboxed<A1>, Unboxed<A2>]
            : Arg extends [infer A0, infer A1, infer A2, infer A3]
              ? [Unboxed<A0>, Unboxed<A1>, Unboxed<A2>, Unboxed<A3>]
              : Arg extends Array<infer T>
                ? Array<Unboxed<T>>
                : Arg extends object
                  ? { [Key in keyof Arg]: Unboxed<Arg[Key]> }
                  : Arg;
/** A function run in the page with `arg` (or a string expression). */
export type PageFunction<Arg, R> = string | ((arg: Unboxed<Arg>) => R | Promise<R>);
/** A function run in the page on `on` (an element, elements, a value) with `arg2`. */
export type PageFunctionOn<On, Arg2, R> = string | ((on: On, arg2: Unboxed<Arg2>) => R | Promise<R>);
/** A handle to `T`: an ElementHandle for a node, else a JSHandle. */
export type SmartHandle<T> = T extends Node ? ElementHandle<T> : JSHandle<T>;
/** The elements locators and element handles evaluate on. */
export type PageElement = SVGElement | HTMLElement;

/** Where a handle's CDP commands go: its page's session, or a worker's. */
export type Sender = (method: string, params?: object) => Promise<any>;

type Remote = { type: string; subtype?: string; objectId?: string; value?: unknown; unserializableValue?: string; description?: string; className?: string };
type Timeout = { timeout?: number };

const unserializable = (v: string): unknown =>
  v === 'NaN' ? NaN : v === 'Infinity' ? Infinity : v === '-Infinity' ? -Infinity : v === '-0' ? -0 : v.endsWith('n') ? BigInt(v.slice(0, -1)) : undefined;

export function valueOf(r: Remote): unknown {
  if (r.unserializableValue !== undefined) return unserializable(r.unserializableValue);
  return r.value;
}

/** `arg` with handles taken out: they go as their own arguments of `Runtime.callFunctionOn`. */
export function serializeArg(arg: unknown): { template: unknown; handles: JSHandle[] } {
  const handles: JSHandle[] = [];
  const walk = (v: unknown): unknown => {
    if (v instanceof JSHandle) {
      handles.push(v);
      return { __lpHandle: handles.length - 1 };
    }
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === 'object' && !(v instanceof Date) && !(v instanceof RegExp)) {
      return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, walk(x)]));
    }
    return v;
  };
  return { template: walk(arg), handles };
}

export const hasHandles = (arg: unknown): boolean => serializeArg(arg).handles.length > 0;

/** A function declaration calling `fn` with `this` (when `self`) and the rebuilt argument. */
export function callDeclaration(fn: string | Function, self: boolean): string {
  const body = typeof fn === 'function' ? fn.toString() : `(${self ? 'x' : ''}) => (${fn})`;
  return `async function (__tpl, ...__h) {
    const r = (v) => v && typeof v === 'object'
      ? (typeof v.__lpHandle === 'number' && Object.keys(v).length === 1 ? __h[v.__lpHandle]
        : Array.isArray(v) ? v.map(r) : Object.fromEntries(Object.entries(v).map(([k, x]) => [k, r(x)])))
      : v;
    const f = (${body}\n);
    return ${self ? 'await f(this, r(__tpl))' : 'await f(r(__tpl))'};
  }`;
}

// The value's type, compile time only (an interface, not a `declare` field,
// which Playwright's and other Babel transforms reject).
export interface JSHandle<T = any> {
  readonly __type?: T;
}
export class JSHandle<T = any> {
  private _disposed = false;
  constructor(
    readonly _page: Page,
    readonly _remote: Remote,
    /** The `<iframe>` chain of the frame the object lives in (none: the main frame). */
    readonly _frame?: LocatorSpec,
    /** A worker's session instead of the page's. */
    readonly _via?: Sender,
  ) {}
  _send(method: string, params: object = {}): Promise<any> {
    return this._via ? this._via(method, params) : this._page._send(method, params);
  }
  get _objectId(): string | undefined {
    return this._remote.objectId;
  }
  async evaluate<R, Arg, O extends T = T>(pageFunction: PageFunctionOn<O, Arg, R>, arg?: Arg): Promise<R> {
    return this._page._callOn(this, pageFunction, arg, true) as Promise<R>;
  }
  async evaluateHandle<R, Arg, O extends T = T>(pageFunction: PageFunctionOn<O, Arg, R>, arg?: Arg): Promise<SmartHandle<R>> {
    return this._page._callOn(this, pageFunction, arg, false) as Promise<SmartHandle<R>>;
  }
  async jsonValue(): Promise<T> {
    if (!this._objectId) return valueOf(this._remote) as T;
    return this._page._callOn(this, (v: unknown) => v, undefined, true) as Promise<T>;
  }
  async getProperty(name: string): Promise<JSHandle> {
    return this._page._callOn(this, (o: any, n: string) => o[n], name, false) as Promise<JSHandle>;
  }
  async getProperties(): Promise<Map<string, JSHandle>> {
    const map = new Map<string, JSHandle>();
    if (!this._objectId) return map;
    const { result } = await this._send('Runtime.getProperties', { objectId: this._objectId, ownProperties: true });
    for (const p of result as { name: string; enumerable?: boolean; value?: Remote }[]) {
      if (p.value && p.enumerable !== false) map.set(p.name, this._page._handle(p.value, this._frame, this._via));
    }
    return map;
  }
  asElement(): ElementHandle<any> | null {
    return null;
  }
  /** `fn` on this value, its JSON value (internal). */
  protected _eval<R>(fn: (on: any, arg: any) => R, arg?: unknown): Promise<Awaited<R>> {
    return this._page._callOn(this, fn, arg, true) as Promise<Awaited<R>>;
  }
  protected _evalHandle(fn: (on: any, arg: any) => unknown, arg?: unknown): Promise<JSHandle> {
    return this._page._callOn(this, fn, arg, false) as Promise<JSHandle>;
  }
  async dispose(): Promise<void> {
    if (this._disposed || !this._objectId) return;
    this._disposed = true;
    await this._send('Runtime.releaseObject', { objectId: this._objectId }).catch(() => {});
  }
  async [Symbol.asyncDispose](): Promise<void> {
    await this.dispose();
  }
  toString(): string {
    return this._objectId ? `JSHandle@${this._remote.subtype ?? this._remote.type}` : `JSHandle@${String(valueOf(this._remote))}`;
  }
}

let marks = 0;

export class ElementHandle<T = Node> extends JSHandle<T> {
  override asElement(): ElementHandle<T> {
    return this;
  }
  override toString(): string {
    return `JSHandle@${this._remote.description ?? 'node'}`;
  }

  /** Runs `fn` with a locator for this element (it carries a marker attribute meanwhile). */
  private async marked<T>(fn: (loc: Locator) => Promise<T>): Promise<T> {
    const id = `h${++marks}`;
    const connected = await this._eval((el: Element, mark: string) => {
      if (!el.isConnected || el.nodeType !== 1) return false;
      el.setAttribute('data-lp-handle', mark);
      return true;
    }, id);
    if (!connected) throw new Error('elementHandle: Element is not attached to the DOM');
    try {
      return await fn(this._page._locatorFor({ css: `[data-lp-handle="${id}"]`, ...(this._frame ? { frame: this._frame } : {}) }, `elementHandle(${id})`));
    } finally {
      await this._eval((el: Element) => el.removeAttribute('data-lp-handle')).catch(() => {});
    }
  }

  // ---- actions (native, on a locator for this element) ----

  click(o?: Parameters<Locator['click']>[0]) { return this.marked((l) => l.click(o)); }
  dblclick(o?: Parameters<Locator['dblclick']>[0]) { return this.marked((l) => l.dblclick(o)); }
  tap(o?: Parameters<Locator['tap']>[0]) { return this.marked((l) => l.tap(o)); }
  hover(o?: Parameters<Locator['hover']>[0]) { return this.marked((l) => l.hover(o)); }
  fill(value: string, o?: Timeout) { return this.marked((l) => l.fill(value, o)); }
  press(key: string, o?: Timeout) { return this.marked((l) => l.press(key, o)); }
  pressSequentially(text: string, o?: Timeout) { return this.marked((l) => l.pressSequentially(text, o)); }
  check(o?: Timeout) { return this.marked((l) => l.check(o)); }
  uncheck(o?: Timeout) { return this.marked((l) => l.uncheck(o)); }
  setChecked(checked: boolean, o?: Timeout) { return this.marked((l) => l.setChecked(checked, o)); }
  focus() { return this.marked((l) => l.focus()); }
  selectOption(values: Parameters<Locator['selectOption']>[0], o?: Timeout) { return this.marked((l) => l.selectOption(values, o)); }
  selectText(o?: Timeout) { return this.marked((l) => l.selectText(o)); }
  setInputFiles(files: Parameters<Locator['setInputFiles']>[0], o?: Timeout) { return this.marked((l) => l.setInputFiles(files, o)); }
  dispatchEvent(type: string, init?: Record<string, unknown>) { return this.marked((l) => l.dispatchEvent(type, init)); }
  scrollIntoViewIfNeeded() { return this._eval((el: Element) => el.scrollIntoView({ block: 'nearest', inline: 'nearest' })); }
  screenshot(o?: Parameters<Locator['screenshot']>[0]) { return this.marked((l) => l.screenshot(o)); }
  isVisible() { return this.marked((l) => l.isVisible()); }
  async isHidden() { return !(await this.isVisible()); }
  isEnabled() { return this.marked((l) => l.isEnabled()); }
  async isDisabled() { return !(await this.isEnabled()); }
  isEditable() { return this.marked((l) => l.isEditable()); }
  isChecked() { return this.marked((l) => l.isChecked()); }
  inputValue() { return this.marked((l) => l.inputValue()); }

  // ---- reads ----

  getAttribute(name: string): Promise<string | null> {
    return this._eval((el: Element, n: string) => el.getAttribute(n), name);
  }
  textContent(): Promise<string | null> {
    return this._eval((el: Node) => el.textContent);
  }
  innerText(): Promise<string> {
    return this._eval((el: HTMLElement) => el.innerText ?? el.textContent ?? '');
  }
  innerHTML(): Promise<string> {
    return this._eval((el: Element) => el.innerHTML);
  }
  async boundingBox(): Promise<{ x: number; y: number; width: number; height: number } | null> {
    return this._eval((el: Element) => {
      const r = el.getBoundingClientRect();
      return r.width === 0 && r.height === 0 ? null : { x: r.x, y: r.y, width: r.width, height: r.height };
    });
  }
  async ownerFrame(): Promise<Frame> {
    return this._page._frameOf(this._frame) ?? this._page.mainFrame();
  }
  /** The frame of an `<iframe>` element, else null. */
  async contentFrame(): Promise<Frame | null> {
    const isFrame = await this._eval((el: Element) => el.localName === 'iframe' || el.localName === 'frame');
    if (!isFrame) return null;
    await this._page._markFrames();
    const id = await this.getAttribute('data-lp-frame');
    return id ? this._page._frameById(id) : null;
  }
  async waitForElementState(state: 'visible' | 'hidden' | 'stable' | 'enabled' | 'disabled' | 'editable', o: Timeout = {}): Promise<void> {
    await this.marked(async (l) => {
      if (state === 'visible' || state === 'hidden') return l.waitFor({ state, timeout: o.timeout });
      if (state === 'stable') return;
      const deadline = Date.now() + (o.timeout ?? 30_000);
      for (;;) {
        const ok = state === 'enabled' ? await l.isEnabled() : state === 'disabled' ? !(await l.isEnabled()) : await l.isEditable();
        if (ok) return;
        if (Date.now() > deadline) throw new Error(`elementHandle.waitForElementState: Timeout exceeded waiting for "${state}"`);
        await new Promise((r) => setTimeout(r, 50));
      }
    });
  }

  // ---- queries inside the element (CSS) ----

  async $(selector: string): Promise<ElementHandle<PageElement> | null> {
    const h = await this._evalHandle((el: Element, s: string) => el.querySelector(s), selector);
    return h.asElement();
  }
  async $$(selector: string): Promise<ElementHandle<PageElement>[]> {
    const list = await this._evalHandle((el: Element, s: string) => Array.from(el.querySelectorAll(s)), selector);
    const props = await list.getProperties();
    await list.dispose();
    return [...props.values()].map((h) => h.asElement()).filter((h): h is ElementHandle<PageElement> => !!h);
  }
  async $eval<R, Arg, E extends PageElement = PageElement>(selector: string, fn: PageFunctionOn<E, Arg, R>, arg?: Arg): Promise<R> {
    const h = await this.$(selector);
    if (!h) throw new Error(`elementHandle.$eval: Failed to find element matching selector "${selector}"`);
    try {
      return await h.evaluate(fn as PageFunctionOn<Node, Arg, R>, arg);
    } finally {
      await h.dispose();
    }
  }
  async $$eval<R, Arg, E extends PageElement = PageElement>(selector: string, fn: PageFunctionOn<E[], Arg, R>, arg?: Arg): Promise<R> {
    const list = await this._evalHandle((el: Element, s: string) => Array.from(el.querySelectorAll(s)), selector);
    try {
      return (await list.evaluate(fn as any, arg)) as R;
    } finally {
      await list.dispose();
    }
  }
}

/** Playwright's Download: the file lands in the browser's download directory under a guid. */
export class Download {
  private _done: Promise<{ path: string | null; failure: string | null }>;
  _finish!: (r: { path: string | null; failure: string | null }) => void;
  constructor(
    private _page: Page,
    private _url: string,
    private _suggested: string,
  ) {
    this._done = new Promise((r) => (this._finish = r));
  }
  page(): Page {
    return this._page;
  }
  url(): string {
    return this._url;
  }
  suggestedFilename(): string {
    return this._suggested;
  }
  async path(): Promise<string> {
    const { path, failure } = await this._done;
    if (!path) throw new Error(`download.path: ${failure ?? 'download failed'}`);
    return path;
  }
  async saveAs(to: string): Promise<void> {
    const from = await this.path();
    mkdirSync(dirname(to), { recursive: true });
    copyFileSync(from, to);
  }
  async failure(): Promise<string | null> {
    return (await this._done).failure;
  }
  async createReadStream() {
    return createReadStream(await this.path());
  }
  async delete(): Promise<void> {
    const { path } = await this._done;
    if (path) rmSync(path, { force: true });
  }
  async cancel(): Promise<void> {}
}
