// Stored identities: one signed-in session per name on disk, in the layout of
// the scraping docs ("Stored profiles"):
//
//   <dir>/                mode 700
//     alice.json          storageState (cookies + localStorage), mode 600
//     alice.identity.json how alice looks: userAgent, proxy, locale, ... (optional, written by hand)
//
// The state file is Playwright's `storageState` JSON, so files made by
// Playwright or by `context.storageState({ path })` load here too. A state
// saved by `pyro profile login` also holds a `pyro` key (where the person
// signed in, to check the session on load); Playwright ignores it.
import { chmodSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BrowserContextOptions } from '../wright/browser.js';

/** Playwright's `storageState`: the cookie jar and each origin's localStorage. */
export type SavedState = Required<Exclude<NonNullable<BrowserContextOptions['storageState']>, string>>;

/** `<name>.identity.json`: the context options of an identity. */
export type Identity = {
  userAgent?: string;
  viewport?: { width: number; height: number };
  extraHTTPHeaders?: Record<string, string>;
  locale?: string;
  timezoneId?: string;
  geolocation?: { latitude: number; longitude: number; accuracy?: number };
  /** `http://user:pass@host:port`, or Playwright's `{ server, bypass, username, password }`. */
  proxy?: string | { server: string; bypass?: string; username?: string; password?: string };
  /** Hosts that skip `proxy` (Chrome's bypass list syntax). */
  proxyBypass?: string;
  colorScheme?: 'light' | 'dark' | 'no-preference';
  /** Device pixels per CSS pixel (2056x1200 at 3 renders 6168x3600 screenshots). */
  deviceScaleFactor?: number;
  isMobile?: boolean;
  hasTouch?: boolean;
  permissions?: string[];
};

/**
 * How a session was signed in by `pyro profile login`, kept in the state
 * file under `pyro.login`: where to check that it still holds. No secrets:
 * the sign-in link is never stored, and URLs keep no query or fragment.
 */
export type LoginInfo = {
  /** The app's start URL the person gave (`--url`). */
  url: string;
  /** Where `url` landed when signed in: origin and path. */
  landing: string;
  /** An element visible only when signed in (`--wait-for-selector`, when `url` shows it). */
  selector?: string;
  /** When the person signed in (ISO time). */
  at: string;
};

type Cookie = SavedState['cookies'][number];

/** The cookies that look like they hold the sign-in, and whether they have run out. */
export type SessionStatus = {
  cookies: number;
  /** Cookies named like a session or token (else the httpOnly ones): names and expiries only. */
  session: { name: string; domain: string; httpOnly: boolean; expires?: string }[];
  /** The latest expiry among those cookies (ISO), when none lasts for the browser session only. */
  expiresAt?: string;
  /** Every one of those cookies has expired. */
  expired: boolean;
};

export type ProfileInfo = {
  name: string;
  /** A saved session (`<name>.json`). */
  state: boolean;
  /** An identity file (`<name>.identity.json`). */
  identity: boolean;
  savedAt?: string;
  cookies: number;
  /** Cookie domains and localStorage origins: where this identity is signed in. */
  sites: string[];
  /** Signed in by a person with `pyro profile login`: where and when. */
  login?: LoginInfo;
  /** Expiry of the cookies that look like the session's. */
  session?: SessionStatus;
};

const NAME = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,99}$/;

export class ProfileStore {
  constructor(
    readonly dir: string,
    /** `--isolated`: profiles load, nothing is written. */
    readonly readOnly = false,
  ) {}

  paths(name: string): { state: string; identity: string } {
    if (!NAME.test(name) || name.endsWith('.identity')) {
      throw new Error(`bad profile name ${JSON.stringify(name)}: use letters, digits, '_', '-' and '.' (not starting with '.', not ending in '.identity')`);
    }
    return { state: join(this.dir, `${name}.json`), identity: join(this.dir, `${name}.identity.json`) };
  }

  list(): ProfileInfo[] {
    if (!existsSync(this.dir)) return [];
    const names = new Set<string>();
    for (const f of readdirSync(this.dir)) {
      const m = /^(.+?)(\.identity)?\.json$/.exec(f);
      if (m && NAME.test(m[1])) names.add(m[1]);
    }
    return [...names].sort().map((name) => this.info(name));
  }

