// `pyro profile`: stored identities for AI agents, signed in by a person.
//
// `login` reads an emailed sign-in link (without echo, or from stdin), opens
// it in a fresh headless context with the identity's settings, waits until
// the sign-in completes, checks the session in a second fresh context made
// from what it would save (as an agent will load it), and writes the state
// owner-only. The link is never printed or stored; output names origins,
// counts and expiries, never cookie or storage values.
import { existsSync, statSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import { identityKeys, identityOptions, ProfileStore, readIdentity, sessionLine, sessionStatus, when, type Identity, type ProfileInfo, type SavedState } from '../agent/profiles.js';
import { DEFAULT_PROFILE_DIR } from '../agent/session.js';
import { looksLikeSignIn, safeUrl } from '../agent/signin.js';
import { chromium, type Browser } from '../wright/browser.js';
import { urlMatches } from '../wright/match.js';
import type { Page } from '../wright/page.js';

export const HELP = `usage: pyro profile <command> [options]

Stored identities for AI agents (pyro mcp --profile <name>, browser_profiles
load). A person signs an identity in, never the agent: the agent asks you to
run pyro profile login when a session has expired.

commands:
  login <name> --url <url>    sign in with a sign-in link from an email and save the session
  list                        the stored identities: sites, cookie counts, expiries
  show <name>                 one identity, without secrets: files, origins, counts, expiries
  delete <name> [--all]       delete the saved session (--all: and the identity file)

options:
  --profile-dir <dir>         where profiles live (default ~/.pyro/profiles, mode 700)
  --json                      list and show print JSON (still without secrets)

login:
  --url <url>                 the app's start URL (required): opened once signed in, to save
                              its localStorage and to check the session, now and on each load
  --identity <file>           how the identity looks: userAgent, proxy, locale, timezoneId,
                              viewport, deviceScaleFactor, ... (default <name>.identity.json
                              in the profile directory, when it exists); a file given here is
                              saved there as <name>.identity.json
  --wait-for-url <pattern>    the sign-in is done when the URL matches: a glob ("**/dashboard")
                              or /regex/
  --wait-for-selector <css>   the sign-in is done when this element shows (say, the account
                              menu); loads check for it too when the start URL shows it
                              (default: the page left the link and the network is idle)
  --timeout <ms>              how long the sign-in may take (default 60000)
  --ws <url>                  use a running pyro serve instead of launching one

Request a sign-in link for the account, then copy it from the email without
opening it (such links usually work once) and paste it when asked: it is read
without echo. Scripts pipe it on stdin: printf '%s\\n' "$LINK" | pyro profile login ...
The link is never printed or stored. The session is saved to
<profile-dir>/<name>.json (mode 600): cookies and localStorage, Playwright's
storageState format, and the start URL to check it with.

  -h, --help                  this help`;

type Args = { positional: string[]; values: Map<string, string>; flags: Set<string> };

const VALUE_FLAGS = new Set(['url', 'profile-dir', 'identity', 'wait-for-url', 'wait-for-selector', 'timeout', 'ws']);
const BOOL_FLAGS = new Set(['all', 'json', 'help']);

class UsageError extends Error {}
/** The sign-in did not complete: the message says what happened and what to do. */
class SignInFailed extends Error {}

function parse(argv: string[]): Args {
  const args: Args = { positional: [], values: new Map(), flags: new Set() };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '-h') {
      args.flags.add('help');
      continue;
    }
    const m = /^--([a-z-]+)(?:=(.*))?$/s.exec(arg);
    if (!m) {
      if (arg.startsWith('-')) throw new UsageError(`unknown option ${arg} (pyro profile --help)`);
      args.positional.push(arg);
      continue;
    }
    const [, name, inline] = m;
    if (BOOL_FLAGS.has(name)) {
      if (inline !== undefined) throw new UsageError(`--${name} takes no value`);
      args.flags.add(name);
    } else if (VALUE_FLAGS.has(name)) {
      const value = inline ?? argv[++i];
      if (value === undefined) throw new UsageError(`--${name} needs a value`);
      args.values.set(name, value);
    } else {
      throw new UsageError(`unknown option --${name} (pyro profile --help)`);
    }
  }
  return args;
}

