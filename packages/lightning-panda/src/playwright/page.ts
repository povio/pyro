// Playwright's Page, Frame, Locator and FrameLocator on lightningpanda.
//
// Every awaited call is one `LP.run` command: locating, the actionability
// checks, waiting and settling happen in the browser, natively. The class
// names are Playwright's on purpose: `expect` from `@playwright/test` checks
// `receiver.constructor.name` and then calls `locator._expect(...)` (or
// `page.mainFrame()._expect(...)`), which these classes answer, so
// `expect(locator).toBeVisible()` works whichever `expect` the test imports.
import { EventEmitter } from 'node:events';
import { readFileSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';
import { Cdp, CdpError } from '../cdp.js';
import type { LocatorSpec, Step } from '../steps.js';
import {
  type ExpectedText,
  type UrlMatch,
  optsString,
  parseSelector,
  pattern,
  quote,
  resolveUrl,
  textMatches,
  textsContain,
  textsMatch,
  urlMatches,
} from './match.js';
import type { BrowserContext } from './browser.js';
import { Coverage } from './coverage.js';
import type { Clock } from './clock.js';
import { renderAria, type AriaNode } from './aria.js';
import { callDeclaration, Download, ElementHandle, hasHandles, JSHandle, serializeArg, valueOf, type PageElement, type PageFunction, type PageFunctionOn, type Sender, type SmartHandle } from './handles.js';
import { APIRequestContext } from './request.js';

const SENT = new WeakMap<Cdp, Map<string, string>>();

export class TimeoutError extends Error {
  override name = 'TimeoutError';
}

const notSupported = (what: string) => new Error(`lightning-panda: ${what} is not supported yet`);

type TextMatch = string | RegExp;
type Timeout = { timeout?: number };
type RunReply = { reads: Record<string, any>; results: any[]; requests: LogEntry[]; ms: number };
type LogEntry = { method: string; url: string; status: number | null; error: string | null; ms: number | null };

/** A CdpError from LP.run as Playwright words it (`TimeoutError` for a step that timed out). */
function runError(api: string, e: unknown, describe?: string): unknown {
  if (!(e instanceof CdpError)) return e;
  // `LP.run: step 0 (Click {…}): timed out after 200 ms: Click {…}: {…} matched no element`
  const head = /^LP\.run: step \d+(?: \((.*?)\))?: /.exec(e.message);
  const step = head?.[1];
  const message = head ? e.message.slice(head[0].length) : e.message;
  const timedOut = /^timed out after (\d+) ms: ([\s\S]*)$/.exec(message);
  if (timedOut) {
    const reason = step && timedOut[2].startsWith(`${step}: `) ? timedOut[2].slice(step.length + 2) : timedOut[2];
    const log = [describe && `waiting for ${describe}`, reason].filter(Boolean).map((l) => `  - ${l}`);
    return new TimeoutError(`${api}: Timeout ${timedOut[1]}ms exceeded.\nCall log:\n${log.join('\n')}`);
  }
  return new Error(`${api}: ${message}`);
}

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Playwright's retry intervals for client-side polling. */
const POLL = [0, 50, 100, 250, 500, 1000];

// ---- network ----

export class Request {
  _response: Response | null = null;
  _failure: string | null = null;
  _finished: Promise<void>;
  _finish!: () => void;
  _redirectedFrom: Request | null = null;
  _redirectedTo: Request | null = null;

  constructor(
    private _page: Page,
    readonly _id: string,
    private _data: { url: string; method: string; headers: Record<string, string>; postData?: string },
    private _type: string,
  ) {
    this._finished = new Promise((r) => (this._finish = r));
  }
  url(): string {
    return this._data.url;
  }
  method(): string {
    return this._data.method;
  }
  headers(): Record<string, string> {
    return lowerKeys(this._data.headers);
  }
  async allHeaders(): Promise<Record<string, string>> {
    return this.headers();
  }
  async headerValue(name: string): Promise<string | null> {
    return this.headers()[name.toLowerCase()] ?? null;
  }
  postData(): string | null {
    return this._data.postData ?? null;
  }
  postDataBuffer(): Buffer | null {
    return this._data.postData === undefined ? null : Buffer.from(this._data.postData);
  }
  postDataJSON(): any {
    const d = this._data.postData;
    if (d === undefined) return null;
    try {
      return JSON.parse(d);
    } catch {
      return Object.fromEntries(new URLSearchParams(d));
    }
  }
  resourceType(): string {
    return this._type.toLowerCase();
  }
  isNavigationRequest(): boolean {
    return this._type === 'Document';
  }
  async response(): Promise<Response | null> {
    await this._finished;
    return this._response;
  }
  failure(): { errorText: string } | null {
    return this._failure === null ? null : { errorText: this._failure };
  }
  frame(): Frame {
    return this._page.mainFrame();
  }
  redirectedFrom(): Request | null {
    return this._redirectedFrom;
  }
  redirectedTo(): Request | null {
    return this._redirectedTo;
  }
  timing() {
    return { startTime: -1, domainLookupStart: -1, domainLookupEnd: -1, connectStart: -1, secureConnectionStart: -1, connectEnd: -1, requestStart: -1, responseStart: -1, responseEnd: -1 };
  }
}

export class Response {
  constructor(
    private _page: Page,
    private _request: Request,
    private _data: { url: string; status: number; statusText?: string; headers?: Record<string, string> },
  ) {}
  url(): string {
    return this._data.url;
  }
  status(): number {
    return this._data.status;
  }
  statusText(): string {
    return this._data.statusText ?? '';
  }
  ok(): boolean {
    return this._data.status === 0 || (this._data.status >= 200 && this._data.status <= 299);
  }
  headers(): Record<string, string> {
    return lowerKeys(this._data.headers ?? {});
  }
  async allHeaders(): Promise<Record<string, string>> {
    return this.headers();
  }
  async headerValue(name: string): Promise<string | null> {
    return this.headers()[name.toLowerCase()] ?? null;
  }
  request(): Request {
    return this._request;
  }
  frame(): Frame {
    return this._page.mainFrame();
  }
  async finished(): Promise<null> {
    await this._request._finished;
    return null;
  }
  async body(): Promise<Buffer> {
    if (!this._request._id) throw notSupported('reading this response body (enable it with page.on("response") before navigating)');
    await this._request._finished;
    const r = await this._page._send('Network.getResponseBody', { requestId: this._request._id });
    return Buffer.from(r.body, r.base64Encoded ? 'base64' : 'utf8');
  }
  async text(): Promise<string> {
    return (await this.body()).toString('utf8');
  }
  async json(): Promise<any> {
    return JSON.parse(await this.text());
  }
}

const lowerKeys = (h: Record<string, string>) => Object.fromEntries(Object.entries(h).map(([k, v]) => [k.toLowerCase(), v]));

export class Dialog {
  private handled = false;
  constructor(
    private _page: Page,
    private _type: string,
    private _message: string,
    private _default: string,
  ) {}
  type(): string {
    return this._type;
  }
  message(): string {
    return this._message;
  }
  defaultValue(): string {
    return this._default;
  }
  page(): Page {
    return this._page;
  }
  async accept(promptText?: string): Promise<void> {
    if (this.handled) throw new Error('Cannot accept dialog which is already handled!');
    this.handled = true;
    await this.answer({ accept: true, ...(promptText === undefined ? {} : { promptText }) });
  }
  async dismiss(): Promise<void> {
    if (this.handled) throw new Error('Cannot dismiss dialog which is already handled!');
    this.handled = true;
    await this.answer({ accept: false });
  }
  private async answer(params: object): Promise<void> {
    try {
      await this._page._send('Page.handleJavaScriptDialog', params);
    } catch (e) {
      // A dialog an action (one LP.run) opened does not block: it was already dismissed.
      if (!(e instanceof CdpError) || !/No dialog is showing/.test(e.message)) throw e;
      if (!Dialog.warned) {
        Dialog.warned = true;
        console.warn('lightning-panda: a dialog opened by an action or locator call was dismissed before the listener answered it; only dialogs from page.evaluate() can be accepted');
      }
    }
  }
  static warned = false;
}

export class ConsoleMessage {
  constructor(
    private _page: Page,
    private _type: string,
    private _text: string,
    private _location: { url: string; lineNumber: number; columnNumber: number },
  ) {}
  type(): string {
    return this._type === 'warning' ? 'warning' : this._type;
  }
  text(): string {
    return this._text;
  }
  args(): never[] {
    return [];
  }
  location() {
    return this._location;
  }
  page(): Page {
    return this._page;
  }
}

// ---- locators ----

type FilterOptions = { has?: Locator; hasNot?: Locator; hasText?: TextMatch; hasNotText?: TextMatch; visible?: boolean };
type ClickOptions = Timeout & {
  button?: 'left' | 'right' | 'middle';
  clickCount?: number;
  delay?: number;
  force?: boolean;
  modifiers?: string[];
  position?: { x: number; y: number };
  trial?: boolean;
};
/** The ARIA roles `getByRole` takes (Playwright's list). */
export type AriaRole = 'alert'|'alertdialog'|'application'|'article'|'banner'|'blockquote'|'button'|'caption'|'cell'|'checkbox'|'code'|'columnheader'|'combobox'|'complementary'|'contentinfo'|'definition'|'deletion'|'dialog'|'directory'|'document'|'emphasis'|'feed'|'figure'|'form'|'generic'|'grid'|'gridcell'|'group'|'heading'|'img'|'insertion'|'link'|'list'|'listbox'|'listitem'|'log'|'main'|'marquee'|'math'|'meter'|'menu'|'menubar'|'menuitem'|'menuitemcheckbox'|'menuitemradio'|'navigation'|'none'|'note'|'option'|'paragraph'|'presentation'|'progressbar'|'radio'|'radiogroup'|'region'|'row'|'rowgroup'|'rowheader'|'scrollbar'|'search'|'searchbox'|'separator'|'slider'|'spinbutton'|'status'|'strong'|'subscript'|'superscript'|'switch'|'tab'|'table'|'tablist'|'tabpanel'|'term'|'textbox'|'time'|'timer'|'toolbar'|'tooltip'|'tree'|'treegrid'|'treeitem';
type RoleOptions = {
  name?: TextMatch;
  exact?: boolean;
  /** `aria-checked` / a checkbox's state; `'mixed'` for indeterminate. */
  checked?: boolean | 'mixed';
  pressed?: boolean | 'mixed';
  disabled?: boolean;
  expanded?: boolean;
  selected?: boolean;
  level?: number;
  includeHidden?: boolean;
};
type FilePayload = { name: string; mimeType: string; buffer: Buffer };

/** What `getBy*` and `locator()` build on: a page, frame, locator or frame locator. */
abstract class Scope {
  abstract _page: Page;
  /** The spec a new locator nests in (`within`) or is framed by. */
  protected abstract _child(spec: LocatorSpec, desc: string): Locator;

  locator(selectorOrLocator: string | Locator, options: FilterOptions = {}): Locator {
    const loc =
      typeof selectorOrLocator === 'string'
        ? this._child(parseSelector(selectorOrLocator, this._page._testIdAttribute()), `locator(${quote(selectorOrLocator)})`)
        : this._child(selectorOrLocator._spec, selectorOrLocator._desc);
    return Object.keys(options).length ? loc.filter(options) : loc;
  }
  getByRole(role: AriaRole, options: RoleOptions = {}): Locator {
    const { name, exact, checked, pressed, disabled, expanded, selected, level, includeHidden, ...rest } = options;
    const extra = Object.keys(rest).filter((k) => (rest as Record<string, unknown>)[k] !== undefined);
    if (extra.length) throw notSupported(`getByRole option ${extra.join(', ')}`);
    const states = clean({ checked, pressed, disabled, expanded, selected, level, includeHidden }) as Partial<LocatorSpec>;
    // `false` is a state to match, not an absent option.
    for (const [k, v] of Object.entries({ checked, pressed, disabled, expanded, selected })) if (v === false) (states as any)[k] = false;
    const spec: LocatorSpec = { role, ...(name === undefined ? {} : { name: pattern(name) }), ...(exact ? { exact: true } : {}), ...states };
    return this._child(spec, `getByRole(${quote(role)}${optsString({ name, exact, checked, pressed, disabled, expanded, selected, level, includeHidden })})`);
  }
  getByText(text: TextMatch, options: { exact?: boolean } = {}): Locator {
    return this._child({ text: pattern(text), ...(options.exact ? { exact: true } : {}) }, `getByText(${quote(text)}${optsString(options)})`);
  }
  getByLabel(text: TextMatch, options: { exact?: boolean } = {}): Locator {
    return this._child({ label: pattern(text), ...(options.exact ? { exact: true } : {}) }, `getByLabel(${quote(text)}${optsString(options)})`);
  }
  getByPlaceholder(text: TextMatch, options: { exact?: boolean } = {}): Locator {
    return this._child({ placeholder: pattern(text), ...(options.exact ? { exact: true } : {}) }, `getByPlaceholder(${quote(text)}${optsString(options)})`);
  }
  getByTestId(testId: TextMatch): Locator {
    if (testId instanceof RegExp) throw notSupported('getByTestId with a RegExp');
    const attribute = this._page._testIdAttribute();
    return this._child({ testId, ...(attribute === 'data-testid' ? {} : { testIdAttribute: attribute }) }, `getByTestId(${quote(testId)})`);
  }
  getByAltText(text: TextMatch, options: { exact?: boolean } = {}): Locator {
    return this._child(attrSpec('alt', text, options), `getByAltText(${quote(text)}${optsString(options)})`);
  }
  getByTitle(text: TextMatch, options: { exact?: boolean } = {}): Locator {
    return this._child(attrSpec('title', text, options), `getByTitle(${quote(text)}${optsString(options)})`);
  }
  frameLocator(selector: string): FrameLocator {
    return new FrameLocator(this._page, this.locator(selector));
  }
}

function attrSpec(attr: string, text: TextMatch, options: { exact?: boolean }): LocatorSpec {
  if (text instanceof RegExp) throw notSupported(`a RegExp for the ${attr} attribute`);
  const v = JSON.stringify(text);
  return { css: options.exact ? `[${attr}=${v}]` : `[${attr}*=${v} i]` };
}

export class Locator extends Scope {
  constructor(
    readonly _page: Page,
    readonly _spec: LocatorSpec,
    readonly _desc: string,
  ) {
    super();
  }

  protected _child(spec: LocatorSpec, desc: string): Locator {
    const { frame, ...within } = this._spec;
    return new Locator(this._page, { ...spec, within, ...(frame ? { frame } : {}) }, `${this._desc}.${desc}`);
  }

  page(): Page {
    return this._page;
  }
  toString(): string {
    return this._desc;
  }

  // ---- narrowing ----

  /** The spec to narrow further: wrapped (`of`) once it has picked an `nth`, so the new step applies after it. */
  private base(): LocatorSpec {
    if (this._spec.nth === undefined) return this._spec;
    const { frame, ...picked } = this._spec;
    return { of: picked, ...(frame ? { frame } : {}) };
  }
  filter(options: FilterOptions = {}): Locator {
    const base = this.base();
    const filter = { ...base.filter };
    if (options.hasText !== undefined) filter.hasText = pattern(options.hasText);
    if (options.hasNotText !== undefined) filter.hasNotText = pattern(options.hasNotText);
    if (options.has) filter.has = innerSpec(options.has);
    if (options.hasNot) filter.hasNot = innerSpec(options.hasNot);
    if (options.visible !== undefined) filter.visible = options.visible;
    return new Locator(this._page, { ...base, filter }, `${this._desc}.filter(${filterDesc(options)})`);
  }
  nth(index: number): Locator {
    return new Locator(this._page, { ...this.base(), nth: index }, `${this._desc}.nth(${index})`);
  }
  first(): Locator {
    return this.nth(0).withDesc(`${this._desc}.first()`);
  }
  last(): Locator {
    return this.nth(-1).withDesc(`${this._desc}.last()`);
  }
  private withDesc(desc: string): Locator {
    return new Locator(this._page, this._spec, desc);
  }
  /** Matches of both (Playwright's `and`). */
  and(locator: Locator): Locator {
    return this.join('and', locator);
  }
  /** Matches of either, in document order (Playwright's `or`). */
  or(locator: Locator): Locator {
    return this.join('or', locator);
  }
  private join(kind: 'and' | 'or', other: Locator): Locator {
    const { frame, ...mine } = this._spec;
    const { frame: otherFrame, ...theirs } = other._spec;
    if (JSON.stringify(frame) !== JSON.stringify(otherFrame)) throw new Error(`locator.${kind}: locators must belong to the same frame`);
    const spec: LocatorSpec = { of: mine, [kind]: [theirs], ...(frame ? { frame } : {}) };
    return new Locator(this._page, spec, `${this._desc}.${kind}(${other._desc})`);
  }
  contentFrame(): FrameLocator {
    return new FrameLocator(this._page, this);
  }
  async all(): Promise<Locator[]> {
    const n = await this.count();
    return Array.from({ length: n }, (_, i) => this.nth(i));
  }
  /** The element (waiting for exactly one), as a handle. */
  async elementHandle(options: Timeout = {}): Promise<ElementHandle<PageElement>> {
    const h = await this._page._handleFrom('locator.elementHandle', '(el) => el', { locator: this._spec, timeout: options.timeout }, this._desc);
    return h.asElement() as ElementHandle<PageElement>;
  }
  async elementHandles(): Promise<ElementHandle<PageElement>[]> {
    const list = await this._page._handleFrom('locator.elementHandles', '(els) => els', { locator: this._spec, all: true }, this._desc);
    const props = await list.getProperties();
    await list.dispose();
    return [...props.values()].map((h) => h.asElement()).filter((h): h is ElementHandle<PageElement> => !!h);
  }

  // ---- actions ----

  private act(api: string, action: string, timeout: number | undefined, extra: Record<string, unknown> = {}): Promise<void> {
    const step = { act: { locator: this._spec, action, ...extra, timeout: this._page._timeout(timeout) } };
    return this._page._run(`locator.${api}`, [step], this._desc).then(() => undefined);
  }
  private pointer(o: ClickOptions): Record<string, unknown> {
    const modifiers = [...new Set([...(o.modifiers ?? []), ...this._page.keyboard._held()])];
    return {
      ...(modifiers.length ? { modifiers } : {}),
      ...(o.button && o.button !== 'left' ? { button: o.button } : {}),
      ...(o.position ? { position: o.position } : {}),
      ...(o.force ? { force: true } : {}),
    };
  }
  async click(options: ClickOptions = {}): Promise<void> {
    if (options.trial) return this.trial(options.timeout);
    if ((options.clickCount ?? 1) > 2) throw notSupported('clickCount above 2');
    return this.act('click', options.clickCount === 2 ? 'dblclick' : 'click', options.timeout, this.pointer(options));
  }
  async dblclick(options: ClickOptions = {}): Promise<void> {
    if (options.trial) return this.trial(options.timeout);
    return this.act('dblclick', 'dblclick', options.timeout, this.pointer(options));
  }
  async tap(options: ClickOptions = {}): Promise<void> {
    return this.click(options);
  }
  async hover(options: ClickOptions = {}): Promise<void> {
    if (options.trial) return this.trial(options.timeout);
    return this.act('hover', 'hover', options.timeout, this.pointer(options));
  }
  /** `trial`: the actionability checks only. */
  private async trial(timeout?: number): Promise<void> {
    const t = this._page._timeout(timeout);
    const checks = [
      { check: { locator: this._spec, assert: 'visible', timeout: t } },
      { check: { locator: this._spec, assert: 'enabled', expected: true, timeout: t } },
    ];
    await this._page._run('locator.click', checks, this._desc);
  }
  async fill(value: string, options: Timeout & { force?: boolean } = {}): Promise<void> {
    return this.act('fill', 'fill', options.timeout, { value: String(value) });
  }
  async clear(options: Timeout = {}): Promise<void> {
    return this.act('clear', 'fill', options.timeout, { value: '' });
  }
  async press(key: string, options: Timeout & { delay?: number } = {}): Promise<void> {
    return this.act('press', 'press', options.timeout, { value: key });
  }
  async pressSequentially(text: string, options: Timeout & { delay?: number } = {}): Promise<void> {
    return this.act('pressSequentially', 'type', options.timeout, { value: text });
  }
  async check(options: Timeout & { force?: boolean; trial?: boolean } = {}): Promise<void> {
    if (options.trial) return this.trial(options.timeout);
    return this.act('check', 'check', options.timeout);
  }
  async uncheck(options: Timeout & { force?: boolean; trial?: boolean } = {}): Promise<void> {
    if (options.trial) return this.trial(options.timeout);
    return this.act('uncheck', 'uncheck', options.timeout);
  }
  async setChecked(checked: boolean, options: Timeout & { force?: boolean; trial?: boolean } = {}): Promise<void> {
    return checked ? this.check(options) : this.uncheck(options);
  }
  async focus(options: Timeout = {}): Promise<void> {
    return this.act('focus', 'focus', options.timeout, { settle: false });
  }
  async blur(options: Timeout = {}): Promise<void> {
    return this.act('blur', 'blur', options.timeout, { settle: false });
  }
  async dragTo(target: Locator, options: Timeout = {}): Promise<void> {
    return this.act('dragTo', 'drag', options.timeout, { to: target._spec });
  }
  async setInputFiles(files: string | string[] | FilePayload | FilePayload[], options: Timeout = {}): Promise<void> {
    const list = (Array.isArray(files) ? files : [files]) as (string | FilePayload)[];
    const wire = list.map((f) =>
      typeof f === 'string'
        ? { name: basename(f), base64: readFileSync(f).toString('base64') }
        : { name: f.name, mimeType: f.mimeType, base64: Buffer.from(f.buffer).toString('base64') },
    );
    return this.act('setInputFiles', 'upload', options.timeout, { files: wire });
  }
  async selectOption(
    values: string | { value?: string; label?: string; index?: number } | (string | { value?: string; label?: string; index?: number })[] | null,
    options: Timeout = {},
  ): Promise<string[]> {
    const list = values === null ? [] : Array.isArray(values) ? values : [values];
    const wanted = list.map((v) => (typeof v === 'string' ? { valueOrLabel: v } : v));
    return this._page._call('locator.selectOption', SELECT_OPTION, { locator: this._spec, arg: wanted, timeout: options.timeout }, this._desc);
  }
  async selectText(options: Timeout & { force?: boolean } = {}): Promise<void> {
    await this.evaluate((el: any) => (el.select ? el.select() : getSelection()?.selectAllChildren(el)), undefined, options);
  }
  async scrollIntoViewIfNeeded(options: Timeout = {}): Promise<void> {
    await this.evaluate((el: Element) => el.scrollIntoView({ block: 'nearest', inline: 'nearest' }), undefined, options);
  }
  async dispatchEvent(type: string, eventInit: Record<string, unknown> = {}, options: Timeout = {}): Promise<void> {
    await this._page._call('locator.dispatchEvent', DISPATCH_EVENT, { locator: this._spec, arg: { type, eventInit }, timeout: options.timeout }, this._desc);
  }
  async highlight(): Promise<void> {}

  // ---- reads ----

  private async read(api: string, read: string, timeout?: number, property?: string): Promise<any> {
    const step = { read: { locator: this._spec, read, as: 'v', ...(property ? { property } : {}), timeout: this._page._timeout(timeout) } };
    const r = await this._page._run(`locator.${api}`, [step], this._desc);
    return r.reads.v;
  }
  async count(): Promise<number> {
    return this.read('count', 'count', 0);
  }
  /**
   * The element's ARIA tree as Playwright's YAML; `mode: 'ai'` adds refs
   * (`[ref=e5]`, for `locator('aria-ref=e5')`), `[active]`,
   * `[cursor=pointer]` and the snapshots of `<iframe>`s.
   */
  async ariaSnapshot(options: Timeout & { depth?: number; mode?: 'ai' | 'default' } = {}): Promise<string> {
    if (options.mode === 'ai') {
      // As Playwright: the AI mode does not wait for the locator, it fails at once.
      if ((await this.count()) === 0) throw new Error(`locator.ariaSnapshot: Selector "${this._desc}" does not match any element`);
      return this._page._aiSnapshot('locator.ariaSnapshot', this._spec, options.depth, options.timeout, this._desc);
    }
    return renderAria(await this._ariaTree(options.timeout), options.depth);
  }
  /** The engine's ARIA tree of the element (waits for it, as reads do). */
  async _ariaTree(timeout?: number): Promise<AriaNode> {
    const tree = await this.read('ariaSnapshot', 'aria', timeout);
    if (tree?.error) throw new Error(`locator.ariaSnapshot: ${tree.error}`);
    return tree as AriaNode;
  }
  async textContent(options: Timeout = {}): Promise<string | null> {
    return this.evaluate((el: Node) => el.textContent, undefined, options);
  }
  async innerText(options: Timeout = {}): Promise<string> {
    return this.evaluate((el: HTMLElement) => el.innerText ?? el.textContent ?? '', undefined, options);
  }
  async innerHTML(options: Timeout = {}): Promise<string> {
    return this.evaluate((el: Element) => el.innerHTML, undefined, options);
  }
  async inputValue(options: Timeout = {}): Promise<string> {
    return this.read('inputValue', 'value', options.timeout);
  }
  async getAttribute(name: string, options: Timeout = {}): Promise<string | null> {
    return this.evaluate((el: Element, n: string) => el.getAttribute(n), name, options);
  }
  async allTextContents(): Promise<string[]> {
    return this.evaluateAll((els: Node[]) => els.map((e) => e.textContent ?? ''));
  }
  async allInnerTexts(): Promise<string[]> {
    return this.evaluateAll((els: HTMLElement[]) => els.map((e) => e.innerText ?? e.textContent ?? ''));
  }
  /** Like Playwright: no waiting; `false` when nothing matches. */
  async isVisible(): Promise<boolean> {
    return this._evaluateAll<boolean | 'strict'>(IS_VISIBLE).then((v) => {
      if (v === 'strict') throw new Error(`locator.isVisible: strict mode violation: ${this._desc} resolved to several elements`);
      return v as boolean;
    });
  }
  async isHidden(): Promise<boolean> {
    return !(await this.isVisible());
  }
  async isEnabled(options: Timeout = {}): Promise<boolean> {
    return this.read('isEnabled', 'enabled', options.timeout);
  }
  async isDisabled(options: Timeout = {}): Promise<boolean> {
    return !(await this.isEnabled(options));
  }
  async isChecked(options: Timeout = {}): Promise<boolean> {
    const v = await this._page._call<boolean | 'mixed' | null>('locator.isChecked', CHECKED_STATE, { locator: this._spec, timeout: options.timeout }, this._desc);
    if (v === null) throw new Error(`locator.isChecked: Error: Not a checkbox or radio button`);
    return v === true;
  }
  async isEditable(options: Timeout = {}): Promise<boolean> {
    return this._page._call('locator.isEditable', IS_EDITABLE, { locator: this._spec, timeout: options.timeout }, this._desc);
  }
  async boundingBox(options: Timeout = {}): Promise<{ x: number; y: number; width: number; height: number } | null> {
    return this.read('boundingBox', 'box', options.timeout);
  }
  evaluate<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, Arg, R>, arg: Arg, options?: Timeout): Promise<R>;
  evaluate<R, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, void, R>, options?: Timeout): Promise<R>;
  async evaluate<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, Arg, R>, arg?: Arg, options: Timeout = {}): Promise<R> {
    return this._page._call('locator.evaluate', source(pageFunction), { locator: this._spec, arg, timeout: options.timeout }, this._desc);
  }
  evaluateAll<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E[], Arg, R>, arg: Arg): Promise<R>;
  evaluateAll<R, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E[], void, R>): Promise<R>;
  async evaluateAll<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E[], Arg, R>, arg?: Arg): Promise<R> {
    return this._page._call('locator.evaluateAll', source(pageFunction), { locator: this._spec, all: true, arg, timeout: 0 }, this._desc);
  }
  /** One of this module's in-page function sources, on all matches. */
  _evaluateAll<R>(fn: string, arg?: unknown): Promise<R> {
    return this._page._call('expect', fn, { locator: this._spec, all: true, arg, timeout: 0 }, this._desc);
  }
  evaluateHandle<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, Arg, R>, arg: Arg, options?: Timeout): Promise<SmartHandle<R>>;
  evaluateHandle<R, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, void, R>, options?: Timeout): Promise<SmartHandle<R>>;
  async evaluateHandle<R, Arg, E extends PageElement = PageElement>(pageFunction: PageFunctionOn<E, Arg, R>, arg?: Arg, options: Timeout = {}): Promise<SmartHandle<R>> {
    return this._page._handleFrom('locator.evaluateHandle', source(pageFunction), { locator: this._spec, arg, timeout: options.timeout }, this._desc) as Promise<SmartHandle<R>>;
  }
  async waitFor(options: Timeout & { state?: 'attached' | 'detached' | 'visible' | 'hidden' } = {}): Promise<void> {
    const timeout = this._page._timeout(options.timeout);
    const state = options.state ?? 'visible';
    const check =
      state === 'visible' || state === 'hidden'
        ? { locator: this._spec, assert: state, timeout }
        : { locator: { ...this._spec, nth: undefined }, assert: 'count', expected: 0, not: state === 'attached', timeout };
    await this._page._run('locator.waitFor', [{ check: clean(check) }], this._desc);
  }
  async screenshot(options: Timeout & { path?: string; type?: 'png' | 'jpeg'; quality?: number } = {}): Promise<Buffer> {
    const box = await this.boundingBox(options);
    if (!box) throw new Error(`locator.screenshot: ${this._desc} is not visible`);
    return this._page.screenshot({ ...options, clip: box });
  }
  /** What `expect(locator).toX()` from `@playwright/test` calls. */
  async _expect(expression: string, options: ExpectOptions): Promise<ExpectResult> {
    return expectLocator(this, expression, options);
  }
}