  info(name: string): ProfileInfo {
    const p = this.paths(name);
    const state = existsSync(p.state);
    const info: ProfileInfo = { name, state, identity: existsSync(p.identity), cookies: 0, sites: [] };
    if (state) {
      info.savedAt = statSync(p.state).mtime.toISOString();
      try {
        const s = readState(p.state);
        info.cookies = s.cookies.length;
        info.sites = [...new Set([...s.cookies.map((c) => (c.domain ?? c.url ?? '').replace(/^\./, '')), ...s.origins.filter((o) => o.localStorage.length).map((o) => o.origin)])].filter(Boolean);
        info.login = readLogin(p.state);
        info.session = sessionStatus(s);
      } catch {
        // Listed anyway; loading it reports the problem.
      }
    }
    return info;
  }

  /** The saved session, identity and sign-in record of `name` (state or identity may be missing, not both). */
  read(name: string): { state?: SavedState; identity?: Identity; login?: LoginInfo } {
    const p = this.paths(name);
    const state = existsSync(p.state) ? readState(p.state) : undefined;
    const identity = existsSync(p.identity) ? readIdentity(p.identity) : undefined;
    if (!state && !identity) throw new Error(`no profile ${JSON.stringify(name)} in ${this.dir}`);
    return { state, identity, login: state ? readLogin(p.state) : undefined };
  }

  /**
   * Writes `state` as `name`: owner-only, replaced atomically, in an
   * owner-only directory. Cookies are the context's whole jar. With `merge`
   * (the default) localStorage of origins the context had no page on is kept
   * from the previous save (`storageState()` reads it from open pages only),
   * and so is the sign-in record; `login` replaces the record. Returns the
   * kept origins.
   */
  save(name: string, state: SavedState, o: { merge?: boolean; login?: LoginInfo } = {}): { kept: string[] } {
    if (this.readOnly) throw new Error('profiles are read-only in this session (--isolated)');
    const p = this.paths(name);
    this.ensureDir();
    let kept: string[] = [];
    const origins = [...state.origins];
    let login = o.login;
    if ((o.merge ?? true) && existsSync(p.state)) {
      try {
        const old = readState(p.state);
        const fresh = new Set(origins.map((x) => x.origin));
        const keep = old.origins.filter((x) => !fresh.has(x.origin) && x.localStorage.length);
        kept = keep.map((x) => x.origin);
        origins.push(...keep);
        login ??= readLogin(p.state);
      } catch {
        // An unreadable old file is replaced.
      }
    }
    writePrivate(p.state, { cookies: state.cookies, origins, ...(login ? { pyro: { login } } : {}) });
    return { kept };
  }

  /** Writes `identity` as `name`'s identity file (owner-only: a proxy URL may hold credentials). */
  saveIdentity(name: string, identity: Identity): void {
    if (this.readOnly) throw new Error('profiles are read-only in this session (--isolated)');
    this.ensureDir();
    writePrivate(this.paths(name).identity, identity);
  }

  /** Deletes the saved session of `name`; its identity file stays unless `all`. */
  delete(name: string, o: { all?: boolean } = {}): { identityKept: boolean; identityDeleted: boolean } {
    if (this.readOnly) throw new Error('profiles are read-only in this session (--isolated)');
    const p = this.paths(name);
    const hasIdentity = existsSync(p.identity);
    if (!existsSync(p.state) && !(o.all && hasIdentity)) throw new Error(`no saved session for profile ${JSON.stringify(name)}`);
    if (existsSync(p.state)) rmSync(p.state);
    if (o.all && hasIdentity) rmSync(p.identity);
    return { identityKept: hasIdentity && !o.all, identityDeleted: hasIdentity && !!o.all };
  }

  /** The directory, made owner-only (and tightened when it is ours and others could read it). */
  private ensureDir(): void {
    mkdirSync(this.dir, { recursive: true, mode: 0o700 });
    const st = lstatSync(this.dir);
    if (st.isSymbolicLink()) return;
    if ((st.mode & 0o077) !== 0 && (!process.getuid || st.uid === process.getuid())) chmodSync(this.dir, 0o700);
  }
}

