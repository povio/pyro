// `expect` with Playwright's API, implemented here: web-first matchers on
// locators, pages and API responses retry in the browser (native checks)
// or on a value read in the page; every other matcher is Bun's (Jest's).
// `.not`, `.soft`, `.poll`, `.configure`, `.extend`, `toPass` and the
// asymmetric matchers (`expect.any`, `objectContaining`, ...) work as in
// Playwright.
import { basename, dirname, join, relative } from 'node:path';
import { CdpError } from '../cdp.js';
import { ScreenshotStep, type ScreenshotOptions } from '../steps.js';
import { mismatch, prepare, rewrite, settle } from '../screenshots.js';
import type { ExpectedText } from './match.js';
import { resolveUrl, urlMatches } from './match.js';
import { expectLocator, Frame, limit, Locator, Page, type ExpectOptions, type ExpectResult } from './page.js';
import { APIResponse } from './request.js';
import { currentTest } from './context.js';
import { anonymousName, sanitize, trimLongString, ariaSnapshotFile, lineDiff, matchFile, missingWritten, snapshotPath, updateSnapshots, type SnapshotOptions, type UpdateMode } from './snapshot.js';
import { dedent, matchesAria, parseAriaTemplate, renderAria, type AriaNode, type AriaTemplate } from './aria.js';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';

type BunExpect = ((value: unknown) => any) & Record<string, any>;
let bunExpect: BunExpect | undefined;
async function jest(): Promise<BunExpect> {
  if (!bunExpect) {
    if (!(globalThis as any).Bun) throw new Error('pyro: expect runs under `bun test`');
    bunExpect = require('bun:test').expect as unknown as BunExpect;
  }
  return bunExpect;
}

export type ExpectConfig = { timeout?: number; soft?: boolean; message?: string };
type Custom = (this: MatcherState, received: any, ...args: any[]) => { pass: boolean; message: () => string } | Promise<{ pass: boolean; message: () => string }>;
type MatcherState = { isNot: boolean; promise: string; timeout: number; utils: { printReceived: (v: unknown) => string; printExpected: (v: unknown) => string; stringify: (v: unknown) => string } };

const defaults = { timeout: 5_000 };
/** The config's `expect.timeout` (set by the runner). */
export function setExpectDefaults(o: { timeout?: number }) {
  if (o.timeout !== undefined) defaults.timeout = o.timeout;
}

const customMatchers: Record<string, Custom> = {};

const stringify = (v: unknown) => {
  if (typeof v === 'string') return JSON.stringify(v);
  if (v instanceof RegExp) return String(v);
  try {
    return JSON.stringify(v) ?? String(v);
  } catch {
    return String(v);
  }
};
const utils = { printReceived: stringify, printExpected: stringify, stringify };

/** Playwright's `serializeExpectedTextValues`. */
function texts(values: (string | RegExp)[], o: { matchSubstring?: boolean; normalizeWhiteSpace?: boolean; ignoreCase?: boolean } = {}): ExpectedText[] {
  return values.map((v) =>
    typeof v === 'string'
      ? { string: v, matchSubstring: o.matchSubstring, normalizeWhiteSpace: o.normalizeWhiteSpace, ignoreCase: o.ignoreCase }
      : { regexSource: v.source, regexFlags: v.flags, matchSubstring: o.matchSubstring, normalizeWhiteSpace: o.normalizeWhiteSpace, ignoreCase: o.ignoreCase },
  );
}

class ExpectError extends Error {
  override name = 'Error';
  constructor(message: string, readonly matcherResult?: unknown) {
    super(message);
  }
}

/** Playwright's failure message: what was expected of which locator, and what it saw. */
function failure(matcher: string, isNot: boolean, subject: string, expected: string, received: unknown, r: ExpectResult, timeout: number, message?: string): ExpectError {
  const lines = [
    `${message ? message + '\n\n' : ''}expect(${subject.startsWith('page') ? 'page' : 'locator'})${isNot ? '.not' : ''}.${matcher}(${expected === '' ? '' : 'expected'}) failed`,
    '',
    subject.startsWith('page') ? '' : `Locator: ${subject}`,
    expected === '' ? '' : `Expected${isNot ? ': not' : ':'} ${expected}`,
    r.errorMessage ? `Error: ${r.errorMessage}` : `Received: ${typeof received === 'string' ? received : stringify(received)}`,
    r.timedOut ? `Timeout: ${timeout}ms` : '',
    r.log?.length ? `\nCall log:\n${r.log.map((l) => `  - ${l}`).join('\n')}` : '',
  ].filter((l, i) => l !== '' || i === 1);
  return new ExpectError(lines.join('\n'));
}