const innerSpec = (l: Locator): LocatorSpec => {
  const { frame: _frame, ...spec } = l._spec;
  return spec;
};
const filterDesc = (o: FilterOptions) =>
  `{ ${Object.entries(o)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => `${k}: ${v instanceof Locator ? v._desc : typeof v === 'string' || v instanceof RegExp ? quote(v) : String(v)}`)
    .join(', ')} }`;
const clean = (o: Record<string, unknown>) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined && v !== false));
/** Playwright: a string is an expression, called when it evaluates to a function. */
const source = (fn: string | Function) =>
  typeof fn === 'function' ? fn.toString() : `(el, arg) => { const v = (${fn}\n); return typeof v === 'function' ? v(el, arg) : v; }`;

export class FrameLocator {
  constructor(
    private _page: Page,
    /** The `<iframe>` element. */
    private _owner: Locator,
  ) {}
  private scoped(make: (scope: Scope) => Locator): Locator {
    const page = this._page;
    const frameSpec = this._owner._spec;
    const desc = `${this._owner._desc}.contentFrame()`;
    const scope = new (class extends Scope {
      _page = page;
      protected _child(spec: LocatorSpec, d: string): Locator {
        return new Locator(page, { ...spec, frame: frameSpec }, `${desc}.${d}`);
      }
    })();
    return make(scope);
  }
  locator(selectorOrLocator: string | Locator, options?: FilterOptions): Locator {
    return this.scoped((s) => s.locator(selectorOrLocator, options));
  }
  getByRole(role: AriaRole, options?: RoleOptions): Locator {
    return this.scoped((s) => s.getByRole(role, options));
  }
  getByText(text: TextMatch, options?: { exact?: boolean }): Locator {
    return this.scoped((s) => s.getByText(text, options));
  }
  getByLabel(text: TextMatch, options?: { exact?: boolean }): Locator {
    return this.scoped((s) => s.getByLabel(text, options));
  }
  getByPlaceholder(text: TextMatch, options?: { exact?: boolean }): Locator {
    return this.scoped((s) => s.getByPlaceholder(text, options));
  }
  getByTestId(testId: string): Locator {
    return this.scoped((s) => s.getByTestId(testId));
  }
  getByAltText(text: TextMatch, options?: { exact?: boolean }): Locator {
    return this.scoped((s) => s.getByAltText(text, options));
  }
  getByTitle(text: TextMatch, options?: { exact?: boolean }): Locator {
    return this.scoped((s) => s.getByTitle(text, options));
  }
  frameLocator(selector: string): FrameLocator {
    return new FrameLocator(this._page, this.locator(selector));
  }
  owner(): Locator {
    return this._owner;
  }
}