/** `value` as JSON in `path`, mode 600, replacing the file atomically. */
function writePrivate(path: string, value: unknown): void {
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(value, null, 2), { mode: 0o600 });
  chmodSync(tmp, 0o600);
  renameSync(tmp, path);
}

function readState(path: string): SavedState {
  const s = JSON.parse(readFileSync(path, 'utf8')) as Partial<SavedState>;
  if (!s || typeof s !== 'object' || (s.cookies && !Array.isArray(s.cookies)) || (s.origins && !Array.isArray(s.origins))) {
    throw new Error(`${path} is not a storageState file ({ cookies, origins })`);
  }
  return { cookies: s.cookies ?? [], origins: (s.origins ?? []).map((o) => ({ origin: o.origin, localStorage: o.localStorage ?? [] })) };
}

function readLogin(path: string): LoginInfo | undefined {
  try {
    const l = (JSON.parse(readFileSync(path, 'utf8')) as { pyro?: { login?: Partial<LoginInfo> } }).pyro?.login;
    if (!l || typeof l.url !== 'string' || typeof l.landing !== 'string') return undefined;
    return { url: l.url, landing: l.landing, ...(typeof l.selector === 'string' ? { selector: l.selector } : {}), at: typeof l.at === 'string' ? l.at : '' };
  } catch {
    return undefined;
  }
}

const IDENTITY_KEYS: Record<keyof Identity, string> = {
  userAgent: 'string',
  viewport: 'object',
  extraHTTPHeaders: 'object',
  locale: 'string',
  timezoneId: 'string',
  geolocation: 'object',
  proxy: 'string or object',
  proxyBypass: 'string',
  colorScheme: 'string',
  deviceScaleFactor: 'number',
  isMobile: 'boolean',
  hasTouch: 'boolean',
  permissions: 'array',
};

/** An identity file, checked: a mistake in it fails here, naming the field, not later in the browser. */
export function readIdentity(path: string): Identity {
  let raw: unknown;
  try {
    raw = JSON.parse(readFileSync(path, 'utf8'));
  } catch (e) {
    throw new Error(`${path} is not JSON: ${e instanceof Error ? e.message : e}`);
  }
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error(`${path}: an identity file is a JSON object`);
  const id = raw as Record<string, unknown>;
  const bad = (field: string, want: string) => new Error(`${path}: ${field} must be ${want}`);
  for (const [k, v] of Object.entries(id)) {
    const want = IDENTITY_KEYS[k as keyof Identity];
    if (!want || v === undefined || v === null) continue; // unknown keys are ignored, as by newContext()
    const type = Array.isArray(v) ? 'array' : typeof v;
    if (!want.split(' or ').includes(type)) throw bad(k, `a${/^[aeiou]/.test(want) ? 'n' : ''} ${want}`);
  }
  const vp = id.viewport as { width?: unknown; height?: unknown } | undefined;
  if (vp && !(isSide(vp.width) && isSide(vp.height))) throw bad('viewport', '{ "width": <CSS px>, "height": <CSS px> }, each 100 to 8000');
  const dsf = id.deviceScaleFactor as number | undefined;
  if (dsf !== undefined && !(dsf >= 0.5 && dsf <= 8)) throw bad('deviceScaleFactor', 'a number from 0.5 to 8');
  return id as Identity;
}

const isSide = (v: unknown) => Number.isInteger(v) && (v as number) >= 100 && (v as number) <= 8000;

/** An identity file as `browser.newContext()` options. */
export function identityOptions(identity: Identity): BrowserContextOptions {
  const o: BrowserContextOptions = {};
  if (identity.userAgent) o.userAgent = identity.userAgent;
  if (identity.viewport) o.viewport = identity.viewport;
  if (identity.extraHTTPHeaders) o.extraHTTPHeaders = identity.extraHTTPHeaders;
  if (identity.locale) o.locale = identity.locale;
  if (identity.timezoneId) o.timezoneId = identity.timezoneId;
  if (identity.geolocation) o.geolocation = identity.geolocation;
  if (identity.colorScheme) o.colorScheme = identity.colorScheme;
  if (identity.deviceScaleFactor) o.deviceScaleFactor = identity.deviceScaleFactor;
  if (identity.isMobile !== undefined) o.isMobile = identity.isMobile;
  if (identity.hasTouch !== undefined) o.hasTouch = identity.hasTouch;
  if (identity.permissions) o.permissions = identity.permissions;
  if (identity.proxy) o.proxy = proxyOption(identity.proxy, identity.proxyBypass);
  return o;
}

