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
import { capture, elementBox, enclosing, fitsModel, MODEL_IMAGE, pageMetrics, previewScale, RENDER_MAX, sha256, trim, type Captured, type Format, type Metrics, type Rect } from './capture.js';
import { clip, errorResult, fence, quote, text, untrusted, type ToolContent, type ToolResult } from './content.js';
import { when } from './profiles.js';
import { s, type JsonSchema, type ObjectSchema } from './schema.js';
import { expiredHint, settle, withTimeout, type BrowserAgentSession, type Tab } from './session.js';
import { checkSession, looksLikeSignIn } from './signin.js';

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
  o: { before?: string; snapshot?: boolean; extra?: ToolContent[]; structured?: Record<string, unknown> } = {},
): Promise<ToolResult> {
  // The title is a round trip on the browser's connection: events sent before
  // it (a popup the action opened) have arrived once it is back.
  if (!session.modalTab() && !tab.page.isClosed()) tab.title = await withTimeout(tab.page.title(), 10_000).catch(() => tab.title);
  await session.sync();
  const sections = [`### Result\n${result}`];
  const modal = modalLines(session);
  if (modal.length) sections.push(`### Modal state\n${modal.join('\n')}`);
  const notes = session.takeNotes();
  // A stored identity's tab on a sign-in page: its session may be gone, and the agent never signs in.
  const profile = tab.entry.profile;
  if (profile && !tab.page.isClosed() && looksLikeSignIn(tab.page.url()) && tab.signInNoted !== tab.page.url() && !result.includes('`pyro profile login')) {
    tab.signInNoted = tab.page.url();
    notes.push(
      `This tab is stored identity ${quote(profile)} and the page looks like a sign-in page: its session may have expired. Do not sign in yourself: ask the person to run \`${session.loginCommand(profile, tab.entry.login)}\`, then load the identity again (browser_profiles load).`,
    );
  }
  if (notes.length) sections.push(`### Events\n${notes.map((n) => `- ${n}`).join('\n')}`);
  if (session.tabs.length > 1) sections.push(`### Tabs\n${await tabLines(session)}`);
  const held = !!session.modalTab();
  if (!tab.page.isClosed()) {
    const url = tab.page.url();
    let page = `### Page\n- URL: ${url}${o.before !== undefined && o.before !== url ? ` (was ${o.before})` : ''}\n- Title: ${quote(tab.title)}\n- Viewport: ${viewportLine(tab)}`;
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
  return { content: [text(sections.join('\n\n')), ...(o.extra ?? [])], ...(o.structured ? { structuredContent: o.structured } : {}) };
}

/** "2056x1200 CSS px at deviceScaleFactor 3 (6168x3600 device px)". */
function viewportLine(tab: Tab): string {
  const { width, height } = tab.viewport();
  const dsf = tab.deviceScaleFactor;
  return `${width}x${height} CSS px at deviceScaleFactor ${dsf}${dsf !== 1 ? ` (${Math.round(width * dsf)}x${Math.round(height * dsf)} device px)` : ''}`;
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

/** Collapses the whitespace of page text: trimmed lines, at most one empty line in a row. */
function compactText(raw: string): string {
  return raw
    .split('\n')
    .map((l) => l.replace(/[ \t\u00a0]+/g, ' ').trim())
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

// ---- screenshots and boxes ----

type ShotInput = {
  type?: Format;
  filename?: string;
  fullPage?: boolean;
  element?: string;
  ref?: string;
  scale?: 'device' | 'css';
  clip?: Rect;
  includeImage?: boolean;
};

/** A number for a result line: at most 3 decimals. */
const num = (n: number) => String(Math.round(n * 1000) / 1000);
const rectText = (r: Rect) => `x ${num(r.x)}, y ${num(r.y)}, ${num(r.width)}x${num(r.height)}`;
const round3 = (r: Rect): Rect => ({ x: +num(r.x), y: +num(r.y), width: +num(r.width), height: +num(r.height) });
const scaled = (r: Rect, k: number): Rect => round3({ x: r.x * k, y: r.y * k, width: r.width * k, height: r.height * k });
const percent = (f: number) => `${f < 0.1 ? Math.round(f * 1000) / 10 : Math.round(f * 100)}%`;

/** What to capture: the area in CSS px from the document's top-left (CDP's clip), and how. */
type ShotPlan = {
  what: string;
  /** The area as the agent sees it: relative to the viewport, or to the document. */
  shown: Rect & { relativeTo: 'viewport' | 'document' };
  area: Rect;
  /** Painted from the document's top-left instead of at the scroll position. */
  beyond: boolean;
  /** The viewport: at device pixels, `Page.captureScreenshot` without a clip. */
  viewport: boolean;
  notes: string[];
};

async function planShot(i: ShotInput, tab: Tab, session: BrowserAgentSession, m: Metrics): Promise<ShotPlan> {
  const notes: string[] = [];
  if (i.ref) {
    const name = quote(i.element ?? i.ref);
    const locator = await locate(tab, i.ref);
    // As Playwright's element screenshots: scrolled into view, the box rounded out to whole CSS px.
    await locator.scrollIntoViewIfNeeded({ timeout: session.actionTimeout });
    const box = await elementBox(locator, session.actionTimeout);
    if (box.crossOrigin) throw new ToolInputError(`${name} is inside a cross-origin iframe, so its place on the page is unknown: capture the iframe element instead.`);
    if (!(box.width > 0 && box.height > 0)) throw new ToolInputError(`${name} has no size on the page (hidden or empty): there is nothing to capture.`);
    const now = await pageMetrics(tab.page);
    if (now.scrollX !== m.scrollX || now.scrollY !== m.scrollY) notes.push(`Scrolled it into view first: the page is scrolled to x ${num(now.scrollX)}, y ${num(now.scrollY)} now.`);
    Object.assign(m, now);
    const area = enclosing({ x: box.x + now.scrollX, y: box.y + now.scrollY, width: box.width, height: box.height });
    const fits = box.width <= now.width && box.height <= now.height;
    return { what: name, shown: { ...area, relativeTo: 'document' }, area, beyond: !fits, viewport: false, notes };
  }
  if (i.clip) {
    const where = i.fullPage ? 'document' : 'viewport';
    const bounds = i.fullPage ? { width: m.documentWidth, height: m.documentHeight } : { width: m.width, height: m.height };
    const c = trim(i.clip, bounds);
    if (!c) {
      throw new ToolInputError(
        `The clip (${rectText(i.clip)}) is outside the ${where} (${num(bounds.width)}x${num(bounds.height)} CSS px).${i.fullPage ? '' : ' A clip is relative to the viewport; with fullPage: true it is relative to the document.'}`,
      );
    }
    if (c.x !== i.clip.x || c.y !== i.clip.y || c.width !== i.clip.width || c.height !== i.clip.height) notes.push(`The clip reached past the ${where}: trimmed to ${rectText(c)}.`);
    const area = i.fullPage ? c : { ...c, x: c.x + m.scrollX, y: c.y + m.scrollY };
    return { what: `the clip ${rectText(c)} of the ${i.fullPage ? 'page' : 'viewport'}`, shown: { ...c, relativeTo: where }, area, beyond: !!i.fullPage, viewport: false, notes };
  }
  if (i.fullPage) {
    const area = { x: 0, y: 0, width: m.width, height: m.documentHeight };
    return { what: 'the full page', shown: { ...area, relativeTo: 'document' }, area, beyond: true, viewport: false, notes };
  }
  const shown = { x: 0, y: 0, width: m.width, height: m.height, relativeTo: 'viewport' as const };
  return { what: 'the viewport', shown, area: { x: m.scrollX, y: m.scrollY, width: m.width, height: m.height }, beyond: false, viewport: true, notes };
}

/** Why an area is not rendered at that size, and how to capture it instead. */
function tooLarge(p: ShotPlan, size: { width: number; height: number }, perCss: number, dpr: number, scale: 'device' | 'css'): string {
  const at = scale === 'css' ? 'CSS pixels' : `device pixels (deviceScaleFactor ${num(dpr)})`;
  let msg = `${p.what[0].toUpperCase()}${p.what.slice(1)} is ${num(p.area.width)}x${num(p.area.height)} CSS px: ${size.width}x${size.height} px at ${at}, more than the ${RENDER_MAX} px a side Pyro renders.`;
  const tallest = Math.floor(RENDER_MAX / perCss);
  if (p.area.width * perCss <= RENDER_MAX) {
    // In document coordinates, as fullPage clips take them.
    const { x, y } = p.area;
    const parts = Math.ceil(p.area.height / tallest);
    msg += ` Capture it in ${parts} parts of at most ${tallest} CSS px: fullPage: true with clip { x: ${num(x)}, y: ${num(y)}, width: ${num(p.area.width)}, height: ${tallest} }, then y: ${num(y + tallest)}, and so on.`;
  }
  if (scale === 'device' && Math.max(p.area.width, p.area.height) <= RENDER_MAX) msg += ` Or use scale: "css" (${Math.round(p.area.width)}x${Math.round(p.area.height)} px).`;
  return msg;
}

/**
 * `browser_take_screenshot`: the capture as the browser encoded it, saved
 * when asked (path, pixel size, sha256), and an image for the model: the
 * capture itself when it fits the model's image limits, else a smaller
 * rendering of the same area.
 */
async function takeScreenshot(i: ShotInput, session: BrowserAgentSession): Promise<ToolResult> {
  if (i.fullPage && i.ref) throw new ToolInputError('fullPage cannot be used with an element');
  if (i.ref && i.clip) throw new ToolInputError('give an element (element and ref) or a clip, not both');
  if (i.clip && !(i.clip.width > 0 && i.clip.height > 0)) throw new ToolInputError('clip width and height must be positive (CSS px)');
  const wantImage = i.includeImage ?? true;
  if (!wantImage && i.filename === undefined) throw new ToolInputError('with includeImage: false nothing would come back: give a filename to save the capture, or leave includeImage out');
  const tab = await session.tab();
  const format: Format = i.type ?? 'png';
  const scale = i.scale ?? 'device';
  // The file's name first: a bad one fails before anything is rendered.
  const path = i.filename !== undefined ? session.outputPath(i.filename, `page-${stamp()}.${format === 'png' ? 'png' : 'jpg'}`) : undefined;
  const m = await pageMetrics(tab.page);
  const p = await planShot(i, tab, session, m);
  const dpr = m.deviceScaleFactor;
  tab.deviceScaleFactor = dpr;
  const perCss = scale === 'css' ? 1 : dpr;
  const size = { width: Math.round(p.area.width * perCss), height: Math.round(p.area.height * perCss) };
  if (path && (size.width > RENDER_MAX || size.height > RENDER_MAX)) throw new ToolInputError(tooLarge(p, size, perCss, dpr, scale));
  const cdp = await tab.cdp();
  /** The area at `px` image pixels per CSS px. */
  const shoot = (px: number, f: Format): Promise<Captured> =>
    p.viewport && px === dpr ? capture(cdp, { format: f }) : capture(cdp, { format: f, clip: p.area, clipScale: px / dpr, beyond: p.beyond });

  let file: Captured | undefined;
  if (path) {
    file = await shoot(perCss, format);
    writeFileSync(path, file.buf);
  }
  let image: Captured | undefined;
  if (wantImage) {
    const fit = previewScale(p.area.width, p.area.height, perCss);
    let px = fit >= perCss * 0.999 ? perCss : fit;
    let f = format;
    for (let attempt = 0; attempt < 5 && !image; attempt++) {
      const c = px === perCss && file && file.format === f ? file : await shoot(px, f);
      if (fitsModel(c)) image = c;
      else if (f === 'png') f = 'jpeg'; // too many bytes: the same pixels as JPEG
      else px *= 0.7;
    }
  }

  const at = scale === 'css' ? 'at CSS pixels' : `at device pixels (deviceScaleFactor ${num(dpr)})`;
  const limits = `the model's image limits (at most ${MODEL_IMAGE.maxSide} px a side and ${MODEL_IMAGE.maxTokens} image tokens)`;
  const lines: string[] = [];
  const extra: ToolContent[] = [];
  const structured: Record<string, unknown> = {
    area: { ...round3(p.shown), relativeTo: p.shown.relativeTo },
    deviceScaleFactor: dpr,
    scale,
    format,
    width: file?.width ?? size.width,
    height: file?.height ?? size.height,
  };
  if (file && path) {
    const hash = sha256(file.buf);
    Object.assign(structured, { path, bytes: file.buf.length, sha256: hash });
    lines.push(
      `Screenshot of ${p.what} saved to ${path}: ${file.width}x${file.height} ${format === 'png' ? 'PNG' : 'JPEG'} ${at}, ${file.buf.length} bytes, sha256 ${hash}${format === 'png' ? ', the PNG exactly as the browser encoded it (lossless)' : ''}.`,
    );
  }
  if (image) {
    extra.push({ type: 'image', data: image.buf.toString('base64'), mimeType: image.format === 'png' ? 'image/png' : 'image/jpeg' });
    structured.image = { width: image.width, height: image.height, format: image.format, preview: image !== file && (image.width !== size.width || image.height !== size.height) };
    const kind = image.format === 'png' ? 'PNG' : 'JPEG';
    if (image === file) lines.push('The image below is the saved file itself.');
    else if (file) lines.push(`The image below is a ${image.width}x${image.height} ${kind} preview of it (${percent(image.width / file.width)} of its width), sized to ${limits}; the file is full size.`);
    else if (image.width === size.width && image.height === size.height) lines.push(`Screenshot of ${p.what} (${image.format}, ${image.width}x${image.height}).`);
    else lines.push(`Screenshot of ${p.what} (${image.format}, ${image.width}x${image.height}): a preview at ${percent(image.width / size.width)} of the ${size.width}x${size.height} ${scale === 'css' ? 'CSS-pixel' : 'device-pixel'} capture, sized to ${limits}. Give a filename to save it at full size.`);
  } else if (wantImage) {
    lines.push(`No image: ${p.what} does not fit ${limits} even scaled down; look at it in parts with clip.`);
  } else {
    lines.push('No image returned (includeImage: false).');
  }
  if (image && image.height > image.width * 6) lines.push('The area is much taller than wide, so the image is narrow: look at parts of it with clip for detail.');
  lines.push(...p.notes);
  if (file && path) extra.push({ type: 'resource_link', uri: pathToFileURL(path).href, name: i.filename || path.split(/[\\/]/).pop()!, mimeType: format === 'png' ? 'image/png' : 'image/jpeg' });
  return report(session, tab, lines.join('\n'), { snapshot: false, extra, structured });
}

/** `e12` from a ref as the model may write it (`[ref=e12]`). */
const refId = (input: string) => /((?:f\d+)?e\d+)/.exec(input)?.[1] ?? input;

const RECT_SCHEMA = (description: string): JsonSchema => ({ type: 'object', description, properties: { x: s.number('Left edge'), y: s.number('Top edge'), width: s.number('Width'), height: s.number('Height') } });

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
      "Capture the current tab: the viewport, the whole scrollable page (fullPage), a rectangle (clip, CSS px) or one element (element and ref). The capture is at device pixels by default (CSS size x deviceScaleFactor: a 2056x1200 viewport at deviceScaleFactor 3 is 6168x3600), or at CSS pixels with scale: \"css\"; clips and elements are cropped at that resolution. filename saves it in the output directory, byte for byte as the browser encoded it (PNG is lossless), and the result gives its path, pixel size and sha256. Unless includeImage is false you also get an image to look at: the capture itself when it fits the model's image limits (2576 px a side, 4784 image tokens), else a smaller preview of the same area. To act on elements use browser_snapshot: refs only come from snapshots.",
    inputSchema: s.object({
      type: s.enum(['png', 'jpeg'], 'Image format (default png; jpeg is lossy, quality 80)'),
      filename: s.string('Save the capture in the output directory under this name (e.g. "home.png"); the result gives its path, pixel size and sha256'),
      fullPage: s.boolean(`The whole scrollable page instead of the viewport (default false; not with an element). Pyro renders at most ${RENDER_MAX} px a side: at deviceScaleFactor 3 a page up to ${Math.floor(RENDER_MAX / 3)} CSS px tall; taller pages are refused with how to capture them in parts`),
      element: s.string(`${ELEMENT}; with ref, only this element is captured: its bounding box rounded out to whole CSS px, scrolled into view first`),
      ref: s.string(`${REF}; with element, only this element is captured`),
      scale: s.enum(['device', 'css'], 'Resolution: "device" (default), one image pixel per device pixel (CSS px x deviceScaleFactor); "css", one per CSS pixel'),
      clip: {
        ...s.object({ x: s.number('Left edge, CSS px'), y: s.number('Top edge, CSS px'), width: s.number('Width, CSS px'), height: s.number('Height, CSS px') }, ['x', 'y', 'width', 'height']),
        description:
          "Only this rectangle, in CSS px, cropped at the scale's resolution (a 100x50 clip at deviceScaleFactor 3 is 300x150 device px). Relative to the viewport's top-left (browser_get_bounding_box's css.viewport box); with fullPage, relative to the document's top-left (its css.document box). Not with an element",
      },
      includeImage: s.boolean('Return an image to look at (default true): the capture when it fits the model\'s image limits, else a preview of it. false returns only the saved file\'s path, size and sha256 (needs filename)'),
    }),
    outputSchema: {
      type: 'object',
      properties: {
        area: { ...RECT_SCHEMA('The captured area in CSS px'), properties: { ...RECT_SCHEMA('').properties, relativeTo: s.enum(['viewport', 'document'], 'What x and y are relative to') } },
        deviceScaleFactor: s.number('Device pixels per CSS pixel of the page'),
        scale: s.enum(['device', 'css'], 'Resolution of the capture'),
        format: s.enum(['png', 'jpeg'], 'Format of the capture'),
        width: s.integer('Width of the capture in pixels (of the saved file, when saved)'),
        height: s.integer('Height of the capture in pixels'),
        path: s.string('The saved file (with filename)'),
        bytes: s.integer('Size of the saved file in bytes'),
        sha256: s.string('SHA-256 of the saved file, hex'),
        image: { type: 'object', description: 'The image returned to look at', properties: { width: s.integer('Width'), height: s.integer('Height'), format: s.enum(['png', 'jpeg'], 'Format'), preview: s.boolean('A scaled-down preview, not the capture itself') } },
      },
    },
    annotations: READ,
    capability: 'core',
    async execute(i: ShotInput, session) {
      return takeScreenshot(i, session);
    },
  },
  {
    name: 'browser_get_bounding_box',
    title: 'Element bounding boxes',
    description:
      "Get the bounding boxes of elements by ref, to compute crop regions for browser_take_screenshot. For each element: css.viewport, its box in CSS px relative to the viewport (getBoundingClientRect; pass it as clip); css.document, relative to the document (plus the scroll offsets; pass it as clip with fullPage: true); device.viewport and device.document, the same boxes in device pixels (x deviceScaleFactor), where the element is in a device-pixel screenshot. Also the viewport size, the scroll offsets, the document size and the device scale factor. Elements inside iframes are given in the page's coordinates.",
    inputSchema: s.object(
      { elements: s.array(s.object({ element, ref }, ['element', 'ref']), 'The elements: a description and the ref from the latest snapshot of each') },
      ['elements'],
    ),
    outputSchema: {
      type: 'object',
      properties: {
        viewport: { type: 'object', description: 'Viewport size, CSS px', properties: { width: s.number('Width'), height: s.number('Height') } },
        scroll: { type: 'object', description: 'Scroll offsets of the page, CSS px', properties: { x: s.number('Horizontal'), y: s.number('Vertical') } },
        document: { type: 'object', description: 'Scrollable size of the document, CSS px', properties: { width: s.number('Width'), height: s.number('Height') } },
        deviceScaleFactor: s.number('Device pixels per CSS pixel'),
        elements: s.array(
          {
            type: 'object',
            properties: {
              element: s.string('The description given'),
              ref: s.string('The ref'),
              css: { type: 'object', description: 'CSS px', properties: { viewport: RECT_SCHEMA('Relative to the viewport'), document: RECT_SCHEMA('Relative to the document') } },
              device: { type: 'object', description: 'Device px', properties: { viewport: RECT_SCHEMA('Relative to the viewport'), document: RECT_SCHEMA('Relative to the document') } },
              inViewport: s.enum(['full', 'partial', 'none'], 'How much of the box the viewport shows'),
            },
          },
          'One entry per element, in order',
        ),
      },
    },
    annotations: READ,
    capability: 'core',
    async execute({ elements }: { elements: { element: string; ref: string }[] }, session) {
      if (!elements.length) throw new ToolInputError('give at least one element (element and ref)');
      const tab = await session.tab();
      const located: Locator[] = [];
      for (const e of elements) located.push(await locate(tab, e.ref));
      const m = await pageMetrics(tab.page);
      const dpr = m.deviceScaleFactor;
      tab.deviceScaleFactor = dpr;
      const out = [];
      for (const [k, e] of elements.entries()) {
        const b = await elementBox(located[k], session.actionTimeout);
        const viewport = round3(b);
        const document = round3({ ...b, x: b.x + m.scrollX, y: b.y + m.scrollY });
        const right = b.x + b.width;
        const bottom = b.y + b.height;
        const shownW = Math.min(right, m.width) - Math.max(b.x, 0);
        const shownH = Math.min(bottom, m.height) - Math.max(b.y, 0);
        const inViewport = !(b.width > 0 && b.height > 0) || shownW <= 0 || shownH <= 0 ? 'none' : shownW >= b.width - 1e-6 && shownH >= b.height - 1e-6 ? 'full' : 'partial';
        out.push({
          element: e.element,
          ref: refId(e.ref),
          css: { viewport, document },
          device: { viewport: scaled(viewport, dpr), document: scaled(document, dpr) },
          inViewport,
          ...(!(b.width > 0 && b.height > 0) ? { note: 'not rendered: the box has no size' } : {}),
          ...(b.crossOrigin ? { note: 'inside a cross-origin iframe: the box is relative to that iframe, not the page' } : {}),
        });
      }
      const data = {
        viewport: { width: m.width, height: m.height },
        scroll: { x: m.scrollX, y: m.scrollY },
        document: { width: m.documentWidth, height: m.documentHeight },
        deviceScaleFactor: dpr,
        elements: out,
      };
      const head = `Bounding boxes of ${out.length} element${out.length === 1 ? '' : 's'}: viewport ${num(m.width)}x${num(m.height)} CSS px, scrolled to x ${num(m.scrollX)}, y ${num(m.scrollY)}, deviceScaleFactor ${num(dpr)}. css.viewport is getBoundingClientRect (a clip for a viewport screenshot), css.document adds the scroll (a clip with fullPage: true), device.* are the same boxes in device pixels.`;
      // One line per element: compact for the model, and still JSON.
      const { elements: list, ...top } = data;
      const json = `${JSON.stringify(top).slice(0, -1)},"elements":[\n${list.map((e) => JSON.stringify(e)).join(',\n')}\n]}`;
      return report(session, tab, `${head}\n${fence(json, 'json')}`, { snapshot: false, structured: data });
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
    description:
      "Resize the current tab's viewport, in CSS pixels (e.g. 390x844 for a phone, 1280x720 for a laptop; each side 100 to 8000), and optionally set its device scale factor (devicePixelRatio, 0.5 to 8: 2 or 3 for a high-density screen). Screenshots are taken at device pixels: 2056x1200 at deviceScaleFactor 3 gives 6168x3600 images. The page keeps both across navigations.",
    inputSchema: s.object(
      {
        width: s.integer('Width in CSS pixels'),
        height: s.integer('Height in CSS pixels'),
        deviceScaleFactor: s.number('Device pixels per CSS pixel, 0.5 to 8 (default: unchanged)'),
      },
      ['width', 'height'],
    ),
    annotations: { ...ACT, idempotentHint: true, openWorldHint: false },
    capability: 'core',
    async execute(i: { width: number; height: number; deviceScaleFactor?: number }, session) {
      for (const v of [i.width, i.height]) if (v < 100 || v > 8000) throw new ToolInputError('width and height are 100 to 8000');
      if (i.deviceScaleFactor !== undefined && !(i.deviceScaleFactor >= 0.5 && i.deviceScaleFactor <= 8)) throw new ToolInputError('deviceScaleFactor is 0.5 to 8');
      const tab = await session.tab();
      await session.resize(tab, { width: i.width, height: i.height }, i.deviceScaleFactor);
      const m = await pageMetrics(tab.page);
      tab.deviceScaleFactor = m.deviceScaleFactor;
      const asked = i.deviceScaleFactor === undefined || m.deviceScaleFactor === i.deviceScaleFactor;
      const result = `Resized the viewport to ${num(m.width)}x${num(m.height)} CSS px at deviceScaleFactor ${num(m.deviceScaleFactor)}: a device-pixel screenshot of it is ${Math.round(m.width * m.deviceScaleFactor)}x${Math.round(m.height * m.deviceScaleFactor)}.${asked ? '' : ` (The page reports deviceScaleFactor ${num(m.deviceScaleFactor)}, not the ${i.deviceScaleFactor} asked for.)`} The layout may have changed: take a new snapshot before acting.`;
      return report(session, tab, result, { snapshot: false });
    },
  },
  {
    name: 'browser_profiles',
    title: 'Stored identities',
    description:
      "Stored identities, to stay signed in across sessions without signing in. load opens a new tab in a context of its own with a saved state (cookies and localStorage) and the identity's settings (user agent, proxy, locale, time zone, viewport, device scale factor from <name>.identity.json) and switches to it; for an identity a person signed in with `pyro profile login`, it also opens the app and says whether the session still holds. list shows the stored identities; save writes the current tab's cookies and localStorage under name; delete removes a saved state. Never sign in yourself to make or renew one: when a session has expired, ask the person to run `pyro profile login <name>`. Saved states are credentials: save only what the user wants kept; their values are never shown.",
    inputSchema: s.object({ action: s.enum(['list', 'save', 'load', 'delete'], 'What to do'), name: s.string('Profile name (letters, digits, "_", "-", "."), for save, load and delete') }, ['action']),
    annotations: { ...CHANGE, openWorldHint: false },
    capability: 'profiles',
    async execute(i: { action: 'list' | 'save' | 'load' | 'delete'; name?: string }, session) {
      const store = session.profiles;
      if (i.action === 'list') {
        const list = store.list();
        const lines = list.map((p) => {
          const saved = p.state ? `saved ${p.savedAt}, ${p.cookies} cookie${p.cookies === 1 ? '' : 's'}${p.sites.length ? `, signed in at ${p.sites.join(', ')}` : ''}` : 'no saved state';
          const login = p.login ? `; signed in by a person at ${p.login.url}` : '';
          const expiry = p.session?.expired ? `; its session cookies expired ${when(p.session.expiresAt!)}` : p.session?.expiresAt ? `; its session cookies expire ${when(p.session.expiresAt)}` : '';
          return `- ${p.name}: ${saved}${login}${expiry}${p.identity ? '; identity file' : ''}`;
        });
        return { content: [text(list.length ? `### Stored identities in ${store.dir}\n${lines.join('\n')}` : `No stored identities in ${store.dir}.`)], structuredContent: { profiles: list } };
      }
      if (!i.name) throw new ToolInputError(`name is required for ${i.action}`);
      const name = i.name;
      store.paths(name);
      if (i.action === 'save') {
        const tab = await session.tab();
        const state = await tab.entry.context.storageState();
        const { kept } = store.save(name, state);
        const origins = state.origins.filter((o) => o.localStorage.length).map((o) => o.origin);
        const domains = [...new Set(state.cookies.map((c) => (c.domain ?? '').replace(/^\./, '')).filter(Boolean))];
        return {
          content: [
            text(
              `Saved profile ${quote(name)} to ${store.paths(name).state} (owner-only): ${state.cookies.length} cookie${state.cookies.length === 1 ? '' : 's'}${domains.length ? ` for ${domains.join(', ')}` : ''}; localStorage of ${origins.length ? origins.join(', ') : 'no origin'}${kept.length ? `; kept the localStorage of ${kept.join(', ')} from the previous save` : ''}. Load it later with browser_profiles load.`,
            ),
          ],
        };
      }
      if (i.action === 'delete') {
        const { identityKept } = store.delete(name);
        return { content: [text(`Deleted the saved state of profile ${quote(name)}.${identityKept ? ' Its identity file stays.' : ''}`)] };
      }
      const { tab, state, applied, login, session: status } = await session.profileContext(name);
      const cookies = state?.cookies.length ?? 0;
      const origins = state?.origins.filter((o) => o.localStorage.length).map((o) => o.origin) ?? [];
      const loaded = `Loaded profile ${quote(name)} into a new tab [${session.tabs.indexOf(tab)}] with a context of its own: ${cookies} cookie${cookies === 1 ? '' : 's'}${origins.length ? `, localStorage of ${origins.join(', ')}` : ''}${applied.length ? `; identity settings: ${applied.join(', ')}` : ''}.`;
      const command = session.loginCommand(name, login);
      if (status?.expired) {
        const r = await report(session, tab, `${loaded} ${expiredHint(name, `its session cookies expired ${when(status.expiresAt!)}`, command)}`, { snapshot: false });
        return { ...r, isError: true };
      }
      if (!login || session.policy.check(login.url)) {
        const expires = status?.expiresAt ? ` Its session cookies expire ${when(status.expiresAt)}.` : '';
        return report(session, tab, `${loaded}${expires} Navigate to the site to continue as this identity. If it asks you to sign in, the session has expired: do not sign in yourself, ask the person to run \`${command}\`.`, { snapshot: false });
      }
      // Signed in by a person: open the app where they did and read whether the session holds.
      let expired = false;
      const done = (v: unknown) => {
        const c = v as Awaited<ReturnType<typeof checkSession>> | { error: string };
        if ('error' in c) return `${loaded} Opened ${login.url} to check the session, which failed: ${c.error}`;
        if (c.signedIn === false) {
          expired = true;
          return `${loaded} Opened ${login.url} to check the session: ${c.why}. ${expiredHint(name, 'the app does not take it any more', command)}`;
        }
        if (c.signedIn === undefined) return `${loaded} Opened ${login.url} to check the session: ${c.why}. Look at the page: if it asks you to sign in, the session has expired; do not sign in yourself, ask the person to run \`${command}\`.`;
        return `${loaded} Opened ${login.url}: signed in (${c.why}, as when the person signed in${login.at ? ` ${when(login.at)}` : ''}). Continue from this page.`;
      };
      const r = await act(session, tab, done, () => checkSession(tab.page, login, session.navigationTimeout).catch((e) => ({ error: message(e) })), { snapshot: true });
      return expired ? { ...r, isError: true } : r;
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
