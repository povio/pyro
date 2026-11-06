// `@povio/lightning-panda/playwright`: Playwright's API on lightningpanda, in
// place of both `playwright` and `@playwright/test`:
//
//   import { test, expect, type Page } from '@povio/lightning-panda/playwright';
//
// The library (`chromium`, Browser, BrowserContext, Page, Locator, …), the
// test runner (`test`, fixtures, hooks, config; tests run with `bun test`),
// `expect`, `defineConfig`, `devices` and `request`. None of it uses
// Playwright.
import { TimeoutError } from './page.js';

export { chromium, Browser, BrowserContext, type BootSpec, type BrowserContextOptions } from './browser.js';
export {
  Page,
  Frame,
  Locator,
  FrameLocator,
  Request,
  Response,
  Route,
  Dialog,
  ConsoleMessage,
  Keyboard,
  Mouse,
  Worker,
  FileChooser,
  WebSocket,
  TimeoutError,
} from './page.js';
export { JSHandle, ElementHandle, Download } from './handles.js';
export { APIRequestContext, APIResponse, request } from './request.js';
export { TestInfo } from './context.js';
export { defineConfig, devices } from './config.js';
export { test, expect } from './runner.js';
export type * from './types.js';
export const errors = { TimeoutError };
/** Playwright's `selectors`: the test id attribute is the `testIdAttribute` option here. */
export const selectors = {
  setTestIdAttribute(_attribute: string): void {},
  async register(): Promise<void> {
    throw new Error('lightning-panda: custom selector engines are not supported');
  },
};
export type BrowserType = typeof import('./browser.js').chromium;
