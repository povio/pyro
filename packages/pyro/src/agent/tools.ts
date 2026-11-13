// The browser tools: one table for the MCP server and for tool calling.
// Names and input shapes follow Playwright MCP's (models know them), with
// Pyro's `browser_get_text` and `browser_profiles` added.
//
// Every action settles before it returns (Pyro waits until the page is
// quiet), so the snapshot taken right after it is the action's outcome: an
// action's result is one line, what changed (URL, new tabs, dialogs,
// downloads, console errors) and, by default, the new snapshot.
import { writeFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import type { Locator } from '../wright/page.js';
import { clip, errorResult, quote, text, untrusted, type ToolContent, type ToolResult } from './content.js';
import { s, type JsonSchema, type ObjectSchema } from './schema.js';
import { settle, withTimeout, type BrowserAgentSession, type Tab } from './session.js';

/** Groups of tools that can be switched off (`--caps`); `core` always stays. */
export const CAPABILITIES = ['core', 'tabs', 'evaluate', 'files', 'profiles'] as const;
export type Capability = (typeof CAPABILITIES)[number];

/** MCP's tool annotations: hints for clients deciding what to ask the user about. */
export type ToolAnnotations = {
  title?: string;
  readOnlyHint: boolean;
  destructiveHint: boolean;
  idempotentHint: boolean;
  openWorldHint: boolean;
};

/** A tool as data: what MCP's `tools/list` and the provider converters need. */
export type ToolDefinition = {
  name: string;
  title: string;
  description: string;
  inputSchema: ObjectSchema;
  outputSchema?: JsonSchema & { type: 'object' };
  annotations: ToolAnnotations;
};

export type BrowserTool = ToolDefinition & {
  capability: Capability;
  /** Allowed while a dialog is open (other tools are refused until it is answered). */
  modalSafe?: boolean;
  execute(input: any, session: BrowserAgentSession, signal?: AbortSignal): Promise<ToolResult>;
};

// ---- shared parameters ----

const ELEMENT = 'Human-readable description of the element (e.g. "Sign in button"), used for permission prompts and logs';
const REF = 'Exact target element reference from the latest page snapshot (e.g. "e12")';
const element = s.string(ELEMENT);
const ref = s.string(REF);

/** Hints by kind: read-only tools, actions on the page, and actions that may change or lose state. */
const READ = { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: true };
const ACT = { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: true };
const CHANGE = { readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: true };

// ---- helpers ----

class ToolInputError extends Error {}

/** `ref` as the model may write it (`e12`, `[ref=e12]`, `ref=e12`), checked against the page. */
async function locate(tab: Tab, refInput: string): Promise<Locator> {
  const m = /^\s*\[?(?:ref=)?((?:f\d+)?e\d+)\]?\s*$/.exec(refInput);
  if (!m) throw new ToolInputError(`ref ${quote(refInput)} is not a snapshot ref: refs look like "e12" (from browser_snapshot)`);
  const locator = tab.page.locator(`aria-ref=${m[1]}`);
  if ((await locator.count()) === 0) {
    throw new ToolInputError(
      `Ref ${m[1]} is not on the page now: the page changed since the snapshot it came from. Take a new snapshot with browser_snapshot and use a ref from it.`,
    );
  }
  return locator;
}

/** Waits `ms`, or less when `signal` aborts. */
function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal?.aborted) return resolve();
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      signal?.removeEventListener('abort', done);
      resolve();
    }
    signal?.addEventListener('abort', done, { once: true });
  });
}

/** The modifier names the engine takes (`ControlOrMeta` is Meta on macOS, as in Playwright). */
const modifier = (m: string) => (m === 'ControlOrMeta' ? (process.platform === 'darwin' ? 'Meta' : 'Control') : m);

/**
 * Runs an action on `tab`. A dialog the action opens interrupts it: the
 * result then reports the dialog, and the action finishes once
 * `browser_handle_dialog` answers it.
 */
async function act(
  session: BrowserAgentSession,
  tab: Tab,
  done: string | ((value: unknown) => string),
  fn: () => Promise<unknown>,
  o: { snapshot?: boolean } = {},
): Promise<ToolResult> {
  const before = tab.page.url();
  const modal = tab.waitModal();
  const outcome = settle(fn());
  const first = await Promise.race([outcome, modal.opened.then(() => 'modal' as const)]);
  modal.dispose();
  if (first === 'modal' && tab.dialog) {
    tab.pending = { outcome, before };
    const started = typeof done === 'string' ? done : 'The action started.';
    return report(session, tab, `${started} It opened a dialog, which holds the page until it is answered (see Modal state).`, { before, snapshot: false });
  }
  const result = first === 'modal' ? await outcome : first;
  if (!result.ok) throw result.error;
  return report(session, tab, typeof done === 'string' ? done : done(result.value), { before, snapshot: o.snapshot });
}

/** The modal states of every tab, as lines telling the model what to do. */
function modalLines(session: BrowserAgentSession): string[] {
  const lines: string[] = [];
  session.tabs.forEach((t, i) => {
    if (t.dialog) {
      const d = t.dialog;
      const prompt = d.type() === 'prompt';
      lines.push(
        `- Tab [${i}]: a ${d.type()} dialog is open: ${quote(d.message())}${prompt ? ` (default answer ${quote(d.defaultValue())})` : ''}. Answer it with browser_handle_dialog (accept: true or false${prompt ? ', promptText' : ''}); other tools wait until then.`,
      );
    }
    if (t.chooser) {
      lines.push(`- Tab [${i}]: a file chooser is open${t.chooser.isMultiple() ? ' (several files allowed)' : ''}. Choose files with browser_file_upload, or call it with an empty paths list to cancel.`);
    }
  });
  return lines;
}