type Opts = { timeout?: number; [k: string]: any };

/** A locator matcher: one `_expect` expression, retried until it holds or times out. */
async function locatorMatcher(
  loc: Locator,
  isNot: boolean,
  cfg: ExpectConfig,
  matcher: string,
  expression: string,
  o: Omit<ExpectOptions, 'isNot' | 'timeout'>,
  timeout: number | undefined,
  shown: string,
): Promise<void> {
  const t = timeout ?? cfg.timeout ?? defaults.timeout;
  const r = await expectLocator(loc, expression, { ...o, isNot, timeout: t });
  if (r.matches !== isNot && !r.errorMessage) return;
  throw failure(matcher, isNot, loc._desc, shown, r.received, r, t, cfg.message);
}

/** Matchers of a locator, a page or an API response; `undefined` for others. */
function webFirst(actual: unknown, isNot: boolean, cfg: ExpectConfig): Record<string, (...a: any[]) => Promise<void>> | undefined {
  const t = (o?: Opts) => o?.timeout;
  if (actual instanceof Locator) {
    const m = (matcher: string, expression: string, o: Omit<ExpectOptions, 'isNot' | 'timeout'>, timeout: number | undefined, shown = '') =>
      locatorMatcher(actual, isNot, cfg, matcher, expression, o, timeout, shown);
    return {
      toBeAttached: (o?: Opts & { attached?: boolean }) => m('toBeAttached', o?.attached === false ? 'to.be.detached' : 'to.be.attached', {}, t(o), 'attached'),
      toBeChecked: (o?: Opts & { checked?: boolean; indeterminate?: boolean }) =>
        m('toBeChecked', 'to.be.checked', { expectedValue: o?.indeterminate ? { indeterminate: true } : { checked: o?.checked ?? true } }, t(o), o?.indeterminate ? 'indeterminate' : o?.checked === false ? 'unchecked' : 'checked'),
      toBeDisabled: (o?: Opts) => m('toBeDisabled', 'to.be.disabled', {}, t(o), 'disabled'),
      toBeEditable: (o?: Opts & { editable?: boolean }) => m('toBeEditable', o?.editable === false ? 'to.be.readonly' : 'to.be.editable', {}, t(o), 'editable'),
      toBeEmpty: (o?: Opts) => m('toBeEmpty', 'to.be.empty', {}, t(o), 'empty'),
      toBeEnabled: (o?: Opts & { enabled?: boolean }) => m('toBeEnabled', o?.enabled === false ? 'to.be.disabled' : 'to.be.enabled', {}, t(o), 'enabled'),
      toBeFocused: (o?: Opts) => m('toBeFocused', 'to.be.focused', {}, t(o), 'focused'),
      toBeHidden: (o?: Opts) => m('toBeHidden', 'to.be.hidden', {}, t(o), 'hidden'),
      toBeInViewport: (o?: Opts & { ratio?: number }) => m('toBeInViewport', 'to.be.in.viewport', { expectedNumber: o?.ratio }, t(o), 'in viewport'),
      toBeVisible: (o?: Opts & { visible?: boolean }) => m('toBeVisible', o?.visible === false ? 'to.be.hidden' : 'to.be.visible', {}, t(o), 'visible'),
      toContainText: (expected: string | RegExp | (string | RegExp)[], o?: Opts & { ignoreCase?: boolean; useInnerText?: boolean }) =>
        Array.isArray(expected)
          ? m('toContainText', 'to.contain.text.array', { expectedText: texts(expected, { matchSubstring: true, normalizeWhiteSpace: true, ignoreCase: o?.ignoreCase }), useInnerText: o?.useInnerText }, t(o), stringify(expected))
          : m('toContainText', 'to.have.text', { expectedText: texts([expected], { matchSubstring: true, normalizeWhiteSpace: true, ignoreCase: o?.ignoreCase }), useInnerText: o?.useInnerText }, t(o), stringify(expected)),
      toContainClass: (expected: string | string[], o?: Opts) =>
        Array.isArray(expected)
          ? m('toContainClass', 'to.contain.class.array', { expectedText: texts(expected) }, t(o), stringify(expected))
          : m('toContainClass', 'to.contain.class', { expectedText: texts([expected]) }, t(o), stringify(expected)),
      toHaveAccessibleDescription: (expected: string | RegExp, o?: Opts & { ignoreCase?: boolean }) =>
        m('toHaveAccessibleDescription', 'to.have.accessible.description', { expectedText: texts([expected], { ignoreCase: o?.ignoreCase, normalizeWhiteSpace: true }) }, t(o), stringify(expected)),
      toHaveAccessibleName: (expected: string | RegExp, o?: Opts & { ignoreCase?: boolean }) =>
        m('toHaveAccessibleName', 'to.have.accessible.name', { expectedText: texts([expected], { ignoreCase: o?.ignoreCase, normalizeWhiteSpace: true }) }, t(o), stringify(expected)),
      toHaveAttribute: (name: string, value?: string | RegExp | Opts, o?: Opts & { ignoreCase?: boolean }) => {
        if (value === undefined || (typeof value === 'object' && !(value instanceof RegExp))) {
          return m('toHaveAttribute', 'to.have.attribute', { expressionArg: name }, (value as Opts | undefined)?.timeout, `attribute ${stringify(name)}`);
        }
        return m('toHaveAttribute', 'to.have.attribute.value', { expressionArg: name, expectedText: texts([value as string | RegExp], { ignoreCase: o?.ignoreCase }) }, t(o), stringify(value));
      },
      toHaveClass: (expected: string | RegExp | (string | RegExp)[], o?: Opts) =>
        Array.isArray(expected)
          ? m('toHaveClass', 'to.have.class.array', { expectedText: texts(expected) }, t(o), stringify(expected))
          : m('toHaveClass', 'to.have.class', { expectedText: texts([expected]) }, t(o), stringify(expected)),
      toMatchAriaSnapshot: (expected?: string | (Opts & { name?: string }), o?: Opts) => ariaMatcher(actual, isNot, cfg, expected, o),
      toHaveCount: (n: number, o?: Opts) => m('toHaveCount', 'to.have.count', { expectedNumber: n }, t(o), String(n)),
      toHaveCSS: (name: string, value: string | RegExp, o?: Opts) => m('toHaveCSS', 'to.have.css', { expressionArg: name, expectedText: texts([value]) }, t(o), stringify(value)),
      toHaveId: (id: string | RegExp, o?: Opts) => m('toHaveId', 'to.have.id', { expectedText: texts([id]) }, t(o), stringify(id)),
      toHaveJSProperty: (name: string, value: unknown, o?: Opts) => m('toHaveJSProperty', 'to.have.property', { expressionArg: name, expectedValue: value }, t(o), stringify(value)),
      toHaveRole: (role: string, o?: Opts) => m('toHaveRole', 'to.have.role', { expectedText: texts([role]) }, t(o), stringify(role)),
      toHaveText: (expected: string | RegExp | (string | RegExp)[], o?: Opts & { ignoreCase?: boolean; useInnerText?: boolean }) =>
        Array.isArray(expected)
          ? m('toHaveText', 'to.have.text.array', { expectedText: texts(expected, { normalizeWhiteSpace: true, ignoreCase: o?.ignoreCase }), useInnerText: o?.useInnerText }, t(o), stringify(expected))
          : m('toHaveText', 'to.have.text', { expectedText: texts([expected], { normalizeWhiteSpace: true, ignoreCase: o?.ignoreCase }), useInnerText: o?.useInnerText }, t(o), stringify(expected)),
      toHaveValue: (value: string | RegExp, o?: Opts) => m('toHaveValue', 'to.have.value', { expectedText: texts([value]) }, t(o), stringify(value)),
      toHaveValues: (values: (string | RegExp)[], o?: Opts) => m('toHaveValues', 'to.have.values', { expectedText: texts(values) }, t(o), stringify(values)),
      toHaveScreenshot: (nameOrOpts?: string | string[] | (ScreenshotOptions & Opts), o?: ScreenshotOptions & Opts) => screenshot(actual._page, actual, isNot, nameOrOpts, o),
    };
  }
  if (actual instanceof Page) {
    return {
      toHaveTitle: async (expected: string | RegExp, o?: Opts) => {
        const timeout = t(o) ?? cfg.timeout ?? defaults.timeout;
        const r = await actual.mainFrame()._expect('to.have.title', { expectedText: texts([expected], { normalizeWhiteSpace: true }), isNot, timeout });
        if (r.matches !== isNot) return;
        throw failure('toHaveTitle', isNot, 'page', stringify(expected), r.received, r, timeout, cfg.message);
      },
      toHaveURL: async (expected: string | RegExp | ((url: URL) => boolean), o?: Opts & { ignoreCase?: boolean }) => {
        const timeout = t(o) ?? cfg.timeout ?? defaults.timeout;
        if (typeof expected === 'function') {
          const deadline = Date.now() + limit(timeout);
          for (;;) {
            if ((await urlMatches(undefined, actual.url(), expected)) !== isNot) return;
            if (Date.now() > deadline) throw failure('toHaveURL', isNot, 'page', 'predicate to succeed', actual.url(), { matches: isNot, timedOut: true }, timeout, cfg.message);
            await actual._nextNavigation(Math.max(1, Math.min(100, deadline - Date.now())));
          }
        }
        const want = typeof expected === 'string' ? resolveUrl(actual.context()._options.baseURL, expected) : expected;
        const r = await actual.mainFrame()._expect('to.have.url', { expectedText: texts([want], { ignoreCase: o?.ignoreCase }), isNot, timeout });
        if (r.matches !== isNot) return;
        throw failure('toHaveURL', isNot, 'page', stringify(want), r.received, r, timeout, cfg.message);
      },
      toHaveScreenshot: (nameOrOpts?: string | string[] | (ScreenshotOptions & Opts), o?: ScreenshotOptions & Opts) => screenshot(actual, undefined, isNot, nameOrOpts, o),
    };
  }
  if (actual instanceof Frame) return undefined;
  if (actual instanceof APIResponse) {
    return {
      toBeOK: async () => {
        if (actual.ok() !== isNot) return;
        const body = await actual.text().catch(() => '');
        throw new ExpectError(`${cfg.message ? cfg.message + '\n\n' : ''}expect(response)${isNot ? '.not' : ''}.toBeOK()\n\n${isNot ? 'Expected status code not' : 'Expected status code'} to be within [200..299]\nReceived: ${actual.status()}\n\nResponse text:\n${body.slice(0, 1000)}`);
      },
    };
  }
  return undefined;
}