// ---- in-page helpers (sources sent with eval steps) ----

const SELECT_OPTION = `(el, wanted) => {
  if (!(el instanceof HTMLSelectElement)) throw new Error('Element is not a <select> element');
  const options = [...el.options];
  const picked = wanted.map((w) => {
    const o = options.find((o, i) =>
      w.valueOrLabel !== undefined ? o.value === w.valueOrLabel || o.label === w.valueOrLabel
        : (w.value === undefined || o.value === w.value) && (w.label === undefined || o.label === w.label) && (w.index === undefined || i === w.index));
    if (!o) throw new Error('No option matches ' + JSON.stringify(w));
    return o;
  });
  if (picked.length > 1 && !el.multiple) throw new Error('Element is not a multi-select');
  for (const o of options) o.selected = picked.includes(o);
  el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return picked.map((o) => o.value);
}`;

const DISPATCH_EVENT = `(el, { type, eventInit }) => {
  const init = { bubbles: true, cancelable: true, composed: true, ...eventInit };
  const E = /^(click|dblclick|mouse|contextmenu)/.test(type) ? MouseEvent
    : /^pointer/.test(type) ? (globalThis.PointerEvent ?? MouseEvent)
    : /^key/.test(type) ? KeyboardEvent
    : /^focus|^blur/.test(type) ? FocusEvent
    : /^(input|beforeinput)$/.test(type) ? (globalThis.InputEvent ?? Event)
    : /^drag|^drop/.test(type) ? (globalThis.DragEvent ?? Event)
    : Event;
  el.dispatchEvent(new E(type, init));
}`;

const IS_VISIBLE = `(els) => {
  if (els.length > 1) return 'strict';
  const el = els[0];
  if (!el) return false;
  if (el.checkVisibility) return el.checkVisibility({ visibilityProperty: true });
  const s = getComputedStyle(el);
  const r = el.getBoundingClientRect();
  return s.visibility !== 'hidden' && r.width > 0 && r.height > 0;
}`;

const CHECKED_STATE = `(el) => {
  if (el instanceof HTMLInputElement && (el.type === 'checkbox' || el.type === 'radio')) return el.indeterminate ? 'mixed' : el.checked;
  const role = el.getAttribute('role');
  if (['checkbox','radio','switch','menuitemcheckbox','menuitemradio','option','treeitem'].includes(role)) {
    const a = el.getAttribute('aria-checked');
    return a === 'mixed' ? 'mixed' : a === 'true';
  }
  return null;
}`;

const IS_EDITABLE = `(el) => {
  if (el.disabled || el.closest?.('fieldset[disabled]')) return false;
  if (el.readOnly || el.getAttribute('aria-readonly') === 'true') return false;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || el.isContentEditable || ['textbox','searchbox','combobox','spinbutton'].includes(el.getAttribute('role'));
}`;

// ---- expect ----

export type ExpectOptions = {
  expectedText?: ExpectedText[];
  expectedNumber?: number;
  expectedValue?: any;
  expressionArg?: any;
  useInnerText?: boolean;
  isNot?: boolean;
  timeout?: number;
};
export type ExpectResult = { matches: boolean; received?: any; log?: string[]; timedOut?: boolean; errorMessage?: string };

/** A check step that is exactly this expectation, when lightningpanda has one. */
function nativeCheck(expression: string, o: ExpectOptions): Record<string, unknown> | null {
  const one = o.expectedText?.length === 1 ? o.expectedText[0] : undefined;
  const plain = one && one.string !== undefined && !one.ignoreCase ? one.string : undefined;
  switch (expression) {
    case 'to.be.visible':
      return { assert: 'visible' };
    case 'to.be.hidden':
      return { assert: 'hidden' };
    case 'to.be.enabled':
      return { assert: 'enabled', expected: true };
    case 'to.be.disabled':
      return { assert: 'enabled', expected: false };
    case 'to.be.focused':
      return { assert: 'focused', expected: true };
    case 'to.be.checked': {
      const v = o.expectedValue ?? {};
      return { assert: 'checked', expected: v.indeterminate ? 'mixed' : v.checked === false ? false : true };
    }
    case 'to.have.count':
      return { assert: 'count', expected: o.expectedNumber };
    case 'to.have.text':
      if (plain === undefined || o.useInnerText || !one!.normalizeWhiteSpace) return null;
      return { assert: one!.matchSubstring ? 'containsText' : 'text', expected: plain };
    case 'to.have.value':
      return plain !== undefined && !one!.normalizeWhiteSpace && !one!.matchSubstring ? { assert: 'value', expected: plain } : null;
    case 'to.have.accessible.name':
      return plain !== undefined && !one!.matchSubstring ? { assert: 'name', expected: plain } : null;
    case 'to.have.attribute.value':
      return plain !== undefined && !one!.matchSubstring && !one!.normalizeWhiteSpace
        ? { assert: 'attribute', property: String(o.expressionArg), expected: plain }
        : null;
    default:
      return null;
  }
}

export async function expectLocator(loc: Locator, expression: string, o: ExpectOptions): Promise<ExpectResult> {
  const isNot = !!o.isNot;
  const timeout = o.timeout ?? 5_000;
  const native = nativeCheck(expression, o);
  if (native) {
    const locator = expression === 'to.have.count' ? loc._spec : loc._spec;
    const step = { check: { locator, ...native, ...(isNot ? { not: true } : {}), timeout } };
    try {
      await loc._page._run('expect', [step], loc._desc);
      return { matches: !isNot };
    } catch (e) {
      const err = runError('expect', e) as Error;
      if (!(err instanceof TimeoutError)) return { matches: isNot, errorMessage: err.message };
      const received = await receivedFor(loc, expression, o).catch(() => undefined);
      return { matches: isNot, timedOut: true, received: received?.received, log: [`waiting for ${loc._desc}`] };
    }
  }
  // Client-side retries of a value read in the page.
  const deadline = Date.now() + timeout;
  let last: { matches: boolean; received?: any } = { matches: false };
  for (let i = 0; ; i++) {
    try {
      last = await receivedFor(loc, expression, o);
    } catch (e) {
      return { matches: isNot, errorMessage: String((e as Error).message ?? e) };
    }
    if (last.matches !== isNot) return last;
    const wait = POLL[Math.min(i + 1, POLL.length - 1)];
    if (Date.now() + wait > deadline) break;
    await delay(wait);
  }
  return { ...last, timedOut: true, log: [`waiting for ${loc._desc}`] };
}

/** The value an expectation looks at, read once, and whether it matches. */
async function receivedFor(loc: Locator, expression: string, o: ExpectOptions): Promise<{ matches: boolean; received?: any }> {
  const texts = o.expectedText ?? [];
  const one = texts[0];
  const arg = { expression, name: o.expressionArg, innerText: !!o.useInnerText };
  const v = await loc._evaluateAll<{ count: number; value?: any; strict?: boolean; unknown?: boolean }>(RECEIVED, arg);
  if (v.strict) throw new Error(`strict mode violation: ${loc._desc} resolved to ${v.count} elements`);
  const r = v.value;
  switch (expression) {
    case 'to.be.attached':
      return { matches: v.count > 0, received: v.count > 0 ? 'attached' : 'detached' };
    case 'to.be.detached':
      return { matches: v.count === 0, received: v.count > 0 ? 'attached' : 'detached' };
    case 'to.have.count':
      return { matches: v.count === o.expectedNumber, received: v.count };
    case 'to.have.text.array':
    case 'to.have.class.array':
      return { matches: textsMatch(texts, r ?? []), received: r ?? [] };
    case 'to.contain.text.array':
      return { matches: textsContain(texts, r ?? []), received: r ?? [] };
    case 'to.contain.class.array':
      return { matches: (r ?? []).length === texts.length && texts.every((t, i) => containsClasses(r[i], t.string ?? '')), received: r ?? [] };
    case 'to.contain.class':
      return { matches: v.count === 1 && containsClasses(r, one?.string ?? ''), received: r };
    case 'to.have.values':
      return { matches: v.count === 1 && Array.isArray(r) && textsMatch(texts, r), received: r };
    case 'to.have.property':
      return { matches: v.count === 1 && JSON.stringify(r) === JSON.stringify(o.expectedValue), received: r };
    case 'to.have.attribute':
      return { matches: v.count === 1 && r === true, received: r ? 'attribute present' : 'attribute missing' };
    case 'to.be.visible':
    case 'to.be.hidden': {
      const visible = v.count === 1 && r === true;
      return { matches: expression === 'to.be.visible' ? visible : !visible, received: visible ? 'visible' : 'hidden' };
    }
    case 'to.be.checked': {
      const want = o.expectedValue?.indeterminate ? 'mixed' : o.expectedValue?.checked === false ? false : true;
      return { matches: v.count === 1 && r === want, received: r === 'mixed' ? 'mixed' : r ? 'checked' : 'unchecked' };
    }
    case 'to.be.in.viewport':
      return { matches: v.count === 1 && r > (o.expectedNumber ?? 0) - 1e-9 && r > 0, received: r };
    case 'to.be.enabled':
    case 'to.be.disabled':
    case 'to.be.editable':
    case 'to.be.readonly':
    case 'to.be.empty':
    case 'to.be.focused':
      return { matches: v.count === 1 && r === true, received: r };
    default:
      if (v.unknown) throw notSupported(`expect matcher ${expression}`);
      return { matches: v.count === 1 && !!one && textMatches(one, r), received: r };
  }
}

const containsClasses = (have: unknown, want: string) => {
  if (typeof have !== 'string') return false;
  const list = have.split(/\s+/);
  return want.split(/\s+/).filter(Boolean).every((c) => list.includes(c));
};

/** In the page: what an expectation reads from the matched elements. */
const RECEIVED = `(els, { expression, name, innerText }) => {
  const text = (e) => innerText ? (e.innerText ?? e.textContent ?? '') : (e.textContent ?? '');
  const all = { 'to.have.text.array': text, 'to.contain.text.array': text, 'to.have.class.array': (e) => e.getAttribute('class') ?? '', 'to.contain.class.array': (e) => e.getAttribute('class') ?? '' };
  if (all[expression]) return { count: els.length, value: els.map(all[expression]) };
  if (expression === 'to.be.attached' || expression === 'to.be.detached' || expression === 'to.have.count') return { count: els.length };
  if (els.length > 1) return { count: els.length, strict: true };
  const e = els[0];
  if (!e) return { count: 0 };
  const visible = () => e.checkVisibility ? e.checkVisibility({ visibilityProperty: true }) : e.getBoundingClientRect().width > 0;
  const disabled = () => !!(e.disabled || e.closest?.('fieldset[disabled]') || e.getAttribute('aria-disabled') === 'true');
  const editable = () => !disabled() && !e.readOnly && e.getAttribute('aria-readonly') !== 'true';
  const one = {
    'to.have.text': () => text(e),
    'to.have.value': () => e.value ?? '',
    'to.have.values': () => [...(e.selectedOptions ?? [])].map((o) => o.value),
    'to.have.attribute': () => e.hasAttribute(name),
    'to.have.attribute.value': () => e.getAttribute(name),
    'to.have.class': () => e.getAttribute('class') ?? '',
    'to.contain.class': () => e.getAttribute('class') ?? '',
    'to.have.id': () => e.id,
    'to.have.css': () => getComputedStyle(e).getPropertyValue(name) || getComputedStyle(e)[name] || '',
    'to.have.property': () => e[name],
    'to.have.role': () => e.getAttribute('role') ?? '',
    'to.have.accessible.name': () => e.getAttribute('aria-label') ?? e.textContent ?? '',
    'to.have.accessible.description': () => e.getAttribute('aria-description') ?? e.getAttribute('title') ?? '',
    'to.be.visible': visible,
    'to.be.hidden': visible,
    'to.be.enabled': () => !disabled(),
    'to.be.disabled': () => disabled(),
    'to.be.editable': editable,
    'to.be.readonly': () => !editable(),
    'to.be.focused': () => document.activeElement === e,
    'to.be.empty': () => ('value' in e && (e instanceof HTMLInputElement || e instanceof HTMLTextAreaElement)) ? e.value === '' : !(e.textContent ?? '').trim(),
    'to.be.checked': () => {
      if (e instanceof HTMLInputElement && (e.type === 'checkbox' || e.type === 'radio')) return e.indeterminate ? 'mixed' : e.checked;
      const a = e.getAttribute('aria-checked');
      return a === 'mixed' ? 'mixed' : a === 'true';
    },
    'to.be.in.viewport': () => {
      const r = e.getBoundingClientRect();
      const w = Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0));
      const h = Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
      return r.width * r.height > 0 ? (w * h) / (r.width * r.height) : 0;
    },
  }[expression];
  if (!one) return { count: 1, unknown: true };
  return { count: 1, value: one() };
}`;

// ---- page ----

type GotoOptions = Timeout & { waitUntil?: 'load' | 'domcontentloaded' | 'networkidle' | 'commit'; referer?: string };

/** In the page: `fn` (source) run in the document of the `<iframe>` it gets. */
const IN_FRAME = `(iframe, [src, arg, expression]) => {
  const w = iframe.contentWindow;
  if (!w) throw new Error('the frame has no window (cross-origin or detached)');
  return expression ? w.eval(src) : w.eval('(' + src + '\\n)')(arg);
}`;

/**
 * The main frame (`page.mainFrame()`), or a child frame: its locators search
 * the document of its `<iframe>`, which carries a `data-lp-frame` attribute
 * once the frame is listed (`page.frames()`, `frame.childFrames()`).
 */