export async function main(argv: string[]): Promise<number> {
  let args: Args;
  try {
    args = parse(argv);
  } catch (e) {
    console.error(`pyro profile: ${message(e)}`);
    return 2;
  }
  const [command, name, ...rest] = args.positional;
  if (args.flags.has('help')) {
    console.log(HELP);
    return 0;
  }
  if (!command) {
    console.error(`${HELP}\n\npyro profile: give a command`);
    return 2;
  }
  const store = new ProfileStore(resolve(args.values.get('profile-dir') ?? DEFAULT_PROFILE_DIR));
  try {
    if (rest.length) throw new UsageError(`unexpected argument ${JSON.stringify(rest[0])}`);
    switch (command) {
      case 'login':
        return await login(store, needName(name, command), args);
      case 'list':
        if (name) throw new UsageError(`unexpected argument ${JSON.stringify(name)}`);
        return list(store, args);
      case 'show':
        return show(store, needName(name, command), args);
      case 'delete':
        return remove(store, needName(name, command), args);
      default:
        throw new UsageError(`unknown command ${JSON.stringify(command)}: login, list, show or delete (pyro profile --help)`);
    }
  } catch (e) {
    console.error(`pyro profile ${command}: ${message(e)}`);
    return e instanceof UsageError ? 2 : 1;
  }
}

function needName(name: string | undefined, command: string): string {
  if (!name) throw new UsageError(`${command} needs a profile name: pyro profile ${command} <name>`);
  return name;
}

// ---- login ----

const AGAIN = 'The link may have expired or been used already (such links usually work once): request a new one and run this command again, pasting the link without opening it first.';