/**
 * `toMatchAriaSnapshot(template)` or, with no template, against
 * `<name>.aria.yml`, as Playwright: the template (blank lines dropped,
 * dedented, the project's `expect.toMatchAriaSnapshot.children` prepended)
 * is polled until it matches or the timeout ends. A missing file is written
 * in "missing" mode; "all" always rewrites it and "changed" only when it
 * still fails, both with the regex (codegen) rendering, never under `.not`.
 */
async function ariaMatcher(loc: Locator, isNot: boolean, cfg: ExpectConfig, expected?: string | (Opts & { name?: string }), o: Opts = {}): Promise<void> {
  const opts = typeof expected === 'object' ? expected : o;
  const timeout = opts.timeout ?? cfg.timeout ?? defaults.timeout;
  const label = `expect(locator)${isNot ? '.not' : ''}.toMatchAriaSnapshot()`;
  const info = currentTest();
  let file: string | undefined;
  let text: string;
  let mode: UpdateMode | undefined;
  let missing = false;
  const write = async (tree?: AriaNode) => {
    const regex = renderAria(tree ?? (await loc._ariaTree(timeout)), undefined, true);
    mkdirSync(dirname(file!), { recursive: true });
    writeFileSync(file!, regex);
  };
  if (typeof expected === 'string') text = expected;
  else {
    if (!info) throw new Error('toMatchAriaSnapshot() must be called during the test');
    file = ariaSnapshotFile(info, opts.name);
    mode = updateSnapshots(info);
    // As Playwright (`!expected`): an empty file is a missing snapshot.
    missing = !existsSync(file) || readFileSync(file, 'utf8') === '';
    if (missing && mode === 'missing') {
      if (isNot) throw new ExpectError(`${label} failed\n\nMatchers using ".not" can't generate new baselines`);
      await write();
      missingWritten(info, `A snapshot doesn't exist at ${relative(process.cwd(), file)}, writing actual.`);
      return;
    }
    if (missing && mode === 'none') throw new ExpectError(`A snapshot doesn't exist at ${relative(process.cwd(), file)}.`);
    text = missing ? '' : readFileSync(file, 'utf8');
  }
  text = dedent(text);
  const children = ((info?.project as { expect?: Record<string, any> } | undefined)?.expect?.toMatchAriaSnapshot as { children?: string } | undefined)?.children;
  if (children && !/^- \/children:/m.test(text)) text = `- /children: ${children}\n` + text;
  // A missing file under "all" / "changed" has no template: nothing matches.
  let template: AriaTemplate | undefined;
  if (!missing) {
    try {
      template = parseAriaTemplate(text);
    } catch (e) {
      throw new ExpectError(`${label}: the template is not valid: ${(e as Error).message}`);
    }
  }
  const deadline = Date.now() + limit(timeout);
  let tree: AriaNode | undefined;
  let pass = false;
  for (let i = 0; ; i++) {
    tree = await loc._ariaTree(Math.max(1, deadline - Date.now())).catch((e) => {
      if (Date.now() >= deadline) throw e;
      return undefined;
    });
    pass = !!tree && !!template && matchesAria(tree, template);
    if (tree && (pass !== isNot || missing)) break;
    if (Date.now() >= deadline) break;
    await new Promise((r) => setTimeout(r, [100, 250, 500, 1000][Math.min(i, 3)]));
  }
  if (file && !isNot && (mode === 'all' || (mode === 'changed' && !pass))) {
    await write(tree);
    console.log(`A snapshot is generated at ${relative(process.cwd(), file)}.`);
    return;
  }
  if (pass !== isNot) return;
  const received = tree ? renderAria(tree) : '';
  throw new ExpectError(
    `${cfg.message ? cfg.message + '\n\n' : ''}${label} failed\n\nLocator: ${loc._desc}${file ? `\nSnapshot: ${relative(process.cwd(), file)}` : ''}\nTimeout: ${timeout}ms\n\n${isNot ? `Expected not to match:\n${text}` : `- Expected\n+ Received\n\n${lineDiff(text.trim(), received)}`}`,
  );
}