export class Frame extends Scope {
  _name = '';
  _url = 'about:blank';
  _detached = false;
  constructor(
    readonly _page: Page,
    /** A child frame's id (none for the main frame). */
    readonly _id?: string,
    public _parent: Frame | null = null,
  ) {
    super();
  }
  /** The `<iframe>` of a child frame, as a locator (in its parent's document). */
  get _owner(): LocatorSpec | undefined {
    if (!this._id) return undefined;
    const own: LocatorSpec = { css: `[data-lp-frame="${this._id}"]` };
    const parent = this._parent?._owner;
    return parent ? { ...own, frame: parent } : own;
  }
  protected _child(spec: LocatorSpec, desc: string): Locator {
    const owner = this._owner;
    return owner ? new Locator(this._page, { ...spec, frame: owner }, `frame(${quote(this._name || this._id!)}).${desc}`) : new Locator(this._page, spec, desc);
  }
  page(): Page {
    return this._page;
  }
  url(): string {
    return this._id ? this._url : this._page.url();
  }
  name(): string {
    return this._name;
  }
  parentFrame(): Frame | null {
    return this._id ? this._parent : null;
  }
  childFrames(): Frame[] {
    return this._page._childFrames(this);
  }
  isDetached(): boolean {
    return this._id ? this._detached : this._page.isClosed();
  }
  async frameElement(): Promise<ElementHandle<HTMLIFrameElement | HTMLFrameElement>> {
    if (!this._owner) throw new Error('frame.frameElement: the main frame has no element');
    const h = await this._page._handleFrom('frame.frameElement', '(el) => el', { locator: this._owner });
    return h.asElement()!;
  }
  async goto(url: string, options: GotoOptions = {}): Promise<Response | null> {
    if (!this._id) return this._page.goto(url, options);
    // A child frame navigates itself; then its document loads.
    const target = resolveUrl(this._page.context()._options.baseURL ?? this._url, url);
    const ms = this._page._timeout(options.timeout);
    const navigated = this._page._frameNavigation(this, ms);
    await this.evaluate(`location.href = ${JSON.stringify(target)}`);
    await navigated;
    await this.waitForLoadState(options.waitUntil === 'domcontentloaded' || options.waitUntil === 'commit' ? 'domcontentloaded' : 'load', { timeout: ms });
    return null;
  }
  evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<R>;
  evaluate<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<R>;
  async evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<R> {
    const fn = pageFunction as string | ((arg: unknown) => unknown);
    if (!this._owner) return this._page.evaluate(pageFunction as PageFunction<unknown, R>, arg as unknown);
    const expression = typeof fn === 'string';
    return this._page._call('frame.evaluate', IN_FRAME, { locator: this._owner, arg: [expression ? fn : fn.toString(), arg ?? null, expression] });
  }
  content() {
    if (!this._id) return this._page.content();
    return this.evaluate(() => (document.doctype ? '<!DOCTYPE ' + document.doctype.name + '>' : '') + document.documentElement.outerHTML);
  }
  title() {
    if (!this._id) return this._page.title();
    return this.evaluate(() => document.title);
  }
  async waitForLoadState(state: 'load' | 'domcontentloaded' | 'networkidle' = 'load', options: Timeout = {}): Promise<void> {
    if (!this._id) return this._page.waitForLoadState(state, options);
    const want = state === 'domcontentloaded' ? ['interactive', 'complete'] : ['complete'];
    const deadline = Date.now() + this._page._timeout(options.timeout);
    for (let i = 0; ; i++) {
      const ready = await this.evaluate(() => document.readyState).catch(() => 'loading');
      if (want.includes(ready as string)) break;
      if (Date.now() > deadline) throw new TimeoutError(`frame.waitForLoadState: Timeout exceeded waiting for "${state}"`);
      await delay(POLL[Math.min(i + 1, POLL.length - 1)]);
    }
    if (state === 'networkidle') await this._page.waitForLoadState('networkidle', options);
  }
  async waitForURL(url: UrlMatch, options: Timeout & { waitUntil?: GotoOptions['waitUntil'] } = {}): Promise<void> {
    if (!this._id) return this._page.waitForURL(url, options);
    const ms = this._page._timeout(options.timeout);
    const deadline = Date.now() + ms;
    const base = this._page.context()._options.baseURL;
    while (!(await urlMatches(base, this._url, url))) {
      const left = deadline - Date.now();
      if (left <= 0) throw new TimeoutError(`frame.waitForURL: Timeout ${ms}ms exceeded.\n  current URL is "${this._url}"`);
      await this._page._frameNavigation(this, left).catch(() => undefined);
    }
    if (options.waitUntil !== 'commit') {
      await this.waitForLoadState(options.waitUntil === 'domcontentloaded' ? 'domcontentloaded' : 'load', { timeout: Math.max(1, deadline - Date.now()) });
    }
  }
  async waitForSelector(selector: string, options: Timeout & { state?: 'attached' | 'detached' | 'visible' | 'hidden'; strict?: boolean } = {}): Promise<ElementHandle<PageElement> | null> {
    if (!this._id) return this._page.waitForSelector(selector, options);
    const state = options.state ?? 'visible';
    const loc = this.locator(selector);
    await (state === 'visible' ? loc.filter({ visible: true }).first() : loc.first()).waitFor(options);
    if (state === 'hidden' || state === 'detached') return null;
    const h = await this._page._handleFrom('frame.waitForSelector', state === 'visible' ? VISIBLE_FIRST : '(els) => els[0] ?? null', { locator: loc._spec, all: true });
    return h.asElement() as ElementHandle<PageElement> | null;
  }
  async waitForFunction<R, A>(fn: string | ((arg: A) => R | Promise<R>), arg?: A, options: Timeout & { polling?: number | 'raf' } = {}): Promise<{ jsonValue(): Promise<R> }> {
    if (!this._id) return this._page.waitForFunction(fn, arg, options);
    const interval = typeof options.polling === 'number' ? options.polling : 16;
    const body = typeof fn === 'function' ? fn.toString() : `() => (${fn})`;
    const poll = `async (arg) => { const f = (${body}\n); for (;;) { const v = await f(arg); if (v) return v; await new Promise((r) => setTimeout(r, ${interval})); } }`;
    const value = await this._page._call<R>('frame.waitForFunction', IN_FRAME, { locator: this._owner, arg: [poll, arg ?? null, false], timeout: options.timeout });
    return { jsonValue: async () => value };
  }
  waitForTimeout(ms: number) {
    return this._page.waitForTimeout(ms);
  }
  async $(selector: string): Promise<ElementHandle<PageElement> | null> {
    if (!this._id) return this._page.$(selector);
    const h = await this._page._handleFrom('frame.$', '(els) => els[0] ?? null', { locator: this.locator(selector)._spec, all: true });
    return h.asElement() as ElementHandle<PageElement> | null;
  }
  $$(selector: string): Promise<ElementHandle<PageElement>[]> {
    if (!this._id) return this._page.$$(selector);
    return this.locator(selector).elementHandles();
  }
  $eval<R, Arg, E extends PageElement = PageElement>(selector: string, pageFunction: PageFunctionOn<E, Arg, R>, arg?: Arg): Promise<R> {
    return this.locator(selector).first().evaluate(pageFunction as PageFunctionOn<E, unknown, R>, arg as unknown);
  }
  $$eval<R, Arg, E extends PageElement = PageElement>(selector: string, pageFunction: PageFunctionOn<E[], Arg, R>, arg?: Arg): Promise<R> {
    return this.locator(selector).evaluateAll(pageFunction as PageFunctionOn<E[], unknown, R>, arg as unknown);
  }
  evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<SmartHandle<R>>;
  evaluateHandle<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<SmartHandle<R>>;
  evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<SmartHandle<R>> {
    const fn = pageFunction as string | ((arg: unknown) => unknown);
    if (!this._owner) return this._page.evaluateHandle(pageFunction as PageFunction<unknown, R>, arg as unknown);
    const src = typeof fn === 'function' ? fn.toString() : `() => (${fn})`;
    return this._page._handleFrom('frame.evaluateHandle', `(iframe, [src, arg]) => iframe.contentWindow.eval('(' + src + '\\n)')(arg)`, {
      locator: this._owner,
      arg: [src, arg ?? null],
      inFrame: true,
    }) as Promise<SmartHandle<R>>;
  }

  /** `expect(page).toHaveURL()` and `.toHaveTitle()` from `@playwright/test`. */
  async _expect(expression: string, o: ExpectOptions): Promise<ExpectResult> {
    const one = o.expectedText?.[0];
    const isNot = !!o.isNot;
    const timeout = o.timeout ?? 5_000;
    const read = async () => (expression === 'to.have.url' ? this._page.url() : await this._page.title());
    if (expression !== 'to.have.url' && expression !== 'to.have.title') throw notSupported(`expect matcher ${expression} on a page`);
    const deadline = Date.now() + timeout;
    let received = await read();
    for (let i = 0; ; i++) {
      if (!!one && textMatches(one, received) !== isNot) return { matches: !isNot, received };
      const wait = POLL[Math.min(i + 1, POLL.length - 1)];
      if (Date.now() + wait > deadline) return { matches: isNot, received, timedOut: true };
      if (expression === 'to.have.url') await this._page._nextNavigation(wait);
      else await delay(wait);
      received = await read();
    }
  }
}

/** `keyboard.down/up` key definitions: `key`, `code`, key code, and the text a press types. */
const MODIFIER_BITS: Record<string, number> = { Alt: 1, Control: 2, Meta: 4, Shift: 8 };
function keyDefinition(key: string): { key: string; code: string; keyCode: number; text: string; location: number } {
  const named: Record<string, [string, number, string?]> = {
    Enter: ['Enter', 13, '\r'], Tab: ['Tab', 9, '\t'], Backspace: ['Backspace', 8], Escape: ['Escape', 27], Delete: ['Delete', 46],
    ArrowLeft: ['ArrowLeft', 37], ArrowUp: ['ArrowUp', 38], ArrowRight: ['ArrowRight', 39], ArrowDown: ['ArrowDown', 40],
    Home: ['Home', 36], End: ['End', 35], PageUp: ['PageUp', 33], PageDown: ['PageDown', 34], Insert: ['Insert', 45],
    Shift: ['ShiftLeft', 16], Control: ['ControlLeft', 17], Alt: ['AltLeft', 18], Meta: ['MetaLeft', 91], ' ': ['Space', 32, ' '],
  };
  for (let i = 1; i <= 12; i++) named[`F${i}`] = [`F${i}`, 111 + i];
  if (named[key]) {
    const [code, keyCode, text] = named[key];
    return { key, code, keyCode, text: text ?? '', location: MODIFIER_BITS[key] ? 1 : 0 };
  }
  if (key.length === 1) {
    const upper = key.toUpperCase();
    const code = /[a-z]/i.test(key) ? `Key${upper}` : /\d/.test(key) ? `Digit${key}` : '';
    return { key, code, keyCode: upper.charCodeAt(0), text: key, location: 0 };
  }
  throw new Error(`Unknown key: "${key}"`);
}

export class Keyboard {
  private held = new Set<string>();
  constructor(private _page: Page) {}
  /** Modifier keys held with `down()`: they apply to clicks and presses. */
  _held(): string[] {
    return [...this.held].filter((k) => k in MODIFIER_BITS);
  }
  private bits(): number {
    return this._held().reduce((b, k) => b | MODIFIER_BITS[k], 0);
  }
  async press(key: string, _options: { delay?: number } = {}): Promise<void> {
    const chord = [...this._held(), key].join('+');
    await this._page._focused('keyboard.press', 'press', chord);
  }
  async type(text: string, _options: { delay?: number } = {}): Promise<void> {
    await this._page._focused('keyboard.type', 'type', text);
  }
  async insertText(text: string): Promise<void> {
    await this._page._focused('keyboard.insertText', 'type', text);
  }
  async down(key: string): Promise<void> {
    const d = keyDefinition(key);
    const repeat = this.held.has(key);
    this.held.add(key);
    await this._page._send('Input.dispatchKeyEvent', {
      type: d.text && !this._held().some((k) => k !== 'Shift') ? 'keyDown' : 'rawKeyDown',
      key: d.key, code: d.code, windowsVirtualKeyCode: d.keyCode, text: d.text, unmodifiedText: d.text,
      location: d.location, autoRepeat: repeat, modifiers: this.bits(),
    });
  }
  async up(key: string): Promise<void> {
    const d = keyDefinition(key);
    this.held.delete(key);
    await this._page._send('Input.dispatchKeyEvent', {
      type: 'keyUp', key: d.key, code: d.code, windowsVirtualKeyCode: d.keyCode, location: d.location, modifiers: this.bits(),
    });
  }
}

export class Mouse {
  constructor(private _page: Page) {}
  private step(action: string, o: Record<string, unknown>) {
    const modifiers = this._page.keyboard._held();
    return this._page
      ._run(`mouse.${action}`, [{ mouse: { action, ...clean(o), ...(modifiers.length ? { modifiers } : {}) } }])
      .then(() => undefined);
  }
  click(x: number, y: number, o: { button?: string; clickCount?: number } = {}) {
    return this.step(o.clickCount === 2 ? 'dblclick' : 'click', { x, y, button: o.button });
  }
  dblclick(x: number, y: number, o: { button?: string } = {}) {
    return this.step('dblclick', { x, y, button: o.button });
  }
  move(x: number, y: number, o: { steps?: number } = {}) {
    return this.step('move', { x, y, steps: o.steps });
  }
  down(o: { button?: string } = {}) {
    return this.step('down', { button: o.button });
  }
  up(o: { button?: string } = {}) {
    return this.step('up', { button: o.button });
  }
  wheel(deltaX: number, deltaY: number) {
    return this.step('wheel', { deltaX, deltaY });
  }
}

export class Page extends Scope {
  readonly _page = this;
  readonly keyboard = new Keyboard(this);
  readonly mouse = new Mouse(this);
  readonly coverage: Coverage = new Coverage(this);
  /** The context's fake clock. */
  get clock(): Clock {
    return this.context().clock;
  }
  private _url = 'about:blank';
  private _mainFrameId?: string;
  private _mainFrame = new Frame(this);
  private _closed = false;
  private _defaultTimeout?: number;
  private _defaultNavigationTimeout?: number;
  private _viewport: { width: number; height: number } | null;
  _network?: Promise<void>;
  _runtime?: Promise<void>;
  _fetch?: Promise<void>;
  /** Asked for before the page has a target: sent as part of attaching it. */
  _wants = { network: false, runtime: false, fetch: false, fileChooser: false, workers: false };
  private _workers = new Map<string, Worker>();
  /** `page.addInitScript` before the page has a target. */
  _pendingScripts: string[] = [];
  _routes: RouteEntry[] = [];
  /** Child frames by id, from Page events (or the frame tree when attaching to a booted page). */
  private _frames = new Map<string, Frame>();
  /** Child frames' seqs (`f1e5` refs), by frame id, never reused. */
  private _frameSeqs = new Map<string, number>();
  private _marking = new Map<string, Promise<void>>();
  /** `exposeBinding` callbacks by binding name. */
  _bindings = new Map<string, { name: string; callback: (source: any, ...args: any[]) => unknown; script: string }>();
  private _dialogsBlock = false;
  _sessionId = '';
  _targetId = '';
  private _attaching?: Promise<Attachment>;
  private _requests = new Map<string, Request>();
  /** Open sockets by CDP requestId, from `Network.webSocket*` events. */
  private _sockets = new Map<string, WebSocket>();
  private _off: (() => void)[] = [];
  private _events = new EventEmitter();
  private _navigations = new EventEmitter();

