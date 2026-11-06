// Playwright's JSHandle, ElementHandle and Download on lightningpanda.
//
// Handles are CDP remote objects (`Runtime.*`). A handle to an element a
// locator found comes from an LP.run `eval` step that hands the element over
// (`Page._handleFrom`). Element handle actions run as native LP.run actions
// on a locator for that element: it carries a `data-lp-handle` attribute
// for the length of the action.
import { copyFileSync, createReadStream, mkdirSync, rmSync } from 'node:fs';
import { dirname } from 'node:path';
import type { Locator, Page } from './page.js';

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

export class JSHandle {
  private _disposed = false;
  constructor(
    readonly _page: Page,
    readonly _remote: Remote,
  ) {}
  get _objectId(): string | undefined {
    return this._remote.objectId;
  }
  async evaluate<R, A = unknown>(fn: string | ((self: any, arg: A) => R | Promise<R>), arg?: A): Promise<R> {
    return this._page._callOn(this, fn, arg, true) as Promise<R>;
  }
  async evaluateHandle<A = unknown>(fn: string | ((self: any, arg: A) => unknown), arg?: A): Promise<JSHandle> {
    return this._page._callOn(this, fn, arg, false) as Promise<JSHandle>;
  }
  async jsonValue(): Promise<any> {
    if (!this._objectId) return valueOf(this._remote);
    return this.evaluate((v: unknown) => v);
  }
  async getProperty(name: string): Promise<JSHandle> {
    return this.evaluateHandle((o: any, n: string) => o[n], name);
  }
  async getProperties(): Promise<Map<string, JSHandle>> {
    const map = new Map<string, JSHandle>();
    if (!this._objectId) return map;
    const { result } = await this._page._send('Runtime.getProperties', { objectId: this._objectId, ownProperties: true });
    for (const p of result as { name: string; enumerable?: boolean; value?: Remote }[]) {
      if (p.value && p.enumerable !== false) map.set(p.name, this._page._handle(p.value));
    }
    return map;
  }
  asElement(): ElementHandle | null {
    return null;
  }
  async dispose(): Promise<void> {
    if (this._disposed || !this._objectId) return;
    this._disposed = true;
    await this._page._send('Runtime.releaseObject', { objectId: this._objectId }).catch(() => {});
  }
  async [Symbol.asyncDispose](): Promise<void> {
    await this.dispose();
  }
  toString(): string {
    return this._objectId ? `JSHandle@${this._remote.subtype ?? this._remote.type}` : `JSHandle@${String(valueOf(this._remote))}`;
  }
}

let marks = 0;

export class ElementHandle extends JSHandle {
  override asElement(): ElementHandle {
    return this;
  }
  override toString(): string {
    return `JSHandle@${this._remote.description ?? 'node'}`;
  }

  /** Runs `fn` with a locator for this element (it carries a marker attribute meanwhile). */
  private async marked<T>(fn: (loc: Locator) => Promise<T>): Promise<T> {
    const id = `h${++marks}`;
    const connected = await this.evaluate((el: Element, mark: string) => {
      if (!el.isConnected || el.nodeType !== 1) return false;
      el.setAttribute('data-lp-handle', mark);
      return true;
    }, id);
    if (!connected) throw new Error('elementHandle: Element is not attached to the DOM');
    try {
      return await fn(this._page.locator(`[data-lp-handle="${id}"]`));
    } finally {
      await this.evaluate((el: Element) => el.removeAttribute('data-lp-handle')).catch(() => {});
    }
  }

  // ---- actions (native, on a locator for this element) ----

  click(o?: Parameters<Locator['click']>[0]) { return this.marked((l) => l.click(o)); }
  dblclick(o?: Parameters<Locator['dblclick']>[0]) { return this.marked((l) => l.dblclick(o)); }
  tap(o?: Parameters<Locator['tap']>[0]) { return this.marked((l) => l.tap(o)); }
  hover(o?: Parameters<Locator['hover']>[0]) { return this.marked((l) => l.hover(o)); }
  fill(value: string, o?: Timeout) { return this.marked((l) => l.fill(value, o)); }
  press(key: string, o?: Timeout) { return this.marked((l) => l.press(key, o)); }
  type(text: string, o?: Timeout) { return this.marked((l) => l.type(text, o)); }
  pressSequentially(text: string, o?: Timeout) { return this.marked((l) => l.pressSequentially(text, o)); }
  check(o?: Timeout) { return this.marked((l) => l.check(o)); }
  uncheck(o?: Timeout) { return this.marked((l) => l.uncheck(o)); }
  setChecked(checked: boolean, o?: Timeout) { return this.marked((l) => l.setChecked(checked, o)); }
  focus() { return this.marked((l) => l.focus()); }
  selectOption(values: Parameters<Locator['selectOption']>[0], o?: Timeout) { return this.marked((l) => l.selectOption(values, o)); }
  selectText(o?: Timeout) { return this.marked((l) => l.selectText(o)); }
  setInputFiles(files: Parameters<Locator['setInputFiles']>[0], o?: Timeout) { return this.marked((l) => l.setInputFiles(files, o)); }
  dispatchEvent(type: string, init?: Record<string, unknown>) { return this.marked((l) => l.dispatchEvent(type, init)); }
  scrollIntoViewIfNeeded() { return this.evaluate((el: Element) => el.scrollIntoView({ block: 'nearest', inline: 'nearest' })); }
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
    return this.evaluate((el: Element, n: string) => el.getAttribute(n), name);
  }
  textContent(): Promise<string | null> {
    return this.evaluate((el: Node) => el.textContent);
  }
  innerText(): Promise<string> {
    return this.evaluate((el: HTMLElement) => el.innerText ?? el.textContent ?? '');
  }
  innerHTML(): Promise<string> {
    return this.evaluate((el: Element) => el.innerHTML);
  }
  async boundingBox(): Promise<{ x: number; y: number; width: number; height: number } | null> {
    return this.evaluate((el: Element) => {
      const r = el.getBoundingClientRect();
      return r.width === 0 && r.height === 0 ? null : { x: r.x, y: r.y, width: r.width, height: r.height };
    });
  }
  ownerFrame() {
    return Promise.resolve(this._page.mainFrame());
  }
  contentFrame() {
    return Promise.resolve(null);
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

  async $(selector: string): Promise<ElementHandle | null> {
    const h = await this.evaluateHandle((el: Element, s: string) => el.querySelector(s), selector);
    return h.asElement();
  }
  async $$(selector: string): Promise<ElementHandle[]> {
    const list = await this.evaluateHandle((el: Element, s: string) => Array.from(el.querySelectorAll(s)), selector);
    const props = await list.getProperties();
    await list.dispose();
    return [...props.values()].map((h) => h.asElement()).filter((h): h is ElementHandle => !!h);
  }
  async $eval<R, A>(selector: string, fn: (el: any, arg: A) => R | Promise<R>, arg?: A): Promise<R> {
    const h = await this.$(selector);
    if (!h) throw new Error(`elementHandle.$eval: Failed to find element matching selector "${selector}"`);
    try {
      return await h.evaluate(fn, arg);
    } finally {
      await h.dispose();
    }
  }
  async $$eval<R, A>(selector: string, fn: (els: any[], arg: A) => R | Promise<R>, arg?: A): Promise<R> {
    const list = await this.evaluateHandle((el: Element, s: string) => Array.from(el.querySelectorAll(s)), selector);
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
