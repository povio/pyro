// `expect`'s types, as `@playwright/test` has them: the matchers depend on
// what is passed (a Locator, a Page, an APIResponse, a function, a value),
// web-first ones return promises, and `expect.extend` adds typed matchers.
import type { ScreenshotOptions } from '../steps.js';
import type { Locator, Page } from './page.js';
import type { APIResponse } from './request.js';

type Timeout = { timeout?: number };
type TextOptions = Timeout & { ignoreCase?: boolean; useInnerText?: boolean };
type ScreenshotAssertionOptions = ScreenshotOptions & { animations?: 'disabled' | 'allow'; caret?: 'hide' | 'initial'; mask?: Locator[]; omitBackground?: boolean; scale?: 'css' | 'device' };

export interface LocatorAssertions {
  toBeAttached(options?: Timeout & { attached?: boolean }): Promise<void>;
  toBeChecked(options?: Timeout & { checked?: boolean; indeterminate?: boolean }): Promise<void>;
  toBeDisabled(options?: Timeout): Promise<void>;
  toBeEditable(options?: Timeout & { editable?: boolean }): Promise<void>;
  toBeEmpty(options?: Timeout): Promise<void>;
  toBeEnabled(options?: Timeout & { enabled?: boolean }): Promise<void>;
  toBeFocused(options?: Timeout): Promise<void>;
  toBeHidden(options?: Timeout): Promise<void>;
  toBeInViewport(options?: Timeout & { ratio?: number }): Promise<void>;
  toBeVisible(options?: Timeout & { visible?: boolean }): Promise<void>;
  toContainClass(expected: string | ReadonlyArray<string>, options?: Timeout): Promise<void>;
  toContainText(expected: string | RegExp | ReadonlyArray<string | RegExp>, options?: TextOptions): Promise<void>;
  toHaveAccessibleDescription(description: string | RegExp, options?: Timeout & { ignoreCase?: boolean }): Promise<void>;
  toHaveAccessibleName(name: string | RegExp, options?: Timeout & { ignoreCase?: boolean }): Promise<void>;
  toHaveAttribute(name: string, value: string | RegExp, options?: Timeout & { ignoreCase?: boolean }): Promise<void>;
  toHaveAttribute(name: string, options?: Timeout): Promise<void>;
  toHaveClass(expected: string | RegExp | ReadonlyArray<string | RegExp>, options?: Timeout): Promise<void>;
  toHaveCount(count: number, options?: Timeout): Promise<void>;
  toHaveCSS(name: string, value: string | RegExp, options?: Timeout): Promise<void>;
  toHaveId(id: string | RegExp, options?: Timeout): Promise<void>;
  toHaveJSProperty(name: string, value: unknown, options?: Timeout): Promise<void>;
  toHaveRole(role: string, options?: Timeout): Promise<void>;
  toHaveScreenshot(name: string | ReadonlyArray<string>, options?: ScreenshotAssertionOptions): Promise<void>;
  toHaveScreenshot(options?: ScreenshotAssertionOptions): Promise<void>;
  toHaveText(expected: string | RegExp | ReadonlyArray<string | RegExp>, options?: TextOptions): Promise<void>;
  toHaveValue(value: string | RegExp, options?: Timeout): Promise<void>;
  toHaveValues(values: ReadonlyArray<string | RegExp>, options?: Timeout): Promise<void>;
  not: LocatorAssertions;
}

export interface PageAssertions {
  toHaveScreenshot(name: string | ReadonlyArray<string>, options?: ScreenshotAssertionOptions & { fullPage?: boolean }): Promise<void>;
  toHaveScreenshot(options?: ScreenshotAssertionOptions & { fullPage?: boolean }): Promise<void>;
  toHaveTitle(titleOrRegExp: string | RegExp, options?: Timeout): Promise<void>;
  toHaveURL(url: string | RegExp | ((url: URL) => boolean), options?: Timeout & { ignoreCase?: boolean }): Promise<void>;
  not: PageAssertions;
}

export interface APIResponseAssertions {
  toBeOK(): Promise<void>;
  not: APIResponseAssertions;
}

export interface FunctionAssertions {
  toPass(options?: { timeout?: number; intervals?: number[] }): Promise<void>;
}