async function tabLines(session: BrowserAgentSession): Promise<string> {
  const safe = !session.modalTab();
  const lines: string[] = [];
  for (const [i, t] of session.tabs.entries()) {
    if (safe && !t.page.isClosed()) t.title = await withTimeout(t.page.title(), 5_000).catch(() => t.title);
    const marks = `${t === session.current ? ' (current)' : ''}${t.entry.profile ? ` (profile ${t.entry.profile})` : ''}`;
    lines.push(`- [${i}]${marks} ${quote(t.title)} ${t.page.url()}`);
  }
  return lines.join('\n');
}

function tabData(session: BrowserAgentSession) {
  return session.tabs.map((t, i) => ({ index: i, url: t.page.url(), title: t.title, current: t === session.current, ...(t.entry.profile ? { profile: t.entry.profile } : {}) }));
}

/**
 * A result about `tab`: the result line, modal states, events, tabs (when
 * more than one), the page's URL, title and new console errors, and its
 * snapshot (`snapshot`, default: the session's after-action setting).
 */
async function report(
  session: BrowserAgentSession,
  tab: Tab,
  result: string,
  o: { before?: string; snapshot?: boolean; extra?: ToolContent[] } = {},
): Promise<ToolResult> {
  // The title is a round trip on the browser's connection: events sent before
  // it (a popup the action opened) have arrived once it is back.
  if (!session.modalTab() && !tab.page.isClosed()) tab.title = await withTimeout(tab.page.title(), 10_000).catch(() => tab.title);
  await session.sync();
  const sections = [`### Result\n${result}`];
  const modal = modalLines(session);
  if (modal.length) sections.push(`### Modal state\n${modal.join('\n')}`);
  const notes = session.takeNotes();
  if (notes.length) sections.push(`### Events\n${notes.map((n) => `- ${n}`).join('\n')}`);
  if (session.tabs.length > 1) sections.push(`### Tabs\n${await tabLines(session)}`);
  const held = !!session.modalTab();
  if (!tab.page.isClosed()) {
    const url = tab.page.url();
    let page = `### Page\n- URL: ${url}${o.before !== undefined && o.before !== url ? ` (was ${o.before})` : ''}\n- Title: ${quote(tab.title)}`;
    const errors = tab.takeNewErrors();
    if (errors.length) {
      page += `\n${untrusted(`- New console errors (${errors.length})`, errors.map((e) => `${e.text}${e.where ? ` (${e.where})` : ''}`).join('\n'))}`;
    }
    sections.push(page);
    const snapshot = o.snapshot ?? session.snapshotAfterAction;
    if (snapshot && !held) {
      const snap = await withTimeout(tab.page.ariaSnapshot({ mode: 'ai', timeout: session.actionTimeout }), session.actionTimeout + 5_000).catch(
        (e) => `(no snapshot: ${message(e)})`,
      );
      sections.push(untrusted('### Snapshot', clip(snap, session.maxSnapshotChars), 'yaml'));
    } else if (o.snapshot === undefined && !held) {
      sections.push('(Snapshots after actions are off: call browser_snapshot to see the page.)');
    }
  }
  return { content: [text(sections.join('\n\n')), ...(o.extra ?? [])] };
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** A URL as typed by a model: `example.com` is `https://example.com/`, `localhost:3000` is `http://localhost:3000`. */
function normalizeUrl(url: string): string {
  const u = url.trim();
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(u) || /^(about|data|blob|javascript):/i.test(u)) return u;
  if (/^(localhost|127\.\d+\.\d+\.\d+|\[::1\]|0\.0\.0\.0)(:|\/|$)/i.test(u)) return `http://${u}`;
  return `https://${u}`;
}

async function navigate(session: BrowserAgentSession, tab: Tab, url: string, prefix = ''): Promise<ToolResult> {
  const target = normalizeUrl(url);
  if (/^javascript:/i.test(target)) throw new ToolInputError('javascript: URLs are not opened; run code with browser_evaluate (when the server offers it)');
  if (/^file:/i.test(target)) {
    // Local files only from the directories uploads may read.
    let path: string;
    try {
      path = fileURLToPath(target);
    } catch {
      throw new ToolInputError(`not a file URL: ${target}`);
    }
    session.uploadPath(path);
  }
  const why = session.policy.check(target);
  if (why) return errorResult(`Not navigating to ${target}: ${why} (the server's --allowed-origins / --blocked-origins settings).`);
  return act(session, tab, `${prefix}Navigated to ${target}.`, () => tab.page.goto(target, { timeout: session.navigationTimeout }), { snapshot: true });
}

const stamp = () => new Date().toISOString().replace(/[:.]/g, '-');

function pngSize(buf: Buffer): string {
  return buf.length > 24 && buf.toString('ascii', 1, 4) === 'PNG' ? `, ${buf.readUInt32BE(16)}x${buf.readUInt32BE(20)}` : '';
}

