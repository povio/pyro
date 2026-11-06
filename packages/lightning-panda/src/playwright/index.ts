// `@povio/lightning-panda/playwright`: Playwright's library API (`chromium`,
// Browser, BrowserContext, Page, Locator) on lightningpanda, for scripts
// that drive a browser without the test runner.
import { TimeoutError } from './page.js';

export { chromium, Browser, BrowserContext, type BootSpec, type ContextOptions } from './browser.js';
export { Page, Frame, Locator, FrameLocator, Request, Response, Route, Dialog, ConsoleMessage, Keyboard, Mouse, TimeoutError } from './page.js';
export const errors = { TimeoutError };