/** A proxy URL (credentials in it) or Playwright's object, as the `proxy` context option. */
export function proxyOption(proxy: NonNullable<Identity['proxy']>, bypass?: string): NonNullable<BrowserContextOptions['proxy']> {
  if (typeof proxy !== 'string') return { ...proxy, ...(bypass && !proxy.bypass ? { bypass } : {}) };
  let url: URL;
  try {
    url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(proxy) ? proxy : `http://${proxy}`);
  } catch {
    throw new Error(`not a proxy URL: ${proxy}`);
  }
  const out: NonNullable<BrowserContextOptions['proxy']> = { server: `${url.protocol}//${url.host}` };
  if (url.username) out.username = decodeURIComponent(url.username);
  if (url.password) out.password = decodeURIComponent(url.password);
  if (bypass) out.bypass = bypass;
  return out;
}

/** Which identity settings `identity` sets (names only: values may hold credentials). */
export function identityKeys(identity: Identity): string[] {
  return Object.keys(identityOptions(identity));
}

/** Cookie names that usually hold a sign-in. */
const SESSION_NAME = /sess|sid|auth|token|jwt|login|remember|refresh/i;

/**
 * The cookies of `state` that look like the session's (named like one, else
 * the httpOnly ones, else all) and whether all of them have expired. Names
 * and expiries only: values never leave the file.
 */
export function sessionStatus(state: SavedState, now = Date.now()): SessionStatus {
  const named = state.cookies.filter((c) => SESSION_NAME.test(c.name));
  const httpOnly = state.cookies.filter((c) => c.httpOnly);
  const picked: Cookie[] = named.length ? named : httpOnly.length ? httpOnly : state.cookies;
  const persistent = (c: Cookie) => c.expires !== undefined && c.expires > 0;
  const session = picked.map((c) => ({
    name: c.name,
    domain: (c.domain ?? (c.url ? new URL(c.url).hostname : '')).replace(/^\./, ''),
    httpOnly: !!c.httpOnly,
    ...(persistent(c) ? { expires: new Date(c.expires! * 1000).toISOString() } : {}),
  }));
  const allPersistent = picked.length > 0 && picked.every(persistent);
  const latest = allPersistent ? Math.max(...picked.map((c) => c.expires!)) : undefined;
  return {
    cookies: state.cookies.length,
    session,
    ...(latest !== undefined ? { expiresAt: new Date(latest * 1000).toISOString() } : {}),
    expired: latest !== undefined && latest * 1000 <= now,
  };
}

/** One line on a session's cookies: "sid (httpOnly) expires 2026-12-13 10:00 UTC", "connect.sid lasts for the browser session". */
export function sessionLine(status: SessionStatus, now = Date.now()): string {
  if (!status.session.length) return status.cookies ? 'no cookie looks like a session cookie' : 'no cookies';
  const shown = status.session.slice(0, 4).map((c) => `${c.name}${c.httpOnly ? ' (httpOnly)' : ''} for ${c.domain || 'its URL'} ${c.expires ? `${Date.parse(c.expires) <= now ? 'expired' : 'expires'} ${when(c.expires, now)}` : 'lasts for the browser session (no expiry)'}`);
  const more = status.session.length > 4 ? `, and ${status.session.length - 4} more` : '';
  return `session cookie${status.session.length === 1 ? '' : 's'}: ${shown.join('; ')}${more}`;
}

/** An ISO time as "2026-12-13 10:00 UTC (in 30 days)". */
export function when(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  const day = 86_400_000;
  const d = t - now;
  const rel = Math.abs(d) < 3_600_000 ? `${Math.round(Math.abs(d) / 60_000)} min` : Math.abs(d) < 2 * day ? `${Math.round(Math.abs(d) / 3_600_000)} h` : `${Math.round(Math.abs(d) / day)} days`;
  return `${iso.slice(0, 16).replace('T', ' ')} UTC (${d >= 0 ? `in ${rel}` : `${rel} ago`})`;
}