/** Collapses the whitespace of page text: trimmed lines, at most one empty line in a row. */
function compactText(raw: string): string {
  return raw
    .split('\n')
    .map((l) => l.replace(/[ \t\u00a0]+/g, ' ').trim())
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

/** Largest image side the Claude API accepts. */
const MAX_IMAGE_SIDE = 8000;

// ---- the tools ----

const STATIC_TYPES = new Set(['image', 'stylesheet', 'font', 'script', 'media']);

export const TOOLS: BrowserTool[] = [
  {
    name: 'browser_navigate',
    title: 'Navigate to a URL',
    description:
      'Open a URL in the current tab and wait until the page has loaded. Returns the page snapshot (elements you can act on carry refs like [ref=e12]), the URL and title. Use it to start on a site or go to a known address; to follow a link on the page, click it.',
    inputSchema: s.object({ url: s.string('The URL to open, e.g. "https://example.com/login"') }, ['url']),
    annotations: { ...ACT, idempotentHint: true },
    capability: 'core',
    async execute({ url }: { url: string }, session) {
      return navigate(session, await session.tab(), url);
    },
  },
  {
    name: 'browser_navigate_back',
    title: 'Go back',
    description: "Go back to the previous page in the current tab's history. Returns the page snapshot.",
    inputSchema: s.object({}),
    annotations: ACT,
    capability: 'core',
    async execute(_input, session) {
      const tab = await session.tab();
      return act(session, tab, 'Went back.', () => tab.page.goBack({ timeout: session.navigationTimeout }), { snapshot: true });
    },
  },
  {
    name: 'browser_snapshot',
    title: 'Page snapshot',
    description:
      "Capture the current tab's accessibility snapshot: the page as a tree of roles, names and text, where every element you can act on carries a ref like [ref=e12] for the action tools. Also returns the URL, title, open tabs and console errors since the last call. Use it before acting and whenever you are unsure what the page shows; it is cheaper and more precise than a screenshot. Action tools already return a fresh snapshot.",
    inputSchema: s.object({}),
    annotations: READ,
    capability: 'core',
    async execute(_input, session) {
      const tab = await session.tab();
      return report(session, tab, 'Snapshot of the current tab.', { snapshot: true });
    },
  },
  {
    name: 'browser_click',
    title: 'Click',
    description:
      'Click an element of the page by its ref from the latest snapshot. Waits until the element is visible, enabled and still, clicks it, then waits until the page settles. Returns what changed (URL, new tabs, dialogs, downloads, console errors) and the new snapshot.',
    inputSchema: s.object(
      {
        element,
        ref,
        button: s.enum(['left', 'right', 'middle'], 'Mouse button (default left)'),
        doubleClick: s.boolean('Double-click instead of a single click (default false)'),
        modifiers: s.array(s.enum(['Alt', 'Control', 'ControlOrMeta', 'Meta', 'Shift'], 'A modifier key'), 'Modifier keys held during the click'),
      },
      ['element', 'ref'],
    ),
    annotations: CHANGE,
    capability: 'core',
    async execute(i: { element: string; ref: string; button?: 'left' | 'right' | 'middle'; doubleClick?: boolean; modifiers?: string[] }, session) {
      const tab = await session.tab();
      const locator = await locate(tab, i.ref);
      const o = { timeout: session.actionTimeout, ...(i.button ? { button: i.button } : {}), ...(i.modifiers?.length ? { modifiers: i.modifiers.map(modifier) } : {}) };
      return act(session, tab, `${i.doubleClick ? 'Double-clicked' : 'Clicked'} ${quote(i.element)}.`, () => (i.doubleClick ? locator.dblclick(o) : locator.click(o)));
    },
  },
  {
    name: 'browser_type',
    title: 'Type text',
    description:
      'Type text into an editable element (text box, text area, content-editable) by ref, replacing what it holds. submit presses Enter afterwards (to send a search or a form). slowly types one key at a time, for fields that react to each key (autocomplete, key handlers); by default the text goes in at once.',
    inputSchema: s.object(
      {
        element,
        ref,
        text: s.string('The text to type'),
        submit: s.boolean('Press Enter after typing (default false)'),
        slowly: s.boolean('Type one key at a time instead of filling the text in at once (default false)'),
      },
      ['element', 'ref', 'text'],
    ),
    annotations: CHANGE,
    capability: 'core',
    async execute(i: { element: string; ref: string; text: string; submit?: boolean; slowly?: boolean }, session) {
      const tab = await session.tab();
      const locator = await locate(tab, i.ref);
      const timeout = session.actionTimeout;
      return act(session, tab, `Typed ${quote(i.text)} into ${quote(i.element)}${i.submit ? ' and pressed Enter' : ''}.`, async () => {
        if (i.slowly) {
          await locator.fill('', { timeout });
          await locator.pressSequentially(i.text, { timeout });
        } else {
          await locator.fill(i.text, { timeout });
        }
        if (i.submit) await locator.press('Enter', { timeout });
      });
    },
  },
  {
    name: 'browser_fill_form',
    title: 'Fill form',
    description:
      'Fill several form fields in one call, each by ref: text boxes, checkboxes, radio buttons, dropdowns (combobox) and sliders. Faster than one call per field. Returns the new snapshot; submit the form with browser_click.',
    inputSchema: s.object(
      {
        fields: s.array(
          s.object(
            {
              name: s.string('Human-readable field name'),
              type: s.enum(['textbox', 'checkbox', 'radio', 'combobox', 'slider'], 'Kind of field'),
              ref: s.string(REF),
              value: s.string('The value: the text for a textbox, "true" or "false" for a checkbox or radio button, the option label for a combobox, the number for a slider'),
            },
            ['name', 'type', 'ref', 'value'],
          ),
          'The fields to fill, in order',
        ),
      },
      ['fields'],
    ),
    annotations: CHANGE,
    capability: 'core',
    async execute({ fields }: { fields: { name: string; type: string; ref: string; value: string }[] }, session) {
      const tab = await session.tab();
      const timeout = session.actionTimeout;
      const located: Locator[] = [];
      for (const f of fields) located.push(await locate(tab, f.ref));
      return act(session, tab, `Filled ${fields.length} field${fields.length === 1 ? '' : 's'}: ${fields.map((f) => quote(f.name)).join(', ')}.`, async () => {
        for (const [k, f] of fields.entries()) {
          const locator = located[k];
          try {
            if (f.type === 'checkbox' || f.type === 'radio') {
              if (!['true', 'false'].includes(f.value.toLowerCase())) throw new ToolInputError(`the value of a ${f.type} is "true" or "false"`);
              await locator.setChecked(f.value.toLowerCase() === 'true', { timeout });
            } else if (f.type === 'combobox') {
              await locator.selectOption({ label: f.value }, { timeout }).catch(async (e) => {
                // A value instead of a label, or a text box with suggestions.
                if (await locator.evaluate((el: Element) => el.tagName === 'SELECT')) return locator.selectOption(f.value, { timeout });
                if (!String(e).includes('select')) throw e;
                return locator.fill(f.value, { timeout });
              });
            } else {
              await locator.fill(f.value, { timeout });
            }
          } catch (e) {
            throw new Error(`field ${quote(f.name)} (${f.ref}): ${message(e)}`);
          }
        }
      });
    },
  },
  {
    name: 'browser_select_option',
    title: 'Select option',
    description: 'Select one or more options of a dropdown (<select> or list box) by ref. values are the labels (or values) of the options to select. Returns the new snapshot.',
    inputSchema: s.object({ element, ref, values: s.array(s.string('An option label or value'), 'The options to select (one, or several in a multiple select)') }, ['element', 'ref', 'values']),
    annotations: { ...CHANGE, idempotentHint: true },
    capability: 'core',
    async execute(i: { element: string; ref: string; values: string[] }, session) {
      const tab = await session.tab();
      const locator = await locate(tab, i.ref);
      return act(session, tab, `Selected ${i.values.map(quote).join(', ')} in ${quote(i.element)}.`, () => locator.selectOption(i.values, { timeout: session.actionTimeout }));
    },
  },
  {
    name: 'browser_hover',
    title: 'Hover',
    description: 'Move the mouse over an element by ref, to open a hover menu or a tooltip. Returns the new snapshot.',
    inputSchema: s.object({ element, ref }, ['element', 'ref']),
    annotations: { ...ACT, idempotentHint: true },
    capability: 'core',
    async execute(i: { element: string; ref: string }, session) {
      const tab = await session.tab();
      const locator = await locate(tab, i.ref);
      return act(session, tab, `Hovered over ${quote(i.element)}.`, () => locator.hover({ timeout: session.actionTimeout }));
    },
  },
  {
    name: 'browser_drag',
    title: 'Drag and drop',
    description: 'Drag one element onto another (drag and drop), both by ref. Returns the new snapshot.',
    inputSchema: s.object(
      {
        startElement: s.string('Human-readable description of the element to drag'),
        startRef: s.string('Ref of the element to drag, from the latest snapshot'),
        endElement: s.string('Human-readable description of the element to drop on'),
        endRef: s.string('Ref of the element to drop on, from the latest snapshot'),
      },
      ['startElement', 'startRef', 'endElement', 'endRef'],
    ),
    annotations: CHANGE,
    capability: 'core',
    async execute(i: { startElement: string; startRef: string; endElement: string; endRef: string }, session) {
      const tab = await session.tab();
      const from = await locate(tab, i.startRef);
      const to = await locate(tab, i.endRef);
      return act(session, tab, `Dragged ${quote(i.startElement)} onto ${quote(i.endElement)}.`, () => from.dragTo(to, { timeout: session.actionTimeout }));
    },
  },
  {
    name: 'browser_press_key',
    title: 'Press a key',
    description:
      'Press a key or a key combination on the focused element (the page when nothing has focus): a key name (Enter, Escape, Tab, ArrowDown, PageDown, Backspace, F5), a character ("a", "/"), or a chord ("Control+a", "Shift+Tab"). Returns the new snapshot.',
    inputSchema: s.object({ key: s.string('The key, character or chord to press, e.g. "Enter", "ArrowLeft", "a" or "Control+a"') }, ['key']),
    annotations: CHANGE,
    capability: 'core',
    async execute({ key }: { key: string }, session) {
      const tab = await session.tab();
      return act(session, tab, `Pressed ${quote(key)}.`, () => tab.page.keyboard.press(key));
    },
  },
  {
    name: 'browser_wait_for',
    title: 'Wait for text or time',
    description:
      'Wait until a text appears on the page, a text disappears, or some seconds pass (at most 60 per call; a text is waited for up to 30 seconds). For content that loads late (search results, a spinner going away). Actions already wait for the page to settle: do not wait after every action. Returns the snapshot.',
    inputSchema: s.object({
      text: s.string('Text to wait for (a case-insensitive substring of the visible text)'),
      textGone: s.string('Text to wait to disappear'),
      time: s.number('Seconds to wait (at most 60)'),
    }),
    annotations: READ,
    capability: 'core',
    async execute(i: { text?: string; textGone?: string; time?: number }, session, signal) {
      if (i.text === undefined && i.textGone === undefined && i.time === undefined) throw new ToolInputError('give text, textGone or time');
      if (i.time !== undefined && !(i.time >= 0)) throw new ToolInputError('time is a number of seconds, 0 or more');
      const tab = await session.tab();
      const done: string[] = [];
      const wait = async () => {
        if (i.time !== undefined) {
          await sleep(Math.min(i.time, 60) * 1000, signal);
          done.push(`${Math.min(i.time, 60)} s passed`);
        }
        if (i.text !== undefined) {
          await tab.page.getByText(i.text).first().waitFor({ state: 'visible', timeout: 30_000 });
          done.push(`${quote(i.text)} appeared`);
        }
        if (i.textGone !== undefined) {
          await tab.page.getByText(i.textGone).first().waitFor({ state: 'hidden', timeout: 30_000 });
          done.push(`${quote(i.textGone)} is gone`);
        }
      };
      return act(session, tab, () => `Waited: ${done.join(', ')}.`, wait, { snapshot: true });
    },
  },
  {
    name: 'browser_handle_dialog',
    title: 'Answer a dialog',
    description:
      'Answer the open JavaScript dialog (alert, confirm, prompt or beforeunload): accept true for OK, false for Cancel; promptText is the answer typed into a prompt. The action that opened the dialog then finishes. Returns the new snapshot.',
    inputSchema: s.object({ accept: s.boolean('true to accept (OK), false to dismiss (Cancel)'), promptText: s.string('The text to answer a prompt dialog with') }, ['accept']),
    annotations: CHANGE,
    capability: 'core',
    modalSafe: true,
    async execute(i: { accept: boolean; promptText?: string }, session) {
      const tab = session.modalTab();
      if (!tab?.dialog) return errorResult('No dialog is open.');
      const dialog = tab.dialog;
      tab.dialog = undefined;
      if (i.accept) await dialog.accept(i.promptText);
      else await dialog.dismiss();
      const switched = tab !== session.current;
      session.select(tab);
      let result = `${i.accept ? 'Accepted' : 'Dismissed'} the ${dialog.type()} dialog ${quote(dialog.message())}${i.accept && i.promptText !== undefined ? ` with ${quote(i.promptText)}` : ''}.${switched ? ' Switched to its tab.' : ''}`;
      const pending = tab.pending;
      tab.pending = undefined;
      if (pending) {
        const modal = tab.waitModal();
        const first = await Promise.race([pending.outcome, modal.opened.then(() => 'modal' as const), sleep(session.navigationTimeout).then(() => 'slow' as const)]);
        modal.dispose();
        if (first === 'modal' && tab.dialog) {
          tab.pending = pending;
          return report(session, tab, `${result} Then another dialog opened (see Modal state).`, { before: pending.before, snapshot: false });
        }
        if (first === 'slow') result += ' The interrupted action has not finished yet.';
        else if (first !== 'modal' && !first.ok) result += ` The interrupted action then failed: ${message(first.error)}`;
      }
      return report(session, tab, result, { before: pending?.before, snapshot: true });
    },
  },
  {
    name: 'browser_tabs',
    title: 'Manage tabs',
    description:
      "List, open, close or switch tabs. list shows every tab; new opens a tab in the current tab's context (at url when given) and switches to it; close closes the tab at index (default: the current one); select switches to the tab at index. Popups that pages open appear as new tabs.",
    inputSchema: s.object(
      {
        action: s.enum(['list', 'new', 'close', 'select'], 'What to do'),
        index: s.integer('Tab index from the tab list, for close and select'),
        url: s.string('URL to open in the new tab, for new'),
      },
      ['action'],
    ),
    annotations: CHANGE,
    capability: 'tabs',
    modalSafe: true,
    async execute(i: { action: 'list' | 'new' | 'close' | 'select'; index?: number; url?: string }, session) {
      if (i.action !== 'list' && session.modalTab()) {
        return errorResult('A dialog is open: answer it with browser_handle_dialog before changing tabs.');
      }
      if (i.action === 'list') {
        await session.sync();
        const notes = session.takeNotes();
        const list = session.tabs.length ? await tabLines(session) : '(no tabs: the browser starts with the next page tool)';
        const events = notes.length ? `### Events\n${notes.map((n) => `- ${n}`).join('\n')}\n\n` : '';
        return { content: [text(`${events}### Tabs\n${list}`)], structuredContent: { tabs: tabData(session) } };
      }
      if (i.action === 'new') {
        const entry = session.current?.entry;
        const tab = await session.newTab(entry);
        session.select(tab);
        if (i.url) return navigate(session, tab, i.url, `Opened tab [${session.tabs.indexOf(tab)}] and switched to it. `);
        return report(session, tab, `Opened tab [${session.tabs.indexOf(tab)}] and switched to it.`, { snapshot: false });
      }
      await session.sync();
      const index = i.index ?? (i.action === 'close' && session.current ? session.tabs.indexOf(session.current) : undefined);
      if (index === undefined) return errorResult(`index is required for ${i.action}`);
      const tab = session.tabs[index];
      if (!tab) return errorResult(`No tab [${index}]: ${session.tabs.length ? `tabs are 0 to ${session.tabs.length - 1}` : 'no tabs are open'}.`);
      if (i.action === 'select') {
        session.select(tab);
        return report(session, tab, `Switched to tab [${index}].`, { snapshot: true });
      }
      await tab.page.close();
      await session.sync();
      const list = session.tabs.length ? await tabLines(session) : '(no tabs left)';
      return { content: [text(`### Result\nClosed tab [${index}].\n\n### Tabs\n${list}`)], structuredContent: { tabs: tabData(session) } };
    },
  },
  {
    name: 'browser_file_upload',
    title: 'Upload files',
    description:
      'Choose the files for the open file chooser: first click the file input or upload button (browser_click), which opens the chooser, then call this with the files. paths are local file paths in the directories the server allows; an empty list cancels the chooser. Returns the new snapshot.',
    inputSchema: s.object({ paths: s.array(s.string('Absolute path of a local file'), 'The files to upload (empty to cancel)') }, ['paths']),
    annotations: CHANGE,
    capability: 'files',
    async execute({ paths }: { paths: string[] }, session) {
      const current = await session.tab();
      const tab = current.chooser ? current : (session.tabs.find((t) => t.chooser) ?? current);
      const chooser = tab.chooser;
      if (!chooser) return errorResult('No file chooser is open: click the file input or the upload button first (browser_click), then call browser_file_upload.');
      session.select(tab);
      if (!paths.length) {
        tab.chooser = undefined;
        return report(session, tab, 'Cancelled the file chooser.', { snapshot: false });
      }
      const files = paths.map((p) => session.uploadPath(p));
      if (files.length > 1 && !chooser.isMultiple()) throw new ToolInputError('this file input takes one file');
      tab.chooser = undefined;
      return act(session, tab, `Chose ${files.map(quote).join(', ')}.`, () => chooser.setFiles(files, { timeout: session.actionTimeout }));
    },
  },
  {
    name: 'browser_evaluate',
    title: 'Run JavaScript',
    description:
      'Run a JavaScript function in the page and return its result as JSON: "() => document.title", or "(element) => element.value" with element and ref to receive that element. Async functions are awaited. Use it to read what the snapshot does not show; to interact, prefer the action tools, which act like a user.',
    inputSchema: s.object(
      {
        function: s.string('JavaScript function source: "() => { /* code */ }", or "(element) => { /* code */ }" when element and ref are given'),
        element: s.string(`${ELEMENT}; with ref, the function gets this element`),
        ref: s.string(`${REF}; with element, the function gets this element`),
      },
      ['function'],
    ),
    annotations: CHANGE,
    capability: 'evaluate',
    async execute(i: { function: string; element?: string; ref?: string }, session) {
      const tab = await session.tab();
      const locator = i.ref ? await locate(tab, i.ref) : undefined;
      const run = () =>
        locator
          ? locator.evaluate(i.function, undefined, { timeout: session.actionTimeout })
          : tab.page.evaluate(`(async () => { const f = (${i.function}\n); return typeof f === 'function' ? await f() : f; })()`);
      const shown = (value: unknown) => {
        const json = value === undefined ? 'undefined' : JSON.stringify(value, null, 2);
        return `${untrusted('Result', clip(json ?? String(value), 20_000), 'json')}\nIf the function changed the page, take a new snapshot before acting.`;
      };
      return act(session, tab, shown, run, { snapshot: false });
    },
  },
  {
    name: 'browser_take_screenshot',
    title: 'Screenshot',
    description:
      "Take a screenshot of the current tab's viewport, of the whole scrollable page (fullPage), or of one element (element and ref), and return the image. Use it to check what the page looks like (layout, images, charts, colors); to find and act on elements use browser_snapshot, since refs only come from snapshots. filename also saves the image in the output directory.",
    inputSchema: s.object({
      type: s.enum(['png', 'jpeg'], 'Image format (default png)'),
      filename: s.string('File name to save the image as, inside the output directory (e.g. "home.png")'),
      fullPage: s.boolean('The whole scrollable page instead of the viewport (default false; not with an element)'),
      element: s.string(`${ELEMENT}; with ref, only this element is captured`),
      ref: s.string(`${REF}; with element, only this element is captured`),
    }),
    annotations: READ,
    capability: 'core',
    async execute(i: { type?: 'png' | 'jpeg'; filename?: string; fullPage?: boolean; element?: string; ref?: string }, session) {
      if (i.fullPage && i.ref) throw new ToolInputError('fullPage cannot be used with an element');
      const tab = await session.tab();
      const type = i.type ?? 'png';
      const quality = type === 'jpeg' ? 80 : undefined;
      let buf: Buffer;
      let what: string;
      let note = '';
      if (i.ref) {
        const locator = await locate(tab, i.ref);
        buf = await locator.screenshot({ type, quality, timeout: session.actionTimeout });
        what = quote(i.element ?? i.ref);
      } else if (i.fullPage) {
        const size = await tab.page.evaluate<{ w: number; h: number }>('({ w: document.documentElement.clientWidth, h: Math.max(document.documentElement.scrollHeight, document.body ? document.body.scrollHeight : 0) })');
        if (size.h > MAX_IMAGE_SIDE) {
          buf = await tab.page.screenshot({ type, quality, fullPage: true, clip: { x: 0, y: 0, width: size.w, height: MAX_IMAGE_SIDE } });
          note = ` Cut at ${MAX_IMAGE_SIDE} px of ${size.h}: the page is taller than an image may be; scroll, or take parts by element.`;
        } else {
          buf = await tab.page.screenshot({ type, quality, fullPage: true });
        }
        what = 'the full page';
      } else {
        buf = await tab.page.screenshot({ type, quality });
        what = 'the viewport';
      }
      const mimeType = type === 'png' ? ('image/png' as const) : ('image/jpeg' as const);
      const extra: ToolContent[] = [{ type: 'image', data: buf.toString('base64'), mimeType }];
      let saved = '';
      if (i.filename !== undefined) {
        const path = session.outputPath(i.filename, `page-${stamp()}.${type === 'png' ? 'png' : 'jpg'}`);
        writeFileSync(path, buf);
        saved = ` Saved to ${path}.`;
        extra.push({ type: 'resource_link', uri: pathToFileURL(path).href, name: i.filename, mimeType });
      }
      return report(session, tab, `Screenshot of ${what} (${type}${pngSize(buf)}).${note}${saved}`, { snapshot: false, extra });
    },
  },
  {
    name: 'browser_get_text',
    title: 'Read page text',
    description:
      "Return the visible text of the current page, or of one element (element and ref), as plain text without the snapshot's structure and refs: the cheapest way to read an article, search results or a table. Use browser_snapshot when you need refs to act.",
    inputSchema: s.object({ element: s.string(`${ELEMENT}; with ref, only this element's text`), ref: s.string(`${REF}; with element, only this element's text`) }),
    annotations: READ,
    capability: 'core',
    async execute(i: { element?: string; ref?: string }, session) {
      const tab = await session.tab();
      const locator = i.ref ? await locate(tab, i.ref) : tab.page.locator('body');
      const raw = await locator.innerText({ timeout: session.actionTimeout });
      const body = compactText(raw);
      const what = i.ref ? `Text of ${quote(i.element ?? i.ref)}` : 'Text of the page';
      return report(session, tab, `${untrusted(`${what} (${body.length} characters)`, clip(body, session.maxSnapshotChars), 'text')}`, { snapshot: false });
    },
  },
  {
    name: 'browser_console_messages',
    title: 'Console messages',
    description: 'List the console messages of the current tab (console.log, warn, error, and uncaught exceptions), oldest first, up to the last 1000.',
    inputSchema: s.object({ onlyErrors: s.boolean('Only errors and uncaught exceptions (default false)') }),
    outputSchema: { type: 'object', properties: { messages: s.array({ type: 'object', properties: { type: s.string('log, warning, error, …'), text: s.string('Text'), where: s.string('Source location') } }, 'Messages') } },
    annotations: { ...READ, openWorldHint: false },
    capability: 'core',
    modalSafe: true,
    async execute(i: { onlyErrors?: boolean }, session) {
      const tab = session.current;
      if (!tab) return { content: [text('No tab is open.')], structuredContent: { messages: [] } };
      const list = tab.console.filter((m) => !i.onlyErrors || m.type === 'error');
      tab.consoleSeen = tab.console.length;
      const body = list.map((m) => `[${m.type}] ${m.text}${m.where ? ` (${m.where})` : ''}`).join('\n');
      return {
        content: [text(list.length ? untrusted(`### ${list.length} console message${list.length === 1 ? '' : 's'}`, body) : `No console ${i.onlyErrors ? 'errors' : 'messages'}.`)],
        structuredContent: { messages: list },
      };
    },
  },
  {
    name: 'browser_network_requests',
    title: 'Network requests',
    description:
      'List the requests the current tab made since it opened (up to the last 500): method, URL, and status or failure. Successful images, fonts, stylesheets, scripts and media are left out unless includeStatic is true.',
    inputSchema: s.object({ includeStatic: s.boolean('Include successful static resources (default false)') }),
    outputSchema: {
      type: 'object',
      properties: {
        requests: s.array(
          { type: 'object', properties: { method: s.string('Method'), url: s.string('URL'), type: s.string('Resource type'), status: s.integer('HTTP status'), failure: s.string('Why it failed') } },
          'Requests',
        ),
      },
    },
    annotations: { ...READ, openWorldHint: false },
    capability: 'core',
    modalSafe: true,
    async execute(i: { includeStatic?: boolean }, session) {
      const tab = session.current;
      if (!tab) return { content: [text('No tab is open.')], structuredContent: { requests: [] } };
      const list = tab.requests.filter((r) => i.includeStatic || !STATIC_TYPES.has(r.type) || r.failure || (r.status ?? 0) >= 400);
      const body = list
        .map((r) => `[${r.method}] ${r.url} => ${r.failure ? `[failed: ${r.failure}]` : r.status !== undefined ? `[${r.status}${r.statusText ? ` ${r.statusText}` : ''}]` : '[pending]'} (${r.type})`)
        .join('\n');
      return {
        content: [text(list.length ? untrusted(`### ${list.length} request${list.length === 1 ? '' : 's'}`, body) : 'No requests (static resources left out).')],
        structuredContent: { requests: list },
      };
    },
  },
  {
    name: 'browser_pdf_save',
    title: 'Save as PDF',
    description: 'Save the current page as a PDF file in the output directory and return its path.',
    inputSchema: s.object({ filename: s.string('File name inside the output directory (default page-<time>.pdf)') }),
    annotations: { ...READ, idempotentHint: false },
    capability: 'files',
    async execute(i: { filename?: string }, session) {
      const tab = await session.tab();
      const path = session.outputPath(i.filename, `page-${stamp()}.pdf`);
      const buf = await tab.page.pdf();
      writeFileSync(path, buf);
      return report(session, tab, `Saved the page as a PDF (${buf.length} bytes): ${path}`, {
        snapshot: false,
        extra: [{ type: 'resource_link', uri: pathToFileURL(path).href, name: i.filename ?? path.split(/[\\/]/).pop()!, mimeType: 'application/pdf' }],
      });
    },
  },
  {
    name: 'browser_resize',
    title: 'Resize viewport',
    description: "Resize the current tab's viewport, in CSS pixels (e.g. 390x844 for a phone, 1280x720 for a laptop). Each side 100 to 8000.",
    inputSchema: s.object({ width: s.integer('Width in CSS pixels'), height: s.integer('Height in CSS pixels') }, ['width', 'height']),
    annotations: { ...ACT, idempotentHint: true, openWorldHint: false },
    capability: 'core',
    async execute(i: { width: number; height: number }, session) {
      for (const v of [i.width, i.height]) if (v < 100 || v > 8000) throw new ToolInputError('width and height are 100 to 8000');
      const tab = await session.tab();
      await tab.page.setViewportSize({ width: i.width, height: i.height });
      return report(session, tab, `Resized the viewport to ${i.width}x${i.height}. The layout may have changed: take a new snapshot before acting.`, { snapshot: false });
    },
  },
  {
    name: 'browser_profiles',
    title: 'Stored identities',
    description:
      "Stored identities, to stay signed in across sessions: save writes the current tab's cookies and localStorage under name; load opens a new tab in a context of its own with that saved state and the identity's settings (user agent, proxy, locale, time zone, viewport from <name>.identity.json) and switches to it; list shows the stored identities; delete removes a saved state. Sign in once, save, and next time load instead of signing in again. Saved states are credentials: save only what the user wants kept.",
    inputSchema: s.object({ action: s.enum(['list', 'save', 'load', 'delete'], 'What to do'), name: s.string('Profile name (letters, digits, "_", "-", "."), for save, load and delete') }, ['action']),
    annotations: { ...CHANGE, openWorldHint: false },
    capability: 'profiles',
    async execute(i: { action: 'list' | 'save' | 'load' | 'delete'; name?: string }, session) {
      const store = session.profiles;
      if (i.action === 'list') {
        const list = store.list();
        const lines = list.map((p) => `- ${p.name}: ${p.state ? `saved ${p.savedAt}, ${p.cookies} cookie${p.cookies === 1 ? '' : 's'}${p.sites.length ? `, signed in at ${p.sites.join(', ')}` : ''}` : 'no saved state'}${p.identity ? '; identity file' : ''}`);
        return { content: [text(list.length ? `### Stored identities in ${store.dir}\n${lines.join('\n')}` : `No stored identities in ${store.dir}.`)], structuredContent: { profiles: list } };
      }
      if (!i.name) throw new ToolInputError(`name is required for ${i.action}`);
      store.paths(i.name);
      if (i.action === 'save') {
        const tab = await session.tab();
        const state = await tab.entry.context.storageState();
        const { kept } = store.save(i.name, state);
        const origins = state.origins.filter((o) => o.localStorage.length).map((o) => o.origin);
        const domains = [...new Set(state.cookies.map((c) => (c.domain ?? '').replace(/^\./, '')).filter(Boolean))];
        return {
          content: [
            text(
              `Saved profile ${quote(i.name)} to ${store.paths(i.name).state} (owner-only): ${state.cookies.length} cookie${state.cookies.length === 1 ? '' : 's'}${domains.length ? ` for ${domains.join(', ')}` : ''}; localStorage of ${origins.length ? origins.join(', ') : 'no origin'}${kept.length ? `; kept the localStorage of ${kept.join(', ')} from the previous save` : ''}. Load it later with browser_profiles load.`,
            ),
          ],
        };
      }
      if (i.action === 'delete') {
        const { identityKept } = store.delete(i.name);
        return { content: [text(`Deleted the saved state of profile ${quote(i.name)}.${identityKept ? ' Its identity file stays.' : ''}`)] };
      }
      const { tab, state, applied } = await session.profileContext(i.name);
      const cookies = state?.cookies.length ?? 0;
      const origins = state?.origins.filter((o) => o.localStorage.length).map((o) => o.origin) ?? [];
      const result = `Loaded profile ${quote(i.name)} into a new tab [${session.tabs.indexOf(tab)}] with a context of its own: ${cookies} cookie${cookies === 1 ? '' : 's'}${origins.length ? `, localStorage of ${origins.join(', ')}` : ''}${applied.length ? `; identity settings: ${applied.join(', ')}` : ''}. Navigate to the site to continue as this identity.`;
      return report(session, tab, result, { snapshot: false });
    },
  },
  {
    name: 'browser_close',
    title: 'Close the browser',
    description: 'Close the browser: every tab and context of this session, with any signed-in state not saved with browser_profiles. The next browser tool starts a fresh browser.',
    inputSchema: s.object({}),
    annotations: { ...CHANGE, idempotentHint: true, openWorldHint: false },
    capability: 'core',
    modalSafe: true,
    async execute(_input, session) {
      await session.closeBrowser();
      return { content: [text('Closed the browser. The next browser tool starts a new one.')] };
    },
  },
];

export { ToolInputError };
