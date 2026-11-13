// Stored identities: one signed-in session per name on disk, in the layout of
// the scraping docs ("Stored profiles"):
//
//   <dir>/                mode 700
//     alice.json          storageState (cookies + localStorage), mode 600
//     alice.identity.json how alice looks: userAgent, proxy, locale, ... (optional, written by hand)
//
// The state file is Playwright's `storageState` JSON, so files made by
// Playwright or by `context.storageState({ path })` load here too.
import { chmodSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
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
  deviceScaleFactor?: number;
  isMobile?: boolean;
  hasTouch?: boolean;
  permissions?: string[];
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
      } catch {
        // Listed anyway; loading it reports the problem.
      }
    }
    return info;
  }

  /** The saved session and identity of `name` (either may be missing, not both). */
  read(name: string): { state?: SavedState; identity?: Identity } {
    const p = this.paths(name);
    const state = existsSync(p.state) ? readState(p.state) : undefined;
    const identity = existsSync(p.identity) ? (JSON.parse(readFileSync(p.identity, 'utf8')) as Identity) : undefined;
    if (!state && !identity) throw new Error(`no profile ${JSON.stringify(name)} in ${this.dir}`);
    return { state, identity };
  }

  /**
   * Writes `state` as `name`: owner-only, replaced atomically. Cookies are
   * the context's whole jar; localStorage of origins the context had no page
   * on is kept from the previous save (`storageState()` reads it from open
   * pages only). Returns those kept origins.
   */
  save(name: string, state: SavedState): { kept: string[] } {
    if (this.readOnly) throw new Error('profiles are read-only in this session (--isolated)');
    const p = this.paths(name);
    mkdirSync(this.dir, { recursive: true, mode: 0o700 });
    let kept: string[] = [];
    const origins = [...state.origins];
    if (existsSync(p.state)) {
      try {
        const old = readState(p.state);
        const fresh = new Set(origins.map((o) => o.origin));
        const keep = old.origins.filter((o) => !fresh.has(o.origin) && o.localStorage.length);
        kept = keep.map((o) => o.origin);
        origins.push(...keep);
      } catch {
        // An unreadable old file is replaced.
      }
    }
    const tmp = `${p.state}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify({ cookies: state.cookies, origins }, null, 2), { mode: 0o600 });
    chmodSync(tmp, 0o600);
    renameSync(tmp, p.state);
    return { kept };
  }

  /** Deletes the saved session of `name`; its identity file stays. */
  delete(name: string): { identityKept: boolean } {
    if (this.readOnly) throw new Error('profiles are read-only in this session (--isolated)');
    const p = this.paths(name);
    if (!existsSync(p.state)) throw new Error(`no saved session for profile ${JSON.stringify(name)}`);
    rmSync(p.state);
    return { identityKept: existsSync(p.identity) };
  }
}

function readState(path: string): SavedState {
  const s = JSON.parse(readFileSync(path, 'utf8')) as Partial<SavedState>;
  if (!s || typeof s !== 'object' || (s.cookies && !Array.isArray(s.cookies)) || (s.origins && !Array.isArray(s.origins))) {
    throw new Error(`${path} is not a storageState file ({ cookies, origins })`);
  }
  return { cookies: s.cookies ?? [], origins: (s.origins ?? []).map((o) => ({ origin: o.origin, localStorage: o.localStorage ?? [] })) };
}

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