  constructor(
    private _context: BrowserContext,
    private _cdp: Cdp,
    viewport: { width: number; height: number } | null,
  ) {
    super();
    this._viewport = viewport;
    this._events.setMaxListeners(0);
    this._navigations.setMaxListeners(0);
    const mine = (fn: (p: any) => void) => (p: any, sid?: string) => !!sid && sid === this._sessionId && fn(p);
    const on = (method: string, fn: (p: any) => void) => this._off.push(_cdp.on(method, mine(fn)));
    on('Page.frameNavigated', (p) => {
      if (p.frame.parentId) {
        const f = this.childFrame(p.frame.id, p.frame.parentId);
        f._url = p.frame.url + (p.frame.urlFragment ?? '');
        f._name = p.frame.name ?? '';
        this._events.emit('framenavigated', f);
        return;
      }
      this._mainFrameId = p.frame.id;
      this.setUrl(p.frame.url + (p.frame.urlFragment ?? ''));
    });
    on('Page.frameAttached', (p) => {
      if (p.parentFrameId) this._events.emit('frameattached', this.childFrame(p.frameId, p.parentFrameId));
    });
    on('Page.frameDetached', (p) => {
      const f = this._frames.get(p.frameId);
      if (!f) return;
      f._detached = true;
      this._frames.delete(p.frameId);
      this._marking.delete(p.frameId);
      this._events.emit('framedetached', f);
    });
    on('Runtime.bindingCalled', (p) => void this.onBinding(p));
    on('Page.fileChooserOpened', (p) => void this.onFileChooser(p));
    on('Target.attachedToTarget', (p) => {
      const info = p.targetInfo;
      if (info?.type !== 'worker' || this._workers.has(p.sessionId)) return;
      const worker = new Worker(this, this._cdp, p.sessionId, info.url);
      this._workers.set(p.sessionId, worker);
      if (p.waitingForDebugger) void this._cdp.send('Runtime.runIfWaitingForDebugger', {}, p.sessionId).catch(() => {});
      this._events.emit('worker', worker);
    });
    this._off.push(
      _cdp.on('Target.detachedFromTarget', (p) => {
        const worker = p.sessionId && this._workers.get(p.sessionId);
        if (!worker) return;
        this._workers.delete(p.sessionId);
        worker._closed();
      }),
    );
    on('Page.navigatedWithinDocument', (p) => {
      if (!p.frameId || p.frameId === (this._mainFrameId ?? this._targetId)) this.setUrl(p.url);
    });
    on('Page.javascriptDialogOpening', (p) => {
      const dialog = new Dialog(this, p.type, p.message, p.defaultPrompt ?? '');
      if (this._events.listenerCount('dialog')) this._events.emit('dialog', dialog);
      else dialog.dismiss().catch(() => {});
      this._syncDialogs();
    });
    on('Page.loadEventFired', () => this._events.emit('load', this));
    on('Page.domContentEventFired', () => this._events.emit('domcontentloaded', this));
    on('Network.requestWillBeSent', (p) => this.onRequest(p));
    on('Network.responseReceived', (p) => {
      const req = this._requests.get(p.requestId);
      if (!req) return;
      req._response = new Response(this, req, p.response);
      this._fire('response', req._response);
    });
    on('Network.loadingFinished', (p) => {
      const req = this._requests.get(p.requestId);
      if (!req) return;
      this._requests.delete(p.requestId);
      req._finish();
      this._fire('requestfinished', req);
    });
    on('Network.loadingFailed', (p) => {
      const req = this._requests.get(p.requestId);
      if (!req) return;
      this._requests.delete(p.requestId);
      req._failure = p.errorText ?? 'failed';
      req._finish();
      this._fire('requestfailed', req);
    });
    // Chrome's order: created, handshake request (the page's `websocket`
    // event, as in Playwright), frames, then closed (after `socketerror` on a failure).
    on('Network.webSocketCreated', (p) => this._sockets.set(p.requestId, new WebSocket(this, p.url)));
    on('Network.webSocketWillSendHandshakeRequest', (p) => {
      const ws = this._sockets.get(p.requestId);
      if (ws && ws._markNotified()) this._events.emit('websocket', ws);
    });
    on('Network.webSocketFrameSent', (p) => this._sockets.get(p.requestId)?._frame('framesent', p.response));
    on('Network.webSocketFrameReceived', (p) => this._sockets.get(p.requestId)?._frame('framereceived', p.response));
    on('Network.webSocketFrameError', (p) => this._sockets.get(p.requestId)?._error(p.errorMessage));
    on('Network.webSocketClosed', (p) => {
      const ws = this._sockets.get(p.requestId);
      this._sockets.delete(p.requestId);
      ws?._closed();
    });
    on('Runtime.consoleAPICalled', (p) => {
      const text = (p.args ?? []).map((a: any) => (a.value !== undefined ? String(a.value) : (a.description ?? a.type))).join(' ');
      const frame = p.stackTrace?.callFrames?.[0];
      this._fire('console', new ConsoleMessage(this, p.type, text, { url: frame?.url ?? '', lineNumber: frame?.lineNumber ?? 0, columnNumber: frame?.columnNumber ?? 0 }));
    });
    on('Runtime.exceptionThrown', (p) => {
      const d = p.exceptionDetails ?? {};
      const err = new Error(d.exception?.description?.split('\n')[0] ?? d.text ?? 'Uncaught exception');
      if (d.exception?.description) err.stack = d.exception.description;
      this._events.emit('pageerror', err);
    });
    on('Fetch.requestPaused', (p) => void this.onPaused(p));
    this._off.push(
      _cdp.on('Target.detachedFromTarget', (p) => {
        if (p.sessionId && p.sessionId === this._sessionId) this.markClosed();
      }),
    );
  }