/** Bun's (Jest's) value matchers, which Playwright's follow. */
export interface GenericMatchers<R> {
  toBe(expected: unknown): R;
  toBeCloseTo(expected: number, numDigits?: number): R;
  toBeDefined(): R;
  toBeFalsy(): R;
  toBeGreaterThan(expected: number | bigint): R;
  toBeGreaterThanOrEqual(expected: number | bigint): R;
  toBeInstanceOf(expected: Function): R;
  toBeLessThan(expected: number | bigint): R;
  toBeLessThanOrEqual(expected: number | bigint): R;
  toBeNaN(): R;
  toBeNull(): R;
  toBeTruthy(): R;
  toBeUndefined(): R;
  toContain(expected: unknown): R;
  toContainEqual(expected: unknown): R;
  toEqual(expected: unknown): R;
  toHaveLength(expected: number): R;
  toHaveProperty(keyPath: string | ReadonlyArray<string>, value?: unknown): R;
  toMatch(expected: RegExp | string): R;
  toMatchObject(expected: Record<string, unknown> | ReadonlyArray<unknown>): R;
  toStrictEqual(expected: unknown): R;
  toThrow(error?: unknown): R;
  toThrowError(error?: unknown): R;
}

/** What a matcher added with `expect.extend` returns. */
export type MatcherReturnType = { message: () => string; pass: boolean; name?: string; expected?: unknown; actual?: unknown; log?: string[]; timeout?: number };
export type ExpectMatcherState = {
  isNot: boolean;
  promise: 'rejects' | 'resolves' | '';
  timeout: number;
  utils: { printReceived(value: unknown): string; printExpected(value: unknown): string; stringify(value: unknown): string };
};
type CustomMatcher = (this: ExpectMatcherState, receiver: any, ...args: any[]) => MatcherReturnType | Promise<MatcherReturnType>;
type Rest<F> = F extends (receiver: any, ...args: infer A) => any ? A : never;
type Extended<E, R> = { [K in keyof E]: E[K] extends CustomMatcher ? (...args: Rest<E[K]>) => R : never };

type Specific<T> = T extends Locator
  ? LocatorAssertions
  : T extends Page
    ? PageAssertions
    : T extends APIResponse
      ? APIResponseAssertions
      : T extends Function
        ? FunctionAssertions
        : {};

export type Matchers<R, T, E = {}> = GenericMatchers<R> &
  Extended<E, R> & {
    not: GenericMatchers<R> & Extended<E, R>;
    resolves: GenericMatchers<Promise<R>> & Extended<E, Promise<R>> & { not: GenericMatchers<Promise<R>> & Extended<E, Promise<R>> };
    rejects: GenericMatchers<Promise<R>> & Extended<E, Promise<R>> & { not: GenericMatchers<Promise<R>> & Extended<E, Promise<R>> };
  };
export type MakeMatchers<T, E = {}> = Omit<Matchers<void, T, E>, keyof Specific<T>> & Specific<T>;

type PollMatchers<T, E> = GenericMatchers<Promise<void>> & Extended<E, Promise<void>> & { not: GenericMatchers<Promise<void>> & Extended<E, Promise<void>> };

type AsymmetricMatcher = any;
interface AsymmetricMatchers {
  any(sample: unknown): AsymmetricMatcher;
  anything(): AsymmetricMatcher;
  arrayContaining(sample: ReadonlyArray<unknown>): AsymmetricMatcher;
  closeTo(sample: number, precision?: number): AsymmetricMatcher;
  objectContaining(sample: Record<string, unknown>): AsymmetricMatcher;
  stringContaining(sample: string): AsymmetricMatcher;
  stringMatching(sample: string | RegExp): AsymmetricMatcher;
}

export type Expect<E = {}> = {
  <T = unknown>(actual: T, messageOrOptions?: string | { message?: string }): MakeMatchers<T, E>;
  soft: <T = unknown>(actual: T, messageOrOptions?: string | { message?: string }) => MakeMatchers<T, E>;
  poll: <T = unknown>(actual: () => T | Promise<T>, messageOrOptions?: string | { message?: string; timeout?: number; intervals?: number[] }) => PollMatchers<T, E>;
  extend<More extends Record<string, CustomMatcher>>(matchers: More): Expect<E & More>;
  configure: (configuration: { message?: string; timeout?: number; soft?: boolean }) => Expect<E>;
  not: Omit<AsymmetricMatchers, 'any' | 'anything'>;
} & AsymmetricMatchers;