/** Playwright's `-actual`/`-expected`/`-diff` stem: the snapshot name (sanitized when a string) in the test's output dir. */
const namedCalls = new WeakMap<object, Map<string, number>>();
function outputStem(info: NonNullable<ReturnType<typeof currentTest>>, name: string | string[]): string {
  let sub = Array.isArray(name) ? join(...name) : sanitize(trimLongString(name, 60).slice(0, -'.png'.length)) + '.png';
  // A name's second and later calls in a test get `-1`, `-2`, ... (testInfo._resolveSnapshotPaths).
  const calls = namedCalls.get(info) ?? new Map<string, number>();
  namedCalls.set(info, calls);
  const n = (calls.get(sub) ?? 0) + 1;
  calls.set(sub, n);
  sub = sub.slice(0, -'.png'.length) + (n > 1 ? `-${n - 1}` : '');
  return info.outputPath(sub);
}

let shots = 0;
/**
 * `toHaveScreenshot`: rendered and compared in the browser, against the
 * baseline at Playwright's screenshot snapshot path
 * (`testInfo.snapshotPath(name, { kind: 'screenshot' })`); the engine's
 * display-list hash (`<name>.json`) lives beside it. A name without `.png`
 * gets it (Playwright requires it).
 */
async function screenshot(page: Page, loc: Locator | undefined, isNot: boolean, nameOrOpts?: string | string[] | (ScreenshotOptions & Opts), o: ScreenshotOptions & Opts = {}): Promise<void> {
  if (isNot) throw new Error('pyro: .not.toHaveScreenshot is not supported');
  const info = currentTest();
  const opts = typeof nameOrOpts === 'object' && !Array.isArray(nameOrOpts) ? nameOrOpts : o;
  const named: string | string[] | undefined = typeof nameOrOpts === 'string' || Array.isArray(nameOrOpts) ? nameOrOpts : (opts as { name?: string | string[] }).name;
  // `omitBackground` is not a step option: as Playwright on Chromium, the page's default background is made
  // transparent for this capture and reset afterwards.
  const { timeout: _t, name: _n, omitBackground, ...shotOpts } = opts as Opts;
  const locator = loc ? { locator: loc._spec } : {};
  let step: ScreenshotStep;
  let file: string | undefined;
  let mode: UpdateMode | undefined;
  if (info) {
    const png = (n: string) => {
      if (!n.endsWith('.png')) throw new ExpectError(`Screenshot name "${n}" must have '.png' extension`);
      return n;
    };
    const name = named === undefined || named.length === 0 ? anonymousName(info, '.png', 'screenshot') : typeof named === 'string' ? png(named) : [...named.slice(0, -1), png(named[named.length - 1])];
    file = snapshotPath(info, name, 'screenshot');
    const configured = ((info.project as { expect?: Record<string, any> }).expect?.toHaveScreenshot ?? {}) as Record<string, unknown>;
    const defaults = Object.fromEntries(['threshold', 'maxDiffPixels', 'maxDiffPixelRatio'].filter((k) => configured[k] !== undefined).map((k) => [k, configured[k]]));
    step = new ScreenshotStep(basename(file).slice(0, -'.png'.length), { ...defaults, ...(shotOpts as ScreenshotOptions), dir: dirname(file), ...locator }, dirname(info.file), false, outputStem(info, name));
    mode = updateSnapshots(info);
    if (mode === 'none' && !existsSync(file)) throw new ExpectError(`A snapshot doesn't exist at ${relative(process.cwd(), file)}.`);
  } else {
    const raw = typeof named === 'string' ? named : Array.isArray(named) ? named.join('-') : `screenshot-${++shots}`;
    step = new ScreenshotStep(raw.replace(/\.png$/, ''), { ...(shotOpts as ScreenshotOptions), ...locator }, process.cwd());
  }
  const existed = file !== undefined && existsSync(file);
  prepare(step, page._baselines, mode);
  if (omitBackground) await page._send('Emulation.setDefaultBackgroundColorOverride', { color: { r: 0, g: 0, b: 0, a: 0 } });
  try {
    const r = await page._runRaw([step.toStep() as any]).finally(() =>
      omitBackground ? page._send('Emulation.setDefaultBackgroundColorOverride', {}).catch(() => {}) : undefined,
    );
    const result = settle(step, r.results[0], mode);
    // As Playwright's "missing" mode: written, the test fails at its end and is not retried.
    if (info && file && result.recorded && !existed && mode === 'missing') missingWritten(info, `A snapshot doesn't exist at ${relative(process.cwd(), file)}, writing actual.`);
  } catch (e) {
    if (e instanceof CdpError && (e.data as any)?.diff) {
      // "changed" (and "all"): a mismatching baseline is rewritten and the assertion passes.
      if (mode === 'changed' || mode === 'all') {
        rewrite(step, e.data);
        return;
      }
      throw mismatch(step, e.data, e.message);
    }
    page._baselines.delete(String(step.wire.name ?? ''));
    throw e;
  }
}