async function login(store: ProfileStore, name: string, args: Args): Promise<number> {
  const paths = store.paths(name);
  const startUrl = args.values.get('url');
  if (!startUrl) throw new UsageError("--url <the app's start URL> is required: it is opened once signed in, to check the session");
  if (!/^https?:\/\//i.test(startUrl) || !URL.canParse(startUrl)) throw new UsageError(`--url takes an http(s) URL, got ${JSON.stringify(startUrl)}`);
  const timeout = args.values.has('timeout') ? Number(args.values.get('timeout')) : 60_000;
  if (!Number.isInteger(timeout) || timeout <= 0) throw new UsageError(`--timeout takes milliseconds, got ${JSON.stringify(args.values.get('timeout'))}`);
  const waitForUrl = args.values.get('wait-for-url');
  const pattern = waitForUrl === undefined ? undefined : urlPattern(waitForUrl);
  const waitForSelector = args.values.get('wait-for-selector');
  const identityFile = args.values.get('identity');
  let identity: Identity | undefined;
  let identityFrom: string | undefined;
  if (identityFile !== undefined) {
    identityFrom = resolve(identityFile);
    if (!existsSync(identityFrom)) throw new UsageError(`--identity: no such file: ${identityFile}`);
    identity = readIdentity(identityFrom);
  } else if (existsSync(paths.identity)) {
    identityFrom = paths.identity;
    identity = readIdentity(paths.identity);
  }

  const raw = await readLink();
  if (!raw) throw new UsageError('no sign-in link was given');
  const link = URL.canParse(raw) ? new URL(raw) : undefined;
  if (!link || !/^https?:$/.test(link.protocol)) throw new UsageError('that is not an http(s) link: paste the whole sign-in link from the email (it starts with https://)');
  const redact = redactor(link);

  const progress = (line: string) => process.stderr.write(`pyro profile login: ${line}\n`);
  const ws = args.values.get('ws');
  const browser = ws ? await chromium.connectOverCDP(ws) : await chromium.launch({ stderr: 'ignore' });
  try {
    progress(`opening the sign-in link (on ${link.host}) in a fresh headless browser${identityFrom ? `, as ${basename(identityFrom)} says` : ''}`);
    const done = await signIn(browser, { link, startUrl, pattern, waitForUrl, waitForSelector, timeout, identity, progress });
    store.save(name, done.state, { merge: false, login: { url: startUrl, landing: done.landing, ...(done.selector ? { selector: done.selector } : {}), at: new Date().toISOString() } });
    let identityNote = '';
    if (identity && identityFile !== undefined) {
      const replaced = existsSync(paths.identity);
      store.saveIdentity(name, identity);
      identityNote = ` (from ${identityFile}, saved as ${basename(paths.identity)}${replaced ? ', replacing the one there' : ''})`;
    } else if (identity) {
      identityNote = ` (${basename(paths.identity)})`;
    }
    const cookieSites = countBy(done.state.cookies.map((c) => (c.domain ?? '').replace(/^\./, '') || 'its URL'));
    const storage = done.state.origins.filter((o) => o.localStorage.length);
    const lines = [
      `Saved stored identity ${JSON.stringify(name)} to ${paths.state} (mode 600, in a mode 700 directory).`,
      `  signed in:     the link led to ${done.linkLanding}`,
      `  checked:       a fresh browser context with the saved session opened ${safeUrl(startUrl)}${done.landing !== safeUrl(startUrl) ? ` (it landed on ${done.landing})` : ''}, not a sign-in page${done.selector ? `, and shows ${JSON.stringify(done.selector)}` : ''}`,
      `  cookies:       ${done.state.cookies.length}${cookieSites.length ? ` (${cookieSites.join(', ')})` : ''}`,
      `  session:       ${sessionLine(sessionStatus(done.state))}`,
      `  localStorage:  ${storage.length ? storage.map((o) => `${o.origin} (${o.localStorage.length} key${o.localStorage.length === 1 ? '' : 's'})`).join(', ') : 'none'}`,
      ...(identity ? [`  identity:      ${identityKeys(identity).join(', ') || 'no settings'}${identityNote}`] : []),
      ...done.notes.map((n) => `  note:          ${n}`),
      `An agent loads it with browser_profiles { "action": "load", "name": "${name}" }, or starts in it with pyro mcp --profile ${name}${resolve(store.dir) === resolve(DEFAULT_PROFILE_DIR) ? '' : ` --profile-dir ${store.dir}`}.`,
    ];
    console.log(redact(lines.join('\n')));
    return 0;
  } catch (e) {
    const text = redact(message(e));
    console.error(`pyro profile login: ${e instanceof SignInFailed ? text : `the sign-in failed: ${text}`}`);
    return 1;
  } finally {
    await browser.close().catch(() => {});
  }
}

type SignInOptions = {
  link: URL;
  startUrl: string;
  pattern?: string | RegExp;
  waitForUrl?: string;
  waitForSelector?: string;
  timeout: number;
  identity?: Identity;
  progress: (line: string) => void;
};

/**
 * Opens the link, waits until the sign-in completes, opens the start URL
 * (so its origin's localStorage is saved), then checks a fresh context made
 * from that state: what it holds after the start URL loaded is what is saved.
 */
async function signIn(browser: Browser, o: SignInOptions): Promise<{ state: SavedState; landing: string; linkLanding: string; selector?: string; notes: string[] }> {
  const options = o.identity ? identityOptions(o.identity) : {};
  const deadline = Date.now() + o.timeout;
  const left = () => Math.max(1, deadline - Date.now());
  const seconds = `${Math.round(o.timeout / 1000)} s`;
  const onLink = (url: string) => withoutHash(url) === withoutHash(o.link.href);
  const notes: string[] = [];

  const first = await browser.newContext(options);
  const page = await first.newPage();
  const statuses = documentStatuses(page);
  try {
    await page.goto(o.link.href, { timeout: left() });
  } catch (e) {
    throw new SignInFailed(`the sign-in link did not open: ${message(e)}`);
  }
  try {
    if (o.pattern !== undefined) await page.waitForURL(o.pattern, { timeout: left() });
    if (o.waitForSelector !== undefined) await page.locator(o.waitForSelector).first().waitFor({ state: 'visible', timeout: left() });
    if (o.pattern === undefined && o.waitForSelector === undefined) {
      await page.waitForURL((u) => !onLink(u.href), { timeout: left() });
      await page.waitForLoadState('networkidle', { timeout: left() });
    }
  } catch {
    const what =
      o.pattern !== undefined && !(await urlMatches(undefined, page.url(), o.pattern))
        ? `the URL never matched ${o.waitForUrl}`
        : o.waitForSelector !== undefined
          ? `${JSON.stringify(o.waitForSelector)} never showed`
          : onLink(page.url())
            ? 'the page stayed on the sign-in link'
            : 'the page never went quiet';
    throw new SignInFailed(`the sign-in did not complete within ${seconds}: ${what}; ${await where(page, onLink)}. ${AGAIN}`);
  }
  const landed = page.url();
  if (o.pattern === undefined && o.waitForSelector === undefined) {
    const status = statuses.get(withoutHash(landed));
    const hint = ' If that is the signed-in app, say what signed in looks like with --wait-for-url or --wait-for-selector.';
    if (status !== undefined && status >= 400) throw new SignInFailed(`the sign-in did not complete: the link led to a page that answered HTTP ${status}; ${await where(page, onLink)}. ${AGAIN}`);
    if (looksLikeSignIn(landed)) throw new SignInFailed(`the sign-in did not complete: the link led to a sign-in or error page; ${await where(page, onLink)}. ${AGAIN}${hint}`);
  }
  const linkLanding = onLink(landed) ? 'a page at the link itself' : safeUrl(landed);
  o.progress(`signed in: the link led to ${linkLanding}`);

  try {
    await page.goto(o.startUrl, { timeout: left() });
  } catch (e) {
    throw new SignInFailed(`signed in, but ${safeUrl(o.startUrl)} (--url) did not open: ${message(e)}`);
  }
  await page.waitForLoadState('networkidle', { timeout: Math.min(left(), 15_000) }).catch(() => {});
  const signedIn = await first.storageState();
  await first.close();
  if (!signedIn.cookies.length && !signedIn.origins.some((x) => x.localStorage.length)) {
    throw new SignInFailed(`the sign-in left no cookies and no localStorage, so there is no session to save. ${AGAIN}`);
  }

  o.progress(`checking the session in a fresh context at ${safeUrl(o.startUrl)}`);
  const second = await browser.newContext({ ...options, storageState: signedIn });
  try {
    const check = await second.newPage();
    const response = await check.goto(o.startUrl, { timeout: left() }).catch((e) => {
      throw new SignInFailed(`the session check could not open ${safeUrl(o.startUrl)}: ${message(e)}`);
    });
    await check.waitForLoadState('networkidle', { timeout: Math.min(left(), 15_000) }).catch(() => {});
    const status = response?.status();
    const landing = safeUrl(check.url());
    const keptElsewhere = 'The app keeps its session where a stored identity cannot hold it (sessionStorage, IndexedDB or memory), or the start URL is on another site than the sign-in.';
    if (status === 401 || status === 403) throw new SignInFailed(`signed in, but a fresh context with the saved cookies and localStorage got HTTP ${status} at ${landing}. ${keptElsewhere}`);
    if (looksLikeSignIn(check.url())) throw new SignInFailed(`signed in, but a fresh context with the saved cookies and localStorage landed on ${landing}, a sign-in page. ${keptElsewhere}`);
    let selector: string | undefined;
    if (o.waitForSelector !== undefined) {
      const shown = await check.locator(o.waitForSelector).first().waitFor({ state: 'visible', timeout: 5_000 }).then(() => true, () => false);
      if (shown) selector = o.waitForSelector;
      else notes.push(`${landing} does not show ${JSON.stringify(o.waitForSelector)}: loads check where the start URL lands, not for that element`);
    }
    const fresh = await second.storageState();
    // Origins only the first context had a page on keep their localStorage.
    const origins = [
      ...fresh.origins.filter((x) => x.localStorage.length),
      ...signedIn.origins.filter((x) => x.localStorage.length && !fresh.origins.some((y) => y.origin === x.origin)),
    ];
    return { state: { cookies: fresh.cookies, origins }, landing, linkLanding, selector, notes };
  } finally {
    await second.close().catch(() => {});
  }
}

/** Status of each document the page loaded, by URL (without fragment). */
function documentStatuses(page: Page): Map<string, number> {
  const statuses = new Map<string, number>();
  page.on('response', (r) => {
    if (r.request().isNavigationRequest()) statuses.set(withoutHash(r.url()), r.status());
  });
  return statuses;
}

/** Where the page is, to print: never the link's URL (it is the secret), and the title. */
async function where(page: Page, onLink: (url: string) => boolean): Promise<string> {
  const title = await page.title().catch(() => '');
  const at = onLink(page.url()) ? 'the page is at the sign-in link' : `the page is at ${safeUrl(page.url())}`;
  return `${at}${title ? ` (title ${JSON.stringify(title.slice(0, 120))})` : ''}`;
}

/** `--wait-for-url`: `/regex/flags`, else a glob. */
function urlPattern(text: string): string | RegExp {
  const m = /^\/(.+)\/([a-z]*)$/s.exec(text);
  if (!m) return text;
  try {
    return new RegExp(m[1], m[2]);
  } catch (e) {
    throw new UsageError(`--wait-for-url: not a regular expression: ${message(e)}`);
  }
}

const withoutHash = (url: string) => url.replace(/#.*$/s, '');

/**
 * Replaces the link and its parts (query values, fragment, long path
 * segments) in text that may quote them: browser errors name the URL they
 * were loading.
 */
function redactor(link: URL): (text: string) => string {
  const parts = new Set<string>();
  const add = (v: string) => {
    if (v.length < 6) return;
    parts.add(v);
    parts.add(encodeURIComponent(v));
    try {
      parts.add(decodeURIComponent(v));
    } catch {
      // not encoded
    }
  };
  add(link.href);
  add(withoutHash(link.href));
  add(link.pathname + link.search);
  add(link.search.slice(1));
  add(link.hash.slice(1));
  for (const [, v] of link.searchParams) add(v);
  for (const seg of link.pathname.split('/')) if (seg.length >= 16) add(seg);
  const list = [...parts].sort((a, b) => b.length - a.length);
  return (text) => list.reduce((out, part) => out.split(part).join('[redacted]'), text);
}

/** The sign-in link: typed or pasted without echo at a terminal, else the first line of stdin. */
async function readLink(): Promise<string> {
  const stdin = process.stdin;
  if (!stdin.isTTY) {
    process.stderr.write('pyro profile login: reading the sign-in link from stdin\n');
    let data = '';
    stdin.setEncoding('utf8');
    for await (const chunk of stdin) {
      data += chunk;
      if (data.length > 65_536 || /\S[^\n]*\n/.test(data)) break;
    }
    return data.split(/\r?\n/).map((l) => l.trim()).find(Boolean) ?? '';
  }
  return readHidden('Paste the sign-in link from the email (it is not shown), then press Enter: ');
}

/** A line read from the terminal in raw mode, not echoed. Ctrl-C aborts (exit 130). */
function readHidden(prompt: string): Promise<string> {
  const stdin = process.stdin;
  // Raw (no echo) before the prompt shows: what is typed once it shows is never echoed.
  stdin.setRawMode(true);
  process.stderr.write(prompt);
  return new Promise((done) => {
    let value = '';
    const finish = (abort: boolean) => {
      stdin.off('data', onData);
      stdin.setRawMode(false);
      stdin.pause();
      process.stderr.write('\n');
      if (abort) {
        process.stderr.write('pyro profile login: cancelled\n');
        process.exit(130);
      }
      done(value.trim());
    };
    const onData = (chunk: string) => {
      // Terminal escape sequences (bracketed paste markers, arrow keys) are not part of the link.
      const clean = chunk.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[@-_]/g, '');
      for (const ch of clean) {
        if (ch === '\r' || ch === '\n') return finish(false);
        if (ch === '\u0003') return finish(true);
        if (ch === '\u0004') return finish(false);
        if (ch === '\u007f' || ch === '\b') value = value.slice(0, -1);
        else if (ch === '\u0015') value = '';
        else if (ch >= ' ') value += ch;
      }
    };
    stdin.setEncoding('utf8');
    stdin.on('data', onData);
    stdin.resume();
  });
}

// ---- list, show, delete ----

function list(store: ProfileStore, args: Args): number {
  const all = store.list();
  if (args.flags.has('json')) {
    console.log(JSON.stringify({ dir: store.dir, profiles: all }, null, 2));
    return 0;
  }
  if (!all.length) {
    console.log(`No stored identities in ${store.dir}. A person signs one in with: pyro profile login <name> --url <app URL>`);
    return 0;
  }
  console.log(`Stored identities in ${store.dir}:`);
  for (const p of all) console.log(`  ${p.name}: ${summary(p)}`);
  return 0;
}

function summary(p: ProfileInfo): string {
  const parts: string[] = [];
  if (!p.state) parts.push('no saved session');
  else {
    parts.push(`saved ${when(p.savedAt!)}`);
    parts.push(`${p.cookies} cookie${p.cookies === 1 ? '' : 's'}${p.sites.length ? ` (${p.sites.join(', ')})` : ''}`);
    if (p.login) parts.push(`signed in by a person at ${p.login.url}`);
    if (p.session?.expired) parts.push(`EXPIRED: its session cookies expired ${when(p.session.expiresAt!)}`);
    else if (p.session?.expiresAt) parts.push(`its session cookies expire ${when(p.session.expiresAt)}`);
  }
  if (p.identity) parts.push('identity file');
  return parts.join('; ');
}

function show(store: ProfileStore, name: string, args: Args): number {
  const paths = store.paths(name);
  const info = store.info(name);
  if (!info.state && !info.identity) throw new Error(`no profile ${JSON.stringify(name)} in ${store.dir}`);
  const { state, identity } = store.read(name);
  const cookies = state ? countBy(state.cookies.map((c) => (c.domain ?? '').replace(/^\./, '') || 'its URL')) : [];
  const storage = state?.origins.filter((o) => o.localStorage.length) ?? [];
  if (args.flags.has('json')) {
    const shown = { ...info, dir: store.dir, modes: modes(paths, store.dir), localStorage: storage.map((o) => ({ origin: o.origin, keys: o.localStorage.length })), identitySettings: identity ? identityKeys(identity) : [] };
    console.log(JSON.stringify(shown, null, 2));
    return 0;
  }
  const lines = [`Stored identity ${JSON.stringify(name)} in ${store.dir} (${modeNote(store.dir, 0o700)})`];
  if (info.state && state) {
    lines.push(`  state file:    ${basename(paths.state)} (${modeNote(paths.state, 0o600)}), saved ${when(info.savedAt!)}`);
    if (info.login) {
      lines.push(
        `  signed in:     by a person at ${info.login.url}${info.login.at ? ` ${when(info.login.at)}` : ''}; it landed on ${info.login.landing}${info.login.selector ? `; loads check for ${JSON.stringify(info.login.selector)}` : ''}`,
      );
    }
    lines.push(`  cookies:       ${state.cookies.length}${cookies.length ? ` (${cookies.join(', ')})` : ''}`);
    lines.push(`  session:       ${sessionLine(sessionStatus(state))}`);
    lines.push(`  localStorage:  ${storage.length ? storage.map((o) => `${o.origin} (${o.localStorage.length} key${o.localStorage.length === 1 ? '' : 's'})`).join(', ') : 'none'}`);
  } else {
    lines.push('  state file:    none (no saved session)');
  }
  lines.push(identity ? `  identity file: ${basename(paths.identity)} (${modeNote(paths.identity, 0o600)}): ${identityKeys(identity).join(', ') || 'no settings'}` : '  identity file: none');
  console.log(lines.join('\n'));
  return 0;
}

function remove(store: ProfileStore, name: string, args: Args): number {
  const { identityKept, identityDeleted } = store.delete(name, { all: args.flags.has('all') });
  console.log(`Deleted the saved session of ${JSON.stringify(name)}.${identityKept ? ' Its identity file stays (--all deletes it too).' : identityDeleted ? ' Deleted its identity file too.' : ''}`);
  return 0;
}

function modes(paths: { state: string; identity: string }, dir: string): Record<string, string> {
  const mode = (p: string) => (existsSync(p) ? (statSync(p).mode & 0o777).toString(8) : 'missing');
  return { dir: mode(dir), state: mode(paths.state), identity: mode(paths.identity) };
}

/** "mode 600", or a warning when others may read it. */
function modeNote(path: string, want: number): string {
  const mode = statSync(path).mode & 0o777;
  return mode & 0o077 ? `mode ${mode.toString(8)}: others may read it, chmod ${want.toString(8)} it` : `mode ${mode.toString(8)}`;
}

/** "a.com: 2, b.com: 1". */
function countBy(items: string[]): string[] {
  const counts = new Map<string, number>();
  for (const i of items) counts.set(i, (counts.get(i) ?? 0) + 1);
  return [...counts].map(([k, n]) => `${k}: ${n}`);
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