  /**
   * The page gets its target on its first command: a pre-booted one when
   * that command is a `goto` the browser's `boot` config covers, else a new
   * one (see `BrowserContext._attachPage`).
   */
  _attach(gotoUrl?: string): Promise<Attachment> {
    return (this._attaching ??= this._context._attachPage(this, gotoUrl));
  }
  get _isAttached(): boolean {
    return this._attaching !== undefined;
  }
  /** A popup: the target already exists and `attaching` binds and sets it up. */
  _adopt(attaching: Promise<Attachment>) {
    this._attaching = attaching;
  }
  /** Child frames of the frame tree (`Page.getFrameTree`). */
  _seedFrames(tree: { frame: { id: string; parentId?: string; url: string; urlFragment?: string; name?: string }; childFrames?: any[] }) {
    for (const child of tree.childFrames ?? []) {
      const f = this.childFrame(child.frame.id, tree.frame.id);
      f._url = child.frame.url + (child.frame.urlFragment ?? '');
      f._name = child.frame.name ?? '';
      this._seedFrames(child);
    }
  }
  private childFrame(id: string, parentId: string): Frame {
    let f = this._frames.get(id);
    const parent = parentId === this._mainFrameId || !this._frames.has(parentId) ? this._mainFrame : this._frames.get(parentId)!;
    if (!f) {
      this._frames.set(id, (f = new Frame(this, id, parent)));
      // Playwright's `frame.seq`: the page's frames in the order they appear (the main frame is 0).
      if (!this._frameSeqs.has(id)) this._frameSeqs.set(id, this._frameSeqs.size + 1);
    } else f._parent = parent;
    return f;
  }
  get _mainFrameIdValue(): string | undefined {
    return this._mainFrameId;
  }
  /** Children of `frame`; marks their `<iframe>` elements so their locators can find them. */
  _childFrames(frame: Frame): Frame[] {
    const out = [...this._frames.values()].filter((f) => (frame._id ? f._parent === frame : f._parent === this._mainFrame));
    for (const f of out) this.markFrame(f);
    return out;
  }
  /** Marks every listed child frame's `<iframe>` and waits for it. */
  async _markFrames(): Promise<void> {
    for (const f of this._frames.values()) this.markFrame(f);
    await Promise.all(this._marking.values());
  }
  _frameById(id: string): Frame | null {
    return this._frames.get(id) ?? null;
  }
  /** The frame whose `<iframe>` chain is `owner` (as `Frame._owner` builds it). */
  _frameOf(owner?: LocatorSpec): Frame | null {
    if (!owner) return null;
    const id = /^\[data-lp-frame="([^"]+)"\]$/.exec(owner.css ?? '')?.[1];
    return id ? this._frameById(id) : null;
  }
  /** Resolves when `frame` next navigates; rejects after `ms`. */
  _frameNavigation(frame: Frame, ms: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const done = (f: Frame) => {
        if (f !== frame) return;
        clearTimeout(timer);
        this._events.off('framenavigated', done);
        resolve();
      };
      const timer = setTimeout(() => {
        this._events.off('framenavigated', done);
        reject(new TimeoutError(`frame navigation: Timeout ${ms}ms exceeded`));
      }, ms);
      this._events.on('framenavigated', done);
    });
  }
  _locatorFor(spec: LocatorSpec, desc: string): Locator {
    return new Locator(this, spec, desc);
  }
  private markFrame(f: Frame): void {
    if (!f._id || this._marking.has(f._id)) return;
    const id = f._id;
    const done = (async () => {
      const { backendNodeId } = await this._send('DOM.getFrameOwner', { frameId: id });
      const { object } = await this._send('DOM.resolveNode', { backendNodeId });
      await this._send('Runtime.callFunctionOn', {
        objectId: object.objectId,
        functionDeclaration: 'function (id) { this.setAttribute("data-lp-frame", id); }',
        arguments: [{ value: id }],
      });
      await this._send('Runtime.releaseObject', { objectId: object.objectId }).catch(() => {});
    })().catch(() => {
      this._marking.delete(id);
    });
    this._marking.set(id, done);
  }
  /** The target the context made or took for this page. */
  _bind(sessionId: string, targetId: string, mainFrameId?: string, url?: string) {
    this._sessionId = sessionId;
    this._targetId = targetId;
    if (mainFrameId) this._mainFrameId = mainFrameId;
    if (url) this.setUrl(url);
  }

  private setUrl(url: string) {
    this._url = url;
    this._navigations.emit('navigated');
    this._events.emit('framenavigated', this._mainFrame);
  }

  /** Resolves on the next URL change, or after `ms`. */
  _nextNavigation(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const done = () => {
        clearTimeout(timer);
        this._navigations.off('navigated', done);
        resolve();
      };
      const timer = setTimeout(done, ms);
      this._navigations.once('navigated', done);
    });
  }

  private onRequest(p: any) {
    const prev = this._requests.get(p.requestId);
    if (prev && p.redirectResponse) {
      prev._response = new Response(this, prev, p.redirectResponse);
      prev._finish();
      this._fire('response', prev._response);
      this._fire('requestfinished', prev);
    }
    const req = new Request(this, p.requestId, p.request, p.type ?? 'Other');
    if (prev && p.redirectResponse) {
      req._redirectedFrom = prev;
      prev._redirectedTo = req;
    }
    this._requests.set(p.requestId, req);
    this._fire('request', req);
  }

  private markClosed() {
    if (this._closed) return;
    this._closed = true;
    // Taken out first: an `off` may remove itself from `_off` (coverage does).
    for (const off of this._off.splice(0)) off();
    this._context._pageClosed(this);
    this._events.emit('close', this);
  }

  // ---- plumbing ----

  async _send<T = any>(method: string, params: object = {}): Promise<T> {
    await this._attach();
    return this._cdp.send(method, params, this._sessionId);
  }
  /** On the attached session only (setup while attaching, cleanup). */
  _sendNow<T = any>(method: string, params: object = {}): Promise<T> {
    return this._cdp.send(method, params, this._sessionId);
  }

  /** One LP.run command; failures as Playwright errors. */
  async _run(api: string, steps: Step[], describe?: string): Promise<RunReply> {
    if (this._closed) throw new Error(`${api}: Target page, context or browser has been closed`);
    try {
      await this._attach();
      // A child frame's locators find its <iframe> by the marker `frames()` sets.
      if (this._marking.size && JSON.stringify(steps).includes('data-lp-frame')) await Promise.all(this._marking.values());
      return await this._cdp.send('LP.run', { steps }, this._sessionId);
    } catch (e) {
      throw runError(api, e, describe);
    }
  }

  /** One LP.run command with the engine's error as it is (its `data` included). */
  async _runRaw(steps: Step[]): Promise<RunReply> {
    await this._attach();
    return this._cdp.send('LP.run', { steps }, this._sessionId);
  }
  /** Screenshot baselines this connection already holds (`toHaveScreenshot`). */
  get _baselines(): Map<string, string> {
    let m = SENT.get(this._cdp);
    if (!m) SENT.set(this._cdp, (m = new Map()));
    return m;
  }

  /** An eval step that calls `fn` (with a locator's element(s) and `arg`); its value. */
  async _call<T>(api: string, fn: string, o: { locator?: LocatorSpec; all?: boolean; arg?: unknown; timeout?: number }, describe?: string): Promise<T> {
    const step = {
      eval: {
        expression: fn,
        call: true,
        as: 'v',
        ...(o.locator ? { locator: o.locator } : {}),
        ...(o.all ? { all: true } : {}),
        ...(o.arg === undefined ? {} : { arg: o.arg }),
        timeout: o.all ? 0 : this._timeout(o.timeout),
      },
    };
    const r = await this._run(api, [step], describe);
    return r.reads.v;
  }

  /** A key press on the focused element (the body when nothing has focus). */
  async _focused(api: string, action: string, value: string): Promise<void> {
    const focused = await this._call<boolean>(api, '() => !!document.activeElement && document.activeElement !== document.body', {});
    const locator = focused ? { css: ':focus' } : { css: 'body' };
    await this._run(api, [{ act: { locator, action, value, timeout: this._timeout() } }]);
  }

  _timeout(timeout?: number): number {
    const t = timeout ?? this._defaultTimeout ?? this._context._defaultTimeout ?? 0;
    return t > 0 ? Math.min(t, 600_000) : 30_000;
  }
  private navTimeout(timeout?: number): number {
    return this._timeout(timeout ?? this._defaultNavigationTimeout ?? this._context._defaultNavigationTimeout ?? this._defaultTimeout);
  }
  _testIdAttribute(): string {
    return this._context._testIdAttribute;
  }
  protected _child(spec: LocatorSpec, desc: string): Locator {
    return new Locator(this, spec, desc);
  }

  // ---- events ----

  on<K extends keyof PageEvents>(event: K, listener: (arg: PageEvents[K]) => unknown): this {
    this.enableFor(event);
    this._events.on(event, listener as (...args: any[]) => void);
    if (event === 'dialog') this._syncDialogs();
    return this;
  }
  addListener<K extends keyof PageEvents>(event: K, listener: (arg: PageEvents[K]) => unknown): this {
    return this.on(event, listener);
  }
  once<K extends keyof PageEvents>(event: K, listener: (arg: PageEvents[K]) => unknown): this {
    this.enableFor(event);
    this._events.once(event, listener as (...args: any[]) => void);
    if (event === 'dialog') this._syncDialogs();
    return this;
  }
  off<K extends keyof PageEvents>(event: K, listener: (arg: PageEvents[K]) => unknown): this {
    this._events.off(event, listener as (...args: any[]) => void);
    if (event === 'dialog') this._syncDialogs();
    return this;
  }
  /** A page event that the context emits too (requests, responses, console). */
  private _fire(event: 'request' | 'response' | 'requestfinished' | 'requestfailed' | 'console', arg: unknown): void {
    this._events.emit(event, arg);
    this._context._emit(event, arg);
  }
  /** Emits a page event (popups and downloads come from the browser connection). */
  _emit(event: string, arg: unknown): void {
    this._events.emit(event, arg);
  }
  /**
   * With a 'dialog' listener, dialogs an action opens wait for its answer
   * (`LP.dialogs {block}`), as in Chrome; without one they are dismissed.
   */
  _syncDialogs(): void {
    const want = this._events.listenerCount('dialog') > 0;
    if (want === this._dialogsBlock) return;
    this._dialogsBlock = want;
    if (this._isAttached) void this._send('LP.dialogs', { block: want }).catch(() => {});
  }
  get _wantsDialogs(): boolean {
    return this._dialogsBlock;
  }
  removeListener<K extends keyof PageEvents>(event: K, listener: (arg: PageEvents[K]) => unknown): this {
    return this.off(event, listener);
  }
  removeAllListeners(event?: string): this {
    this._events.removeAllListeners(event);
    return this;
  }
  private enableFor(event: string) {
    if (['request', 'response', 'requestfinished', 'requestfailed', 'websocket'].includes(event)) void this._enableNetwork();
    if (['console', 'pageerror'].includes(event)) {
      if (!this._isAttached) this._wants.runtime = true;
      else this._runtime ??= this._send('Runtime.enable').then(() => undefined);
    }
    if (event === 'filechooser' && !this._wants.fileChooser) {
      this._wants.fileChooser = true;
      if (this._isAttached) void this._send('Page.setInterceptFileChooserDialog', { enabled: true }).catch(() => {});
    }
    if (event === 'worker' && !this._wants.workers) {
      this._wants.workers = true;
      if (this._isAttached) void this._send('Target.setAutoAttach', WORKER_AUTO_ATTACH).catch(() => {});
    }
  }
  workers(): Worker[] {
    return [...this._workers.values()];
  }
  /** A file input's chooser: `filechooser` listeners get it, its element as a handle. */
  private async onFileChooser(p: { frameId: string; mode: string; backendNodeId: number }): Promise<void> {
    if (!this._events.listenerCount('filechooser')) return;
    const { object } = await this._send('DOM.resolveNode', { backendNodeId: p.backendNodeId });
    const frame = p.frameId && p.frameId !== this._mainFrameId ? this._frames.get(p.frameId) : undefined;
    if (frame) await this._markFrames();
    const element = this._handle(object, frame?._owner) as ElementHandle;
    this._events.emit('filechooser', new FileChooser(this, element, p.mode === 'selectMultiple'));
  }
  /** Network events from now on (requests already in flight are not seen). */
  _enableNetwork(): Promise<void> {
    if (!this._isAttached) {
      this._wants.network = true;
      return Promise.resolve();
    }
    return (this._network ??= this._send('Network.enable').then(() => undefined));
  }
  /** Request interception for `route()` (every request pauses until a handler or the default decides). */
  _enableFetch(): Promise<void> {
    if (!this._isAttached) {
      this._wants.fetch = true;
      return Promise.resolve();
    }
    return (this._fetch ??= this._send('Fetch.enable', { patterns: [{ urlPattern: '*' }] }).then(() => undefined));
  }

  async waitForEvent<K extends keyof PageEvents>(
    event: K,
    optionsOrPredicate?: ((arg: PageEvents[K]) => boolean | Promise<boolean>) | (Timeout & { predicate?: (arg: PageEvents[K]) => boolean | Promise<boolean> }),
  ): Promise<PageEvents[K]> {
    const o = typeof optionsOrPredicate === 'function' ? { predicate: optionsOrPredicate } : (optionsOrPredicate ?? {});
    this.enableFor(event);
    return this.waitEvent(`page.waitForEvent(${quote(event)})`, event, o.predicate ?? (() => true), o.timeout);
  }
  private waitEvent<T>(api: string, event: string, predicate: (arg: T) => boolean | Promise<boolean>, timeout?: number): Promise<T> {
    const ms = this._timeout(timeout);
    return new Promise<T>((resolve, reject) => {
      const done = (fn: () => void) => {
        clearTimeout(timer);
        this._events.off(event, listener as (...args: any[]) => void);
        this._events.off('close', closed);
        fn();
      };
      const listener = async (arg: T) => {
        try {
          if (await predicate(arg)) done(() => resolve(arg));
        } catch (e) {
          done(() => reject(e));
        }
      };
      const closed = () => event !== 'close' && done(() => reject(new Error(`${api}: Target page, context or browser has been closed`)));
      const timer = setTimeout(() => done(() => reject(new TimeoutError(`${api}: Timeout ${ms}ms exceeded while waiting for event "${event}"`))), ms);
      this._events.on(event, listener as (...args: any[]) => void);
      this._events.on('close', closed);
    });
  }
  async waitForResponse(urlOrPredicate: string | RegExp | ((response: Response) => boolean | Promise<boolean>), options: Timeout = {}): Promise<Response> {
    const enabled = this._enableNetwork();
    const base = this._context._options.baseURL;
    const predicate =
      typeof urlOrPredicate === 'function'
        ? (r: Response) => (urlOrPredicate as (r: Response) => boolean | Promise<boolean>)(r)
        : (r: Response) => urlMatches(base, r.url(), urlOrPredicate);
    const api = `page.waitForResponse(${typeof urlOrPredicate === 'function' ? 'predicate' : quote(urlOrPredicate as string)})`;
    const wait = this.waitEvent(api, 'response', predicate, options.timeout);
    await enabled;
    return wait;
  }
  async waitForRequest(urlOrPredicate: string | RegExp | ((request: Request) => boolean | Promise<boolean>), options: Timeout = {}): Promise<Request> {
    const enabled = this._enableNetwork();
    const base = this._context._options.baseURL;
    const predicate =
      typeof urlOrPredicate === 'function'
        ? (r: Request) => (urlOrPredicate as (r: Request) => boolean | Promise<boolean>)(r)
        : (r: Request) => urlMatches(base, r.url(), urlOrPredicate);
    const api = `page.waitForRequest(${typeof urlOrPredicate === 'function' ? 'predicate' : quote(urlOrPredicate as string)})`;
    const wait = this.waitEvent(api, 'request', predicate, options.timeout);
    await enabled;
    return wait;
  }

  // ---- navigation ----

  url(): string {
    return this._url;
  }
  mainFrame(): Frame {
    return this._mainFrame;
  }
  frames(): Frame[] {
    const all = [this._mainFrame, ...this._frames.values()];
    for (const f of this._frames.values()) this.markFrame(f);
    return all;
  }
  frame(selector: string | { name?: string; url?: string | RegExp | ((url: URL) => boolean) }): Frame | null {
    const o = typeof selector === 'string' ? { name: selector } : selector;
    for (const f of this.frames()) {
      if (o.name !== undefined && f.name() !== o.name) continue;
      if (o.url !== undefined) {
        const u = f.url();
        const ok = typeof o.url === 'string' ? u === o.url : o.url instanceof RegExp ? o.url.test(u) : (() => { try { return o.url(new URL(u)); } catch { return false; } })();
        if (!ok) continue;
      }
      return f;
    }
    return null;
  }
  context(): BrowserContext {
    return this._context;
  }
  isClosed(): boolean {
    return this._closed;
  }

  async goto(url: string, options: GotoOptions = {}): Promise<Response | null> {
    const target = resolveUrl(this._context._options.baseURL, url);
    const waitUntil = options.waitUntil === 'domcontentloaded' || options.waitUntil === 'commit' ? 'domcontentloaded' : 'load';
    const timeout = this.navTimeout(options.timeout);
    if (!this._isAttached) {
      const a = await this._attach(target);
      if (a.pooled) {
        // A page booted ahead at the boot URL: the app changes route in place.
        const steps: Step[] = a.ready ? [] : [{ loaded: { timeout } }];
        if (target !== a.url) steps.push({ route: { url: target, timeout } });
        if (options.waitUntil === 'networkidle') steps.push({ idle: { timeout } });
        if (steps.length) await this._run('page.goto', steps, `navigating to "${target}"`);
        return null;
      }
    }
    const steps: Step[] = [{ goto: { url: target, waitUntil, timeout } }];
    if (options.waitUntil === 'networkidle') steps.push({ idle: { timeout } });
    const r = await this._run(`page.goto`, steps, `navigating to "${target}"`);
    return this.documentResponse(r, target);
  }
  /** The main document's response from the run's request log. */
  private documentResponse(r: RunReply, target: string): Response | null {
    const entry = r.requests.find((q) => q.url === target || q.url === this._url) ?? r.requests[0];
    if (!entry || entry.status === null) return null;
    const req = new Request(this, '', { url: entry.url, method: entry.method, headers: {} }, 'Document');
    req._finish();
    const res = new Response(this, req, { url: entry.url, status: entry.status });
    req._response = res;
    return res;
  }
  async reload(options: GotoOptions = {}): Promise<Response | null> {
    const timeout = this.navTimeout(options.timeout);
    const steps: Step[] = [{ reload: { timeout } }];
    if (options.waitUntil === 'networkidle') steps.push({ idle: { timeout } });
    const r = await this._run('page.reload', steps);
    return this.documentResponse(r, this._url);
  }
  async goBack(options: GotoOptions = {}): Promise<null> {
    return this.history('page.goBack', -1, options);
  }
  async goForward(options: GotoOptions = {}): Promise<null> {
    return this.history('page.goForward', 1, options);
  }
  private async history(api: string, delta: number, options: GotoOptions): Promise<null> {
    const timeout = this.navTimeout(options.timeout);
    await this._run(api, [
      { eval: { expression: `(d) => history.go(d)`, call: true, arg: delta } },
      { idle: { timeout } },
      { loaded: { timeout } },
    ]);
    return null;
  }
  async waitForLoadState(state: 'load' | 'domcontentloaded' | 'networkidle' = 'load', options: Timeout = {}): Promise<void> {
    const timeout = this.navTimeout(options.timeout);
    await this._run('page.waitForLoadState', [state === 'networkidle' ? { idle: { timeout } } : { loaded: { timeout } }]);
  }
  async waitForURL(url: UrlMatch, options: Timeout & { waitUntil?: GotoOptions['waitUntil'] } = {}): Promise<void> {
    const ms = this.navTimeout(options.timeout);
    const deadline = Date.now() + ms;
    const base = this._context._options.baseURL;
    while (!(await urlMatches(base, this._url, url))) {
      const left = deadline - Date.now();
      if (left <= 0) throw new TimeoutError(`page.waitForURL: Timeout ${ms}ms exceeded.\n  waiting for navigation to ${typeof url === 'function' ? 'match predicate' : quote(url as string)}\n  current URL is "${this._url}"`);
      await this._nextNavigation(left);
    }
    if (options.waitUntil !== 'commit') await this.waitForLoadState(options.waitUntil === 'domcontentloaded' ? 'domcontentloaded' : options.waitUntil === 'networkidle' ? 'networkidle' : 'load', { timeout: Math.max(1, deadline - Date.now()) });
  }
  async waitForTimeout(ms: number): Promise<void> {
    await delay(ms);
  }
  async waitForSelector(selector: string, options: Timeout & { state?: 'attached' | 'detached' | 'visible' | 'hidden'; strict?: boolean } = {}): Promise<ElementHandle<PageElement> | null> {
    const state = options.state ?? 'visible';
    const loc = this.locator(selector);
    await (state === 'visible' ? loc.filter({ visible: true }).first() : loc.first()).waitFor(options);
    if (state === 'hidden' || state === 'detached') return null;
    const h = await this._handleFrom('page.waitForSelector', state === 'visible' ? VISIBLE_FIRST : '(els) => els[0] ?? null', { locator: loc._spec, all: true });
    return h.asElement() as ElementHandle<PageElement> | null;
  }
  async waitForFunction<R, A>(fn: string | ((arg: A) => R | Promise<R>), arg?: A, options: Timeout & { polling?: number | 'raf' } = {}): Promise<{ jsonValue(): Promise<R> }> {
    const interval = typeof options.polling === 'number' ? options.polling : 16;
    const body = typeof fn === 'function' ? fn.toString() : `() => (${fn})`;
    const poll = `async (arg) => { const f = (${body}); for (;;) { const v = await f(arg); if (v) return v; await new Promise((r) => setTimeout(r, ${interval})); } }`;
    const value = await this._call<R>('page.waitForFunction', poll, { arg, timeout: options.timeout });
    return { jsonValue: async () => value };
  }

  // ---- content ----

  evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<R>;
  evaluate<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<R>;
  async evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<R> {
    const fn = pageFunction as string | ((arg: unknown) => unknown);
    if (hasHandles(arg)) return this._callOn(null, fn, arg, true) as Promise<R>;
    if (typeof fn === 'string') {
      const r = await this._run('page.evaluate', [{ eval: { expression: fn, as: 'v', timeout: this._timeout() } }]);
      return r.reads.v;
    }
    return this._call('page.evaluate', fn.toString(), { arg });
  }
  evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<SmartHandle<R>>;
  evaluateHandle<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<SmartHandle<R>>;
  async evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<SmartHandle<R>> {
    const fn = pageFunction as string | ((arg: unknown) => unknown);
    if (hasHandles(arg)) return this._callOn(null, fn, arg, false) as Promise<SmartHandle<R>>;
    const expression = typeof fn === 'string' ? fn : `(${fn.toString()}\n)(${arg === undefined ? 'undefined' : JSON.stringify(arg)})`;
    const r = await this._send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: false });
    if (r.exceptionDetails) throw new Error(`page.evaluateHandle: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return this._handle(r.result) as SmartHandle<R>;
  }
  /** A remote object as a handle (an element handle for a DOM node). */
  _handle(remote: any, frame?: LocatorSpec, via?: Sender): JSHandle {
    return remote.subtype === 'node' ? new ElementHandle(this, remote, frame, via) : new JSHandle(this, remote, frame, via);
  }
  /**
   * `fn` (source) called in an LP.run `eval` step (with a locator's
   * element(s) and `arg`), its value handed over as a handle: the step
   * leaves it in a page-global queue that `Runtime.evaluate` takes it from.
   */
  async _handleFrom(
    api: string,
    fn: string,
    o: { locator?: LocatorSpec; all?: boolean; arg?: unknown; timeout?: number; inFrame?: boolean },
    describe?: string,
  ): Promise<JSHandle> {
    // `inFrame`: the locator is an <iframe> and `fn` returns a value of its document.
    const realmFrame = o.inFrame ? o.locator : o.locator?.frame;
    const stash = o.inFrame
      ? `async (x, arg) => { const v = await (${fn}\n)(x, arg); (x.contentWindow[Symbol.for('lp.handoff')] ??= []).push(v); return null; }`
      : `async (x, arg) => { const v = await (${fn}\n)(x, arg); (globalThis[Symbol.for('lp.handoff')] ??= []).push(v); return null; }`;
    await this._call(api, stash, o, describe);
    if (!realmFrame) {
      const r = await this._send('Runtime.evaluate', { expression: `globalThis[Symbol.for('lp.handoff')].shift()`, returnByValue: false });
      return this._handle(r.result);
    }
    // The value waits in the frame's window: taken through its <iframe> element.
    const iframe = await this._handleFrom(api, '(el) => el', { locator: realmFrame });
    try {
      const r = await this._send('Runtime.callFunctionOn', {
        objectId: iframe._objectId,
        functionDeclaration: `function () { return this.contentWindow[Symbol.for('lp.handoff')].shift(); }`,
        returnByValue: false,
      });
      if (r.exceptionDetails) throw new Error(`${api}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
      return this._handle(r.result, realmFrame);
    } finally {
      await iframe.dispose();
    }
  }
  /** `fn` called on `self` (or the page, when null) with `arg`, whose handles go as remote objects. */
  async _callOn(self: JSHandle | null, fn: string | Function, arg: unknown, byValue: boolean): Promise<unknown> {
    const { template, handles } = serializeArg(arg);
    const target = self?._objectId ?? handles.find((h) => h._objectId)?._objectId;
    if (!target) {
      // A primitive handle and no other: plain JSON.
      const args = [self ? valueOf(self._remote) : undefined, template];
      const src = typeof fn === 'function' ? fn.toString() : `() => (${fn})`;
      const value = await this._call('evaluate', `(a) => (${src}\n)(${self ? 'a[0], ' : ''}a[1])`, { arg: args });
      return byValue ? value : this._handle({ type: typeof value, value });
    }
    const send = self ? (m: string, p: object) => self._send(m, p) : (m: string, p: object) => this._send(m, p);
    const r = await send('Runtime.callFunctionOn', {
      functionDeclaration: callDeclaration(fn, !!self),
      objectId: target,
      arguments: [template === undefined ? {} : { value: template }, ...handles.map((h) => (h._objectId ? { objectId: h._objectId } : { value: valueOf(h._remote) }))],
      awaitPromise: true,
      returnByValue: byValue,
    });
    if (r.exceptionDetails) throw new Error(`evaluate: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return byValue ? valueOf(r.result) : this._handle(r.result, self?._frame, self?._via);
  }
  async $eval<R, Arg, E extends PageElement = PageElement>(selector: string, pageFunction: PageFunctionOn<E, Arg, R>, arg?: Arg): Promise<R> {
    return this.locator(selector).first().evaluate(pageFunction as PageFunctionOn<E, unknown, R>, arg as unknown);
  }
  async $$eval<R, Arg, E extends PageElement = PageElement>(selector: string, pageFunction: PageFunctionOn<E[], Arg, R>, arg?: Arg): Promise<R> {
    return this.locator(selector).evaluateAll(pageFunction as PageFunctionOn<E[], unknown, R>, arg as unknown);
  }
  /** The first match (no waiting), or null. */
  async $(selector: string): Promise<ElementHandle<PageElement> | null> {
    const h = await this._handleFrom('page.$', '(els) => els[0] ?? null', { locator: this.locator(selector)._spec, all: true });
    return h.asElement() as ElementHandle<PageElement> | null;
  }
  async $$(selector: string): Promise<ElementHandle<PageElement>[]> {
    return this.locator(selector).elementHandles();
  }
  /**
   * The page's ARIA tree as Playwright's YAML: from the main frame's
   * `<body>`, as Playwright's `ariaSnapshotForFrame` (in both modes).
   */
  async ariaSnapshot(options: Timeout & { depth?: number; mode?: 'ai' | 'default' } = {}): Promise<string> {
    if (options.mode === 'ai') return this._aiSnapshot('page.ariaSnapshot', { css: 'body' }, options.depth, options.timeout);
    return this.locator('body').ariaSnapshot(options);
  }
  /** Playwright's `ariaSnapshotForFrame` in AI mode: the tree under `spec`, then each shown `<iframe>`'s, nested under it. */
  async _aiSnapshot(api: string, spec: LocatorSpec, depth: number | undefined, timeout: number | undefined, desc?: string): Promise<string> {
    return (await this.aiLines(api, spec, depth, timeout, desc)).join('\n');
  }
  private async aiLines(api: string, spec: LocatorSpec, depth: number | undefined, timeout: number | undefined, desc?: string): Promise<string[]> {
    const frameSeqs = Object.fromEntries(this._frameSeqs);
    if (this._mainFrameId) frameSeqs[this._mainFrameId] = 0;
    const step = { read: { locator: spec, read: 'aria', mode: 'ai', frameSeqs, as: 'v', timeout: this._timeout(timeout) } };
    const tree = (await this._run(api, [step], desc)).reads.v as AriaNode & { error?: string; iframes?: { ref: string; frameId: string }[] };
    if (tree?.error) throw new Error(`${api}: ${tree.error}`);
    const iframeDepths: Record<string, number> = {};
    const text = renderAria(tree, depth, false, true, iframeDepths);
    const children = new Map<string, string[]>();
    for (const { ref } of tree.iframes ?? []) {
      if (!(ref in iframeDepths)) continue;
      // Playwright: `depth - iframeDepth - 1`, and no limit when that is 0.
      const childDepth = depth ? depth - iframeDepths[ref] - 1 : undefined;
      const child: LocatorSpec = { css: 'body', frame: { ariaRef: ref, ...(spec.frame ? { frame: spec.frame } : {}) } };
      children.set(ref, await this.aiLines(api, child, childDepth, timeout).catch(() => []));
    }
    const out: string[] = [];
    for (const line of text.split('\n')) {
      const m = /^(\s*)- iframe (?:\[active\] )?\[ref=([^\]]*)\]/.exec(line);
      if (!m) {
        out.push(line);
        continue;
      }
      const lines = children.get(m[2]) ?? [];
      out.push(lines.length ? line + ':' : line);
      out.push(...lines.map((l) => m[1] + '  ' + l));
    }
    return out;
  }
  async title(): Promise<string> {
    return this._call('page.title', '() => document.title', {});
  }
  async content(): Promise<string> {
    return this._call(
      'page.content',
      `() => (document.doctype ? '<!DOCTYPE ' + document.doctype.name + '>' : '') + document.documentElement.outerHTML`,
      {},
    );
  }
  async setContent(html: string, options: GotoOptions = {}): Promise<void> {
    const timeout = this.navTimeout(options.timeout);
    await this._run('page.setContent', [
      { eval: { expression: `(html) => { document.open(); document.write(html); document.close(); }`, call: true, arg: html } },
      { loaded: { timeout } },
    ]);
  }
  async addInitScript<Arg>(script: PageFunction<Arg, any> | { path?: string; content?: string }, arg?: Arg): Promise<void> {
    const source = initScriptSource(script as string | Function | { path?: string; content?: string }, arg);
    if (!this._isAttached) this._pendingScripts.push(source);
    else await this._send('Page.addScriptToEvaluateOnNewDocument', { source });
  }
  async addScriptTag(o: { url?: string; path?: string; content?: string; type?: string }): Promise<null> {
    const content = o.content ?? (o.path ? readFileSync(o.path, 'utf8') : undefined);
    await this._call(
      'page.addScriptTag',
      `({ url, content, type }) => new Promise((resolve, reject) => { const s = document.createElement('script'); if (type) s.type = type; if (url) { s.src = url; s.onload = () => resolve(null); s.onerror = () => reject(new Error('Failed to load script ' + url)); } else { s.textContent = content; } document.head.appendChild(s); if (!url) resolve(null); })`,
      { arg: { url: o.url, content, type: o.type } },
    );
    return null;
  }
  async addStyleTag(o: { url?: string; path?: string; content?: string }): Promise<null> {
    const content = o.content ?? (o.path ? readFileSync(o.path, 'utf8') : undefined);
    await this._call(
      'page.addStyleTag',
      `({ url, content }) => new Promise((resolve, reject) => { if (url) { const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = url; l.onload = () => resolve(null); l.onerror = () => reject(new Error('Failed to load ' + url)); document.head.appendChild(l); } else { const s = document.createElement('style'); s.textContent = content; document.head.appendChild(s); resolve(null); } })`,
      { arg: { url: o.url, content } },
    );
    return null;
  }

  // ---- viewport, screenshots ----

  viewportSize(): { width: number; height: number } | null {
    return this._viewport;
  }
  async setViewportSize(size: { width: number; height: number }): Promise<void> {
    await this._run('page.setViewportSize', [{ viewport: size }]);
    this._viewport = { ...size };
  }
  async emulateMedia(o: { colorScheme?: 'light' | 'dark' | 'no-preference' | null; reducedMotion?: 'reduce' | 'no-preference' | null; media?: 'screen' | 'print' | null } = {}): Promise<void> {
    await this._run('page.emulateMedia', [{ media: clean({ colorScheme: o.colorScheme ?? undefined, reducedMotion: o.reducedMotion ?? undefined, media: o.media ?? undefined }) }]);
  }
  async screenshot(o: Timeout & { path?: string; fullPage?: boolean; clip?: { x: number; y: number; width: number; height: number }; type?: 'png' | 'jpeg'; quality?: number } = {}): Promise<Buffer> {
    const r = await this._send('Page.captureScreenshot', {
      format: o.type ?? (o.path?.endsWith('.jpg') || o.path?.endsWith('.jpeg') ? 'jpeg' : 'png'),
      ...(o.quality !== undefined ? { quality: o.quality } : {}),
      ...(o.clip ? { clip: { ...o.clip, scale: 1 } } : {}),
      ...(o.fullPage ? { captureBeyondViewport: true } : {}),
    });
    const buffer = Buffer.from(r.data, 'base64');
    if (o.path) writeFileSync(o.path, buffer);
    return buffer;
  }
  async pdf(o: { path?: string; [k: string]: unknown } = {}): Promise<Buffer> {
    const { path, ...rest } = o;
    const r = await this._send('Page.printToPDF', rest);
    const buffer = Buffer.from(r.data, 'base64');
    if (path) writeFileSync(path, buffer);
    return buffer;
  }

  // ---- unsupported for now ----

  async route(url: UrlMatch, handler: RouteHandler, options: { times?: number } = {}): Promise<void> {
    this._routes.unshift({ url, handler, times: options.times });
    await this._enableFetch();
  }
  async unroute(url: UrlMatch, handler?: RouteHandler): Promise<void> {
    this._routes = this._routes.filter((r) => !sameRoute(r, url, handler));
  }
  async unrouteAll(_options: { behavior?: 'wait' | 'ignoreErrors' | 'default' } = {}): Promise<void> {
    this._routes = [];
  }
  async routeFromHAR(har: string, options: HarOptions = {}): Promise<void> {
    if (options.update) return this._context._recordHar(har, options, this);
    await this.route(options.url ?? '**/*', harHandler(har, options));
  }

  /** A request paused by `Fetch.enable`: the first matching route decides (page routes, then the context's). */
  private async onPaused(p: any): Promise<void> {
    const request = new Request(this, p.networkId ?? '', p.request, p.resourceType ?? 'Other');
    const route = new Route(this, p.requestId, request);
    const base = this._context._options.baseURL;
    try {
      for (const entry of [...this._routes, ...this._context._routes]) {
        if (entry.times !== undefined && entry.times <= 0) continue;
        if (!(await urlMatches(base, request.url(), entry.url))) continue;
        if (entry.times !== undefined) entry.times--;
        if (await route._handle(entry.handler)) return;
      }
      await route._continueDefault();
    } catch (e) {
      if (!this._closed && !(e instanceof CdpError)) console.error('lightning-panda: route handler failed:', e);
      await route._continueDefault().catch(() => {});
    }
  }
  async exposeFunction(name: string, fn: (...args: any[]) => unknown): Promise<void> {
    await this.exposeBinding(name, (_source: unknown, ...args: unknown[]) => fn(...args));
  }
  /** `window[name](...args)` in every document of this page calls `callback({ page, frame, context }, ...args)` here. */
  async exposeBinding(name: string, callback: (source: any, ...args: any[]) => unknown): Promise<void> {
    if (this._bindings.has(bindingName(name))) throw new Error(`page.exposeBinding: Function "${name}" has been already registered`);
    const entry = { name, callback, script: bindingScript(name) };
    this._bindings.set(bindingName(name), entry);
    if (this._isAttached) await this._installBinding(entry);
  }
  /** Adds the CDP binding and defines `window[name]` now and in later documents. */
  async _installBinding(b: { name: string; script: string }): Promise<void> {
    await this._sendNow('Runtime.addBinding', { name: bindingName(b.name) });
    await this._sendNow('Page.addScriptToEvaluateOnNewDocument', { source: b.script });
    await this._sendNow('Runtime.evaluate', { expression: b.script }).catch(() => {});
  }
  private async onBinding(p: { name: string; payload: string; executionContextId: number }): Promise<void> {
    const b = this._bindings.get(p.name) ?? this._context._bindings.get(p.name);
    if (!b) return;
    let call: { id: number; args: unknown[] };
    try {
      call = JSON.parse(p.payload);
    } catch {
      return;
    }
    const key = JSON.stringify(`${p.name}:${call.id}`);
    let expression: string;
    try {
      const value = await b.callback({ page: this, frame: this._mainFrame, context: this._context }, ...call.args);
      expression = `globalThis.__lpDeliver(${key}, true, ${JSON.stringify(value === undefined ? null : value)})`;
    } catch (e) {
      expression = `globalThis.__lpDeliver(${key}, false, ${JSON.stringify(String((e as Error)?.message ?? e))})`;
    }
    await this._sendNow('Runtime.evaluate', { expression, contextId: p.executionContextId }).catch(() => {});
  }

  // ---- misc ----

  setDefaultTimeout(timeout: number): void {
    this._defaultTimeout = timeout;
  }
  setDefaultNavigationTimeout(timeout: number): void {
    this._defaultNavigationTimeout = timeout;
  }
  async bringToFront(): Promise<void> {}
  async pause(): Promise<void> {}
  /** Always null: lightningpanda records no video. */
  video(): Video | null {
    return null;
  }
  /** API requests sharing this page's context settings (base URL, extra headers). */
  get request(): APIRequestContext {
    return this._context.request;
  }
  async setExtraHTTPHeaders(headers: { [key: string]: string }): Promise<void> {
    await this._enableNetwork();
    await this._send('Network.setExtraHTTPHeaders', { headers });
  }
  async opener(): Promise<null> {
    return null;
  }
  async close(_options: { runBeforeUnload?: boolean } = {}): Promise<void> {
    if (this._closed) return;
    if (this._attaching) {
      await this._attaching.catch(() => undefined);
      if (this._targetId) this._cdp.post('Target.closeTarget', { targetId: this._targetId });
    }
    this.markClosed();
  }
}

// ---- routes ----

export type Attachment = { pooled: boolean; ready: boolean; url?: string };
export type RouteHandler = (route: Route, request: Request) => unknown;
export type RouteEntry = { url: UrlMatch; handler: RouteHandler; times?: number };
export const sameRoute = (r: RouteEntry, url: UrlMatch, handler?: RouteHandler) =>
  String(r.url) === String(url) && (handler === undefined || r.handler === handler);

const ERROR_REASONS: Record<string, string> = {
  aborted: 'Aborted', accessdenied: 'AccessDenied', addressunreachable: 'AddressUnreachable', blockedbyclient: 'BlockedByClient',
  blockedbyresponse: 'BlockedByResponse', connectionaborted: 'ConnectionAborted', connectionclosed: 'ConnectionClosed',
  connectionfailed: 'ConnectionFailed', connectionrefused: 'ConnectionRefused', connectionreset: 'ConnectionReset',
  internetdisconnected: 'InternetDisconnected', namenotresolved: 'NameNotResolved', timedout: 'TimedOut', failed: 'Failed',
};
const MIME: Record<string, string> = {
  html: 'text/html', htm: 'text/html', js: 'text/javascript', mjs: 'text/javascript', css: 'text/css', json: 'application/json',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', svg: 'image/svg+xml', txt: 'text/plain', woff2: 'font/woff2',
};
const headerList = (h: Record<string, string>) => Object.entries(h).map(([name, value]) => ({ name, value: String(value) }));

type Overrides = { url?: string; method?: string; headers?: Record<string, string>; postData?: string | Buffer | object };

/** The response `route.fetch()` returns (Playwright's APIResponse shape). */
class FetchedResponse {
  constructor(
    private _url: string,
    private _status: number,
    private _statusText: string,
    private _headers: Record<string, string>,
    private _body: Buffer,
  ) {}
  url() { return this._url; }
  status() { return this._status; }
  statusText() { return this._statusText; }
  ok() { return this._status >= 200 && this._status <= 299; }
  headers() { return { ...this._headers }; }
  headersArray() { return headerList(this._headers); }
  async body() { return this._body; }
  async text() { return this._body.toString('utf8'); }
  async json() { return JSON.parse(this._body.toString('utf8')); }
  async dispose() {}
}

export class Route {
  private decided = false;
  private overrides: Overrides = {};
  private settle?: (handled: boolean) => void;

  constructor(
    private _page: Page,
    private _id: string,
    private _request: Request,
  ) {}
  request(): Request {
    return this._request;
  }

  /** Runs one handler: true once it fulfilled, continued or aborted; false on `fallback()`. */
  _handle(handler: RouteHandler): Promise<boolean> {
    return new Promise<boolean>((resolve, reject) => {
      this.settle = resolve;
      Promise.resolve()
        .then(() => handler(this, this._request))
        .catch(reject);
    });
  }
  private decide(): void {
    if (this.decided) throw new Error('Route is already handled!');
    this.decided = true;
  }
  async _continueDefault(): Promise<void> {
    if (this.decided) return;
    this.decided = true;
    await this.sendContinue();
  }
  private async sendContinue(): Promise<void> {
    const o = this.overrides;
    const postData = o.postData === undefined ? undefined : Buffer.isBuffer(o.postData) ? o.postData : Buffer.from(typeof o.postData === 'string' ? o.postData : JSON.stringify(o.postData));
    await this._page._sendNow('Fetch.continueRequest', {
      requestId: this._id,
      ...(o.url ? { url: o.url } : {}),
      ...(o.method ? { method: o.method } : {}),
      ...(o.headers ? { headers: headerList(o.headers) } : {}),
      ...(postData ? { postData: postData.toString('base64') } : {}),
    });
  }

  async fulfill(o: { status?: number; headers?: Record<string, string>; contentType?: string; body?: string | Buffer; json?: unknown; path?: string; response?: FetchedResponse | Response } = {}): Promise<void> {
    this.decide();
    let status = o.status ?? o.response?.status() ?? 200;
    const headers: Record<string, string> = {};
    for (const [k, v] of Object.entries(o.response?.headers() ?? {})) headers[k.toLowerCase()] = v;
    for (const [k, v] of Object.entries(o.headers ?? {})) headers[k.toLowerCase()] = v;
    let body: Buffer | undefined;
    if (o.json !== undefined) {
      body = Buffer.from(JSON.stringify(o.json));
      headers['content-type'] ??= 'application/json';
    } else if (o.body !== undefined) {
      body = Buffer.isBuffer(o.body) ? o.body : Buffer.from(o.body);
    } else if (o.path) {
      body = readFileSync(o.path);
      headers['content-type'] ??= MIME[o.path.split('.').pop()?.toLowerCase() ?? ''] ?? 'application/octet-stream';
    } else if (o.response) {
      body = await o.response.body();
    }
    if (o.contentType) headers['content-type'] = o.contentType;
    if (body && !headers['content-length'] && !headers['transfer-encoding']) headers['content-length'] = String(body.length);
    delete headers['content-encoding'];
    await this._page._sendNow('Fetch.fulfillRequest', {
      requestId: this._id,
      responseCode: status,
      responseHeaders: headerList(headers),
      body: (body ?? Buffer.alloc(0)).toString('base64'),
    });
    this.settle?.(true);
  }
  async continue(o: Overrides = {}): Promise<void> {
    this.decide();
    this.overrides = { ...this.overrides, ...o };
    await this.sendContinue();
    this.settle?.(true);
  }
  async abort(errorCode = 'failed'): Promise<void> {
    this.decide();
    await this._page._sendNow('Fetch.failRequest', { requestId: this._id, errorReason: ERROR_REASONS[errorCode.toLowerCase()] ?? 'Failed' });
    this.settle?.(true);
  }
  async fallback(o: Overrides = {}): Promise<void> {
    if (this.decided) throw new Error('Route is already handled!');
    this.overrides = { ...this.overrides, ...o };
    this.settle?.(false);
  }
  /** The request as it would go out (with `fallback`/`continue` overrides), from Node. */
  async fetch(o: Overrides & { maxRedirects?: number; timeout?: number } = {}): Promise<FetchedResponse> {
    const all = { ...this.overrides, ...o };
    const url = all.url ?? this._request.url();
    const method = all.method ?? this._request.method();
    const headers = { ...this._request.headers(), ...(all.headers ?? {}) };
    delete headers['host'];
    delete headers['content-length'];
    const body = all.postData !== undefined ? (typeof all.postData === 'string' || Buffer.isBuffer(all.postData) ? all.postData : JSON.stringify(all.postData)) : (this._request.postData() ?? undefined);
    const r = await fetch(url, { method, headers, body: method === 'GET' || method === 'HEAD' ? undefined : (body as any), redirect: all.maxRedirects === 0 ? 'manual' : 'follow' });
    const h: Record<string, string> = {};
    r.headers.forEach((v, k) => (h[k] = v));
    return new FetchedResponse(r.url, r.status, r.statusText, h, Buffer.from(await r.arrayBuffer()));
  }
}

/** Playwright's Video; `page.video()` is always null here. */
export interface Video {
  path(): Promise<string>;
  saveAs(path: string): Promise<void>;
  delete(): Promise<void>;
}

// ---- event maps ----

export interface PageEvents {
  close: Page;
  console: ConsoleMessage;
  dialog: Dialog;
  domcontentloaded: Page;
  download: Download;
  filechooser: FileChooser;
  frameattached: Frame;
  framedetached: Frame;
  framenavigated: Frame;
  load: Page;
  pageerror: Error;
  popup: Page;
  request: Request;
  requestfailed: Request;
  requestfinished: Request;
  response: Response;
  websocket: WebSocket;
  worker: Worker;
}
export interface WorkerEvents {
  close: Worker;
}
export interface WebSocketEvents {
  close: WebSocket;
  framereceived: { payload: string | Buffer };
  framesent: { payload: string | Buffer };
  socketerror: string;
}

// ---- workers and file choosers ----

const WORKER_AUTO_ATTACH = { autoAttach: true, waitForDebuggerOnStart: false, flatten: true, filter: [{ type: 'worker', exclude: false }] };

export class Worker {
  private _events = new EventEmitter();
  private _isClosed = false;
  constructor(
    private _page: Page,
    private _cdp: Cdp,
    readonly _sessionId: string,
    private _url: string,
  ) {}
  url(): string {
    return this._url;
  }
  private send = (method: string, params: object = {}) => this._cdp.send(method, params, this._sessionId);
  evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<R>;
  evaluate<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<R>;
  async evaluate<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<R> {
    const r = await this.run(pageFunction as string | Function, arg, true);
    return valueOf(r) as R;
  }
  evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg: Arg): Promise<SmartHandle<R>>;
  evaluateHandle<R>(pageFunction: PageFunction<void, R>, arg?: any): Promise<SmartHandle<R>>;
  async evaluateHandle<R, Arg>(pageFunction: PageFunction<Arg, R>, arg?: Arg): Promise<SmartHandle<R>> {
    return this._page._handle(await this.run(pageFunction as string | Function, arg, false), undefined, this.send) as SmartHandle<R>;
  }
  private async run(fn: string | Function, arg: unknown, byValue: boolean) {
    const expression = typeof fn === 'string' ? fn : `(${fn.toString()}\n)(${arg === undefined ? 'undefined' : JSON.stringify(arg)})`;
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: byValue });
    if (r.exceptionDetails) throw new Error(`worker.evaluate: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result;
  }
  _closed(): void {
    this._isClosed = true;
    this._events.emit('close', this);
  }
  isClosed(): boolean {
    return this._isClosed;
  }
  on<K extends keyof WorkerEvents>(event: K, listener: (arg: WorkerEvents[K]) => unknown): this {
    this._events.on(event, listener as (...args: any[]) => void);
    return this;
  }
  once<K extends keyof WorkerEvents>(event: K, listener: (arg: WorkerEvents[K]) => unknown): this {
    this._events.once(event, listener as (...args: any[]) => void);
    return this;
  }
  off<K extends keyof WorkerEvents>(event: K, listener: (arg: WorkerEvents[K]) => unknown): this {
    this._events.off(event, listener as (...args: any[]) => void);
    return this;
  }
  waitForEvent<K extends keyof WorkerEvents>(event: K, o: Timeout = {}): Promise<WorkerEvents[K]> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new TimeoutError(`worker.waitForEvent: Timeout exceeded waiting for "${event}"`)), o.timeout ?? 30_000);
      this._events.once(event, (v) => {
        clearTimeout(timer);
        resolve(v);
      });
    });
  }
}