/** The matchers object `expect(actual)` returns (`.not` flips it). */
function matchers(actual: unknown, cfg: ExpectConfig, isNot: boolean): any {
  const own = webFirst(actual, isNot, cfg);
  const wrap = (fn: (...a: any[]) => Promise<void> | void) => async (...args: any[]) => {
    try {
      await fn(...args);
    } catch (e) {
      if (!cfg.soft) throw e;
      const info = currentTest();
      if (!info) throw e;
      info._softErrors.push(e as Error);
    }
  };
  return new Proxy(
    {},
    {
      get(_t, prop) {
        if (prop === 'not') return matchers(actual, cfg, !isNot);
        if (prop === 'then') return undefined;
        if (typeof prop !== 'string') return undefined;
        if (prop === 'resolves' || prop === 'rejects') {
          return new Proxy({}, { get: (_x, m) => async (...args: any[]) => {
            const j = await jest();
            const base = isNot ? j(actual)[prop].not : j(actual)[prop];
            return base[m as string](...args);
          } });
        }
        if (own && prop in own) return wrap(own[prop]);
        // Synchronous, as in Playwright (it is not awaited).
        if (prop === 'toMatchSnapshot') {
          return (nameOrOpts?: string | string[] | (SnapshotOptions & { name?: string | string[] }), o?: SnapshotOptions) => {
            try {
              toMatchSnapshot(actual, isNot, cfg, nameOrOpts, o);
            } catch (e) {
              const info = currentTest();
              if (!cfg.soft || !info) throw e;
              info._softErrors.push(e as Error);
            }
          };
        }
        if (prop === 'toPass') return wrap((o?: { timeout?: number; intervals?: number[] }) => toPass(actual as () => unknown, isNot, cfg, o));
        if (customMatchers[prop]) {
          return wrap(async (...args: any[]) => {
            const state: MatcherState = { isNot, promise: '', timeout: cfg.timeout ?? defaults.timeout, utils };
            const r = await customMatchers[prop].call(state, actual, ...args);
            if (r.pass === isNot) throw new ExpectError(`${cfg.message ? cfg.message + '\n\n' : ''}${r.message()}`);
          });
        }
        // Not web-first: Bun's (Jest's) matcher, synchronous as in Playwright.
        return (...args: any[]) => {
          if (!bunExpect) throw new Error('pyro: expect runs under `bun test`');
          const call = () => {
            const j = isNot ? bunExpect!(actual).not : bunExpect!(actual);
            if (typeof j[prop] !== 'function') throw new Error(`expect: unknown matcher ${prop}`);
            return j[prop](...args);
          };
          try {
            return call();
          } catch (e) {
            if (cfg.message) (e as Error).message = `${cfg.message}\n\n${(e as Error).message}`;
            if (!cfg.soft) throw e;
            const info = currentTest();
            if (!info) throw e;
            info._softErrors.push(e as Error);
          }
        };
      },
    },
  );
}