/**
 * Playwright's `WebSocket`: a page's socket as `Network.webSocket*` events
 * report it. `framesent` / `framereceived` get `{ payload }` (a string for
 * text, a Buffer for binary), `socketerror` the error message, `close` the socket.
 */
export class WebSocket {
  private _events = new EventEmitter();
  private _isClosed = false;
  private _notified = false;
  constructor(
    private _page: Page,
    private _url: string,
  ) {
    this._events.setMaxListeners(0);
  }
  url(): string {
    return this._url;
  }
  isClosed(): boolean {
    return this._isClosed;
  }
  _markNotified(): boolean {
    if (this._notified) return false;
    this._notified = true;
    return true;
  }
  _frame(event: 'framesent' | 'framereceived', response: { opcode: number; payloadData: string }): void {
    // As Playwright: empty frames are not reported, binary ones are base64.
    if (!response?.payloadData) return;
    const payload = response.opcode === 2 ? Buffer.from(response.payloadData, 'base64') : response.payloadData;
    this._events.emit(event, { payload });
  }
  _error(message: string): void {
    this._events.emit('socketerror', message);
  }
  _closed(): void {
    if (this._isClosed) return;
    this._isClosed = true;
    this._events.emit('close', this);
  }
  on<K extends keyof WebSocketEvents>(event: K, listener: (arg: WebSocketEvents[K]) => unknown): this {
    this._events.on(event, listener as (...args: any[]) => void);
    return this;
  }
  addListener<K extends keyof WebSocketEvents>(event: K, listener: (arg: WebSocketEvents[K]) => unknown): this {
    return this.on(event, listener);
  }
  once<K extends keyof WebSocketEvents>(event: K, listener: (arg: WebSocketEvents[K]) => unknown): this {
    this._events.once(event, listener as (...args: any[]) => void);
    return this;
  }
  off<K extends keyof WebSocketEvents>(event: K, listener: (arg: WebSocketEvents[K]) => unknown): this {
    this._events.off(event, listener as (...args: any[]) => void);
    return this;
  }
  removeListener<K extends keyof WebSocketEvents>(event: K, listener: (arg: WebSocketEvents[K]) => unknown): this {
    return this.off(event, listener);
  }
  removeAllListeners(event?: string): this {
    this._events.removeAllListeners(event);
    return this;
  }
  /**
   * Resolves with the next `event` the predicate accepts. Rejects if the
   * socket closes first (unless waiting for `close`), or with a socket error
   * (unless waiting for `socketerror`), as Playwright does.
   */
  waitForEvent<K extends keyof WebSocketEvents>(
    event: K,
    optionsOrPredicate?: ((arg: WebSocketEvents[K]) => boolean | Promise<boolean>) | (Timeout & { predicate?: (arg: WebSocketEvents[K]) => boolean | Promise<boolean> }),
  ): Promise<WebSocketEvents[K]> {
    const o = typeof optionsOrPredicate === 'function' ? { predicate: optionsOrPredicate } : (optionsOrPredicate ?? {});
    const predicate = o.predicate ?? (() => true);
    const ms = this._page._timeout(o.timeout);
    const api = `webSocket.waitForEvent(${quote(event)})`;
    return new Promise((resolve, reject) => {
      const done = (fn: () => void) => {
        clearTimeout(timer);
        this._events.off(event, listener as (...args: any[]) => void);
        this._events.off('close', closed);
        this._events.off('socketerror', failed);
        fn();
      };
      const listener = async (arg: WebSocketEvents[K]) => {
        try {
          if (await predicate(arg)) done(() => resolve(arg));
        } catch (e) {
          done(() => reject(e));
        }
      };
      const closed = () => event !== 'close' && done(() => reject(new Error(`${api}: Socket closed`)));
      const failed = (message: string) => event !== 'socketerror' && done(() => reject(new Error(`${api}: Socket error: ${message}`)));
      const timer = setTimeout(() => done(() => reject(new TimeoutError(`${api}: Timeout ${ms}ms exceeded while waiting for event "${event}"`))), ms);
      this._events.on(event, listener as (...args: any[]) => void);
      this._events.on('close', closed);
      this._events.on('socketerror', failed);
    });
  }
  waitForFrameSent(o?: Timeout & { predicate?: (frame: { payload: string | Buffer }) => boolean }): Promise<any> {
    return this.waitForEvent('framesent', o);
  }
  waitForFrameReceived(o?: Timeout & { predicate?: (frame: { payload: string | Buffer }) => boolean }): Promise<any> {
    return this.waitForEvent('framereceived', o);
  }
}

export class FileChooser {
  constructor(
    private _page: Page,
    private _element: ElementHandle,
    private _multiple: boolean,
  ) {}
  page(): Page {
    return this._page;
  }
  element(): ElementHandle {
    return this._element;
  }
  isMultiple(): boolean {
    return this._multiple;
  }
  async setFiles(files: Parameters<Locator['setInputFiles']>[0], options: Timeout = {}): Promise<void> {
    await this._element.setInputFiles(files, options);
  }
}

export { WORKER_AUTO_ATTACH };

// ---- bindings and HAR ----

export const bindingName = (name: string) => `__lpb_${name}`;
/** Defines `window[name]`: each call goes out through the CDP binding and resolves on `__lpDeliver`. */
export function bindingScript(name: string): string {
  const b = JSON.stringify(bindingName(name));
  return `(() => {
  const binding = globalThis[${b}];
  if (typeof binding !== 'function') return;
  const calls = (globalThis.__lpBindingCalls ??= new Map());
  let seq = 0;
  globalThis.__lpDeliver ??= (key, ok, value) => { const c = calls.get(key); if (!c) return; calls.delete(key); ok ? c.resolve(value) : c.reject(new Error(value)); };
  Object.defineProperty(globalThis, ${JSON.stringify(name)}, { configurable: true, writable: true, value: (...args) => new Promise((resolve, reject) => {
    const id = ++seq;
    calls.set(${b} + ':' + id, { resolve, reject });
    binding(JSON.stringify({ id, args }));
  }) });
})();`;
}

export type HarOptions = { url?: UrlMatch; notFound?: 'abort' | 'fallback'; update?: boolean; updateContent?: string; updateMode?: string };

/** Replays a HAR file: the entry with the request's method and URL (and body, when several match). */
export function harHandler(path: string, options: HarOptions): RouteHandler {
  const har = JSON.parse(readFileSync(path, 'utf8'));
  const entries: any[] = har.log?.entries ?? [];
  const dir = path.replace(/[^/]*$/, '');
  return async (route, request) => {
    const candidates = entries.filter((e) => e.request?.method === request.method() && e.request?.url === request.url());
    const posted = request.postData();
    const entry = candidates.find((e) => (e.request?.postData?.text ?? null) === posted) ?? candidates[0];
    if (!entry) return options.notFound === 'fallback' ? route.fallback() : route.abort();
    const res = entry.response ?? {};
    const headers: Record<string, string> = {};
    for (const h of res.headers ?? []) if (!/^(content-length|content-encoding|transfer-encoding)$/i.test(h.name)) headers[h.name] = h.value;
    const c = res.content ?? {};
    const body = c._file ? readFileSync(dir + c._file) : c.text === undefined ? Buffer.alloc(0) : Buffer.from(c.text, c.encoding === 'base64' ? 'base64' : 'utf8');
    if (res.status >= 300 && res.status < 400 && res.redirectURL) headers.location ??= res.redirectURL;
    await route.fulfill({ status: res.status ?? 200, headers, body, ...(c.mimeType ? { contentType: c.mimeType } : {}) });
  };
}

/** In the page: the first visible element of the matches (`waitForSelector`'s default state). */
const VISIBLE_FIRST = `(els) => els.find((e) => e.checkVisibility ? e.checkVisibility({ visibilityProperty: true }) : e.getBoundingClientRect().width > 0) ?? els[0] ?? null`;

export function initScriptSource(script: string | Function | { path?: string; content?: string }, arg?: unknown): string {
  if (typeof script === 'function') return `(${script.toString()})(${arg === undefined ? 'undefined' : JSON.stringify(arg)});`;
  if (typeof script === 'string') return script;
  if (script.content !== undefined) return script.content;
  if (script.path) return readFileSync(script.path, 'utf8');
  throw new Error('addInitScript: pass a function, a string, { path } or { content }');
}