/** `expect(value).toMatchSnapshot(name?, options?)` on a string or Buffer. */
function toMatchSnapshot(actual: unknown, isNot: boolean, cfg: ExpectConfig, nameOrOpts?: string | string[] | (SnapshotOptions & { name?: string | string[] }), o: SnapshotOptions = {}): void {
  const info = currentTest();
  if (!info) throw new Error('toMatchSnapshot() must be called during the test');
  if (typeof actual !== 'string' && !Buffer.isBuffer(actual)) throw new ExpectError(`toMatchSnapshot: received value must be a string or a Buffer, got ${typeof actual}`);
  const { name: _n, ...opts } = typeof nameOrOpts === 'object' && !Array.isArray(nameOrOpts) ? nameOrOpts : (o as SnapshotOptions & { name?: string });
  const buf = Buffer.isBuffer(actual) ? actual : Buffer.from(actual);
  // As Playwright: the anonymous extension from the magic bytes.
  const magic = (bytes: number[]) => buf.length >= bytes.length && bytes.every((b, i) => buf[i] === b);
  const ext = typeof actual === 'string' ? '.txt' : magic([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) ? '.png' : magic([0xff, 0xd8, 0xff]) ? '.jpg' : '.dat';
  const named = typeof nameOrOpts === 'object' && !Array.isArray(nameOrOpts) ? nameOrOpts.name : nameOrOpts;
  const parts = typeof named === 'string' && named ? named : Array.isArray(named) && named.length ? named : anonymousName(info, ext, 'snapshot');
  const defaults = ((info.project as { expect?: Record<string, any> }).expect?.toMatchSnapshot ?? {}) as SnapshotOptions;
  const why = matchFile(info, snapshotPath(info, parts), buf, { ...defaults, ...opts }, isNot, `expect(${typeof actual === 'string' ? 'string' : 'Buffer'})${isNot ? '.not' : ''}.toMatchSnapshot()`);
  if (why) throw new ExpectError(`${cfg.message ? cfg.message + '\n\n' : ''}${why}`);
}

async function toPass(fn: () => unknown, isNot: boolean, cfg: ExpectConfig, o: { timeout?: number; intervals?: number[] } = {}): Promise<void> {
  if (isNot) throw new Error('expect(fn).not.toPass() is not supported');
  const timeout = o.timeout ?? 0;
  const intervals = o.intervals ?? [100, 250, 500, 1000];
  const deadline = timeout > 0 ? Date.now() + timeout : Infinity;
  let last: unknown;
  for (let i = 0; ; i++) {
    try {
      await fn();
      return;
    } catch (e) {
      last = e;
    }
    const wait = intervals[Math.min(i, intervals.length - 1)];
    if (Date.now() + wait > deadline) break;
    await new Promise((r) => setTimeout(r, wait));
  }
  throw new ExpectError(`${cfg.message ? cfg.message + '\n\n' : ''}expect(function).toPass() failed\n\nTimeout ${timeout}ms exceeded while waiting on the predicate\n\n${(last as Error)?.message ?? last}`);
}

/** `expect.poll(fn)`: the matcher retried on `fn()`'s value until it holds. */
function poll(fn: () => unknown, cfg: ExpectConfig, o: { timeout?: number; intervals?: number[]; message?: string } = {}, isNot = false): any {
  const timeout = o.timeout ?? cfg.timeout ?? defaults.timeout;
  const intervals = o.intervals ?? [100, 250, 500, 1000];
  return new Proxy(
    {},
    {
      get(_t, prop) {
        if (prop === 'not') return poll(fn, cfg, o, !isNot);
        if (prop === 'then' || typeof prop !== 'string') return undefined;
        return async (...args: any[]) => {
          // 0: no limit, as in Playwright.
          const deadline = Date.now() + limit(timeout);
          let last: unknown;
          for (let i = 0; ; i++) {
            try {
              const value = await fn();
              // One check per poll (0 would be no limit).
              const m = matchers(value, { ...cfg, soft: false, timeout: 1, message: undefined }, isNot);
              await m[prop](...args);
              return;
            } catch (e) {
              last = e;
            }
            const wait = intervals[Math.min(i, intervals.length - 1)];
            if (Date.now() + wait > deadline) break;
            await new Promise((r) => setTimeout(r, wait));
          }
          const message = o.message ?? cfg.message;
          const err = new ExpectError(`${message ? message + '\n\n' : ''}expect.poll(...)${isNot ? '.not' : ''}.${prop}() failed: Timeout ${timeout}ms exceeded while waiting on the predicate\n\n${(last as Error)?.message ?? last}`);
          if (!cfg.soft) throw err;
          const info = currentTest();
          if (!info) throw err;
          info._softErrors.push(err);
        };
      },
    },
  );
}

export type { Expect } from './expect-types.js';
/** The implementation's view of `expect` (the public type is `Expect`). */
type AnyExpect = any;

function make(cfg: ExpectConfig): AnyExpect {
  const message = (m?: string | { message?: string }) => (typeof m === 'string' ? m : m?.message);
  const fn = ((actual: unknown, m?: string | { message?: string }) => matchers(actual, { ...cfg, message: message(m) ?? cfg.message }, false)) as AnyExpect;
  fn.soft = (actual: unknown, m?: string | { message?: string }) => matchers(actual, { ...cfg, soft: true, message: message(m) ?? cfg.message }, false);
  fn.poll = (getter: () => unknown, o?: string | { timeout?: number; intervals?: number[]; message?: string }) => poll(getter, cfg, typeof o === 'string' ? { message: o } : o);
  fn.configure = (o: ExpectConfig) => make({ ...cfg, ...o });
  fn.extend = (m: Record<string, Custom>) => {
    Object.assign(customMatchers, m);
    return fn;
  };
  for (const name of ['any', 'anything', 'arrayContaining', 'objectContaining', 'stringContaining', 'stringMatching', 'closeTo', 'not', 'assertions', 'hasAssertions', 'getState', 'addSnapshotSerializer']) {
    Object.defineProperty(fn, name, {
      get: () => {
        if (!bunExpect) throw new Error('pyro: expect runs under `bun test`');
        return (bunExpect as any)[name];
      },
    });
  }
  return fn;
}

export const expect: import('./expect-types.js').Expect = make({});
/** Loads Bun's expect (the runner awaits it before tests run). */
export const ready = (globalThis as any).Bun ? jest().then(() => undefined) : Promise.resolve();
if ((globalThis as any).Bun) bunExpect = require('bun:test').expect as unknown as BunExpect;
