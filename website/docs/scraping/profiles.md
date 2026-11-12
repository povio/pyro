# Stored profiles

A stored profile is one identity on disk: the cookies and `localStorage` of a signed-in session, saved to a JSON file, plus a small file that says how that identity looks (user agent, viewport, proxy, locale). You sign in once, save, and every later run starts from the file, already signed in. One file per account lets you scrape as many identities as you have, each isolated from the others.

This page uses the Playwright library (`@povio/lightning-panda/playwright`) and its `storageState` format, which is the same JSON Playwright writes. For profiles that the server boots and signs in once and then forks per job, see [Profiles booted once (fork)](/scraping/fork-profiles).

::: warning There is no persistent profile directory
Pyro keeps all browser storage in memory: cookies, `localStorage`, `sessionStorage`, IndexedDB and the HTTP cache live in a browser context and are gone when it closes. There is no `--user-data-dir` flag and no `launchPersistentContext`. A profile persists only as the JSON file you save with `context.storageState()`.
:::

## What a profile file holds

`context.storageState()` returns, and with `{ path }` writes, this shape:

```json
{
  "cookies": [
    {
      "name": "sid",
      "value": "f77fa7d6-eafe-4071-90c0-d85ac2ebcf81",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": 1794509897.088918,
      "httpOnly": true,
      "secure": false,
      "sameSite": "Lax"
    }
  ],
  "origins": [
    {
      "origin": "http://127.0.0.1:4500",
      "localStorage": [{ "name": "apiToken", "value": "1137c7b4-1c1a-438b-b9e1-3f8c8f3fe3a0" }]
    }
  ]
}
```

| Saved | Not saved |
| --- | --- |
| Every cookie in the context's jar, `httpOnly` ones included, with expiry | `sessionStorage` |
| `localStorage` of the origins of the context's open pages | IndexedDB (the `indexedDB: true` option is accepted and ignored) |
| | The HTTP cache, service workers, permissions |

Two consequences:

- **Save while a page of each origin you care about is open.** `localStorage` is read from the open pages, one per origin. If the app keeps its token on `app.example.com` and your page is on `www.example.com`, open a page on `app.example.com` before you save.
- **Apps that keep their session in IndexedDB or `sessionStorage` do not round-trip.** For those, sign in again per run, or use a [fork profile](/scraping/fork-profiles), which keeps the whole signed-in page in memory.

Loading works the other way. `browser.newContext({ storageState })` takes a path or the object. Cookies go into the new context's jar before its first page loads. `localStorage` entries are written into each origin once, before the page's own scripts run, so the app finds its token at boot.

## Directory layout

Keep one directory with two files per identity:

```
profiles/                     mode 700
  alice.json                  storageState: credentials, mode 600
  alice.identity.json         how alice looks: user agent, proxy, locale...
  bob.json
  bob.identity.json
```

Passwords do not go into this directory. Read them from the environment or a secret manager at sign-in time; the state file holds only what the site gave the browser after sign-in.

An identity file holds the context options for that identity, plus the settings the library applies another way:

```json
{
  "userAgent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36",
  "viewport": { "width": 1440, "height": 900 },
  "extraHTTPHeaders": { "accept-language": "de-DE,de;q=0.9" },
  "locale": "de-DE",
  "timezoneId": "Europe/Berlin",
  "proxy": "http://user:pass@proxy-de.example.net:3128",
  "proxyBypass": "*.internal"
}
```

| Field | How it is applied |
| --- | --- |
| `userAgent`, `viewport`, `extraHTTPHeaders` | Context options; the library honors them. |
| `locale` | CDP `Emulation.setLocaleOverride`: `navigator.language(s)` and `Accept-Language`. `Intl` and `Date` formatting keep the process locale. The library's own `locale` option has no effect. |
| `timezoneId` | CDP `Emulation.setTimezoneOverride`: `Date`, `Intl`, `Temporal`. It applies to the whole connection, so two contexts on one connection cannot have different zones. The library's own `timezoneId` option has no effect. |
| `proxy`, `proxyBypass` | `lightningpanda serve --proxy` and `--proxy-bypass`: one browser server per distinct proxy. The library's `proxy` context option has no effect. |

[Proxies, user agents, headers](/scraping/identity) explains each setting and what else is available.

## A profile store

This module opens a profile by name. It reuses the stored state when it still works, signs in again when it does not, and saves the result. Save it as `profiles.ts`:

```ts
// profiles.ts: one identity per name, stored on disk, reused across runs.
import { chromium, type Browser, type BrowserContext, type Page } from '@povio/lightning-panda/playwright';
import { existsSync } from 'node:fs';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';

export const DIR = process.env.PROFILE_DIR ?? 'profiles';

export type Identity = {
  userAgent?: string;
  viewport?: { width: number; height: number };
  extraHTTPHeaders?: Record<string, string>;
  locale?: string;
  timezoneId?: string;
  proxy?: string;
  proxyBypass?: string;
};
export type SignIn = (page: Page, name: string) => Promise<void>;

const paths = (name: string) => {
  if (!/^[\w.-]+$/.test(name)) throw new Error(`bad profile name ${name}`);
  return { state: `${DIR}/${name}.json`, identity: `${DIR}/${name}.identity.json` };
};

export async function readIdentity(name: string): Promise<Identity> {
  const { identity } = paths(name);
  return existsSync(identity) ? JSON.parse(await readFile(identity, 'utf8')) : {};
}

// The proxy is a server flag, so each distinct proxy gets its own browser server.
const browsers = new Map<string, Promise<Browser>>();
function browserFor(id: Identity): Promise<Browser> {
  const key = `${id.proxy ?? ''}|${id.proxyBypass ?? ''}`;
  let browser = browsers.get(key);
  if (!browser) {
    const args = id.proxy ? ['--proxy', id.proxy, ...(id.proxyBypass ? ['--proxy-bypass', id.proxyBypass] : [])] : [];
    browser = chromium.launch({ args });
    browsers.set(key, browser);
  }
  return browser;
}
export async function closeAll() {
  for (const browser of browsers.values()) await (await browser).close();
  browsers.clear();
}

async function newContext(name: string, withState: boolean) {
  const identity = await readIdentity(name);
  const { locale, timezoneId, proxy, proxyBypass, ...options } = identity;
  const browser = await browserFor(identity);
  const context = await browser.newContext({ ...options, ...(withState ? { storageState: paths(name).state } : {}) });
  const page = await context.newPage();
  if (locale || timezoneId) {
    const cdp = await context.newCDPSession(page);
    if (locale) await cdp.send('Emulation.setLocaleOverride', { locale });
    if (timezoneId) await cdp.send('Emulation.setTimezoneOverride', { timezoneId });
  }
  return { context, page };
}

/** Saves cookies and localStorage, readable by the owner only, replaced atomically. */
export async function saveProfile(context: BrowserContext, name: string) {
  const { state } = paths(name);
  await mkdir(DIR, { recursive: true, mode: 0o700 });
  await writeFile(`${state}.tmp`, JSON.stringify(await context.storageState(), null, 2), { mode: 0o600 });
  await rename(`${state}.tmp`, state);
}

/** A signed-in context for `name`: the stored state if it still works, else a new sign-in. */
export async function openProfile(
  name: string,
  opts: { signIn: SignIn; isSignedIn: (page: Page) => Promise<boolean> },
): Promise<{ context: BrowserContext; page: Page; reused: boolean }> {
  if (existsSync(paths(name).state)) {
    const { context, page } = await newContext(name, true);
    if (await opts.isSignedIn(page)) return { context, page, reused: true };
    await context.close(); // expired or revoked
  }
  const { context, page } = await newContext(name, false);
  await opts.signIn(page, name);
  await saveProfile(context, name);
  return { context, page, reused: false };
}
```

And a script that uses it. The site-specific parts are `signIn` (fill the login form) and `isSignedIn` (open a page that needs a session and see whether you were sent to the login page):

```ts
// scrape.ts: bun scrape.ts alice bob
import type { Page } from '@povio/lightning-panda/playwright';
import { closeAll, openProfile, type SignIn } from './profiles';

const BASE = 'http://127.0.0.1:4500';

const signIn: SignIn = async (page, name) => {
  const password = process.env[`PASSWORD_${name.toUpperCase()}`];
  if (!password) throw new Error(`no password for ${name}: set PASSWORD_${name.toUpperCase()}`);
  await page.goto(`${BASE}/login`);
  await page.getByLabel('User').fill(name);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL(`${BASE}/account`);
};

const isSignedIn = async (page: Page) => {
  await page.goto(`${BASE}/account`);
  return !page.url().startsWith(`${BASE}/login`);
};

try {
  for (const name of process.argv.slice(2)) {
    const { context, page, reused } = await openProfile(name, { signIn, isSignedIn });
    await page.goto(`${BASE}/account`);
    await page.locator('.order').first().waitFor();
    console.log(name, reused ? 'reused' : 'signed in', await page.locator('.order').allTextContents());
    await context.close();
  }
} finally {
  await closeAll();
}
```

First run, then a second run:

```
$ PASSWORD_ALICE=... PASSWORD_BOB=... bun scrape.ts alice bob
alice signed in [ "A-1: 12.5", "A-2: 99" ]
bob signed in [ "A-1: 12.5", "A-2: 99" ]
$ bun scrape.ts alice bob
alice reused [ "A-1: 12.5", "A-2: 99" ]
bob reused [ "A-1: 12.5", "A-2: 99" ]
```

The second run needs no passwords: both sessions came from the files.

### Only signing in

To create profiles without scraping, for example from a setup script or by hand once per account, the minimum is:

```ts
import { chromium } from '@povio/lightning-panda/playwright';

const browser = await chromium.launch();
const context = await browser.newContext();
const page = await context.newPage();
await page.goto('https://example.com/login');
// ...fill the form and submit...
await page.waitForURL('https://example.com/account');
await context.storageState({ path: 'profiles/alice.json' });
await browser.close();
```

Run `chmod 600 profiles/alice.json` afterwards; `storageState({ path })` writes with your default permissions.

Sites with captchas, two-factor prompts or device checks at sign-in cannot always be signed in by a script. Pyro is headless only and has no window to sign in by hand. Sign in where you can (a script with the code from your authenticator app, or a one-time manual step in another tool that exports the same `storageState` JSON, such as Playwright with a headed Chrome), then load the file here.

## Expiry and refresh

Sessions end. The site expires the cookie, rotates a token, or signs you out when it sees the session from a new IP. The store above handles this by checking: `isSignedIn` loads a page that needs a session, and a redirect to the login page triggers a new sign-in.

Some refinements:

- **Check without a page.** `context.request` sends the cookies of the context's `storageState`, outside the browser. A request that is redirected to the login page without a session tells you the state is stale without loading the app:

  ```ts
  const context = await browser.newContext({ storageState: 'profiles/alice.json' });
  const r = await context.request.get('https://example.com/account', { maxRedirects: 0 });
  const signedIn = r.status() === 200; // 302 to /login when the session is gone
  ```

  This sees cookies only. If the site's session lives in a `localStorage` token, check with a page.
- **Look at the expiry first.** Cookies carry `expires` (seconds since the epoch; `-1` means a session cookie). If the session cookie expires within the next hour, sign in again before you start a long job:

  ```ts
  import { readFileSync } from 'node:fs';

  function expiresSoon(path: string, cookieName: string, marginSeconds = 3600): boolean {
    const { cookies } = JSON.parse(readFileSync(path, 'utf8'));
    const c = cookies.find((c: { name: string }) => c.name === cookieName);
    return !c || (c.expires > 0 && c.expires < Date.now() / 1000 + marginSeconds);
  }
  ```

- **Save again after long runs.** Sites that roll the session (a new cookie on each request, sliding expiry) leave the file behind. Call `saveProfile(context, name)` at the end of a job, while a page of the site is still open, so the next run starts from the newest state.
- **Detect sign-outs mid-run.** If a page you scrape suddenly shows the login form, stop that identity's jobs, sign in again, and retry the job, instead of saving the signed-out state over the good file.
- **Keep one session per identity.** Two processes signing in as the same account at the same time can invalidate each other's sessions. Run one job per identity at a time, or share one saved state between them and sign in from one place only.

## Rotating identities

With several profiles, spread the work across them so no single account carries all of it. A simple round robin over the profile names:

```ts
const names = ['alice', 'bob', 'carol'];
let turn = 0;
const nextIdentity = () => names[turn++ % names.length];
```

Rotation is for spreading load you are allowed to make. It is not a way around a site's rate limits or bans; see [Scraping responsibly](/scraping/overview#scraping-responsibly).

## Running profiles in parallel

Each profile gets its own browser context, and contexts are isolated: two contexts on one connection have separate cookie jars and storage, so `alice` and `bob` can be open at the same time in one browser. For real parallelism, give each worker its own connection; each connection is a browser thread on the server. [Concurrency at scale](/scraping/concurrency) has a worker pool that assigns jobs to profiles.

The timezone override is the exception: it applies to a whole connection, and a second context on that connection asking for a different zone fails with `Cannot set timezone`. Keep identities with different time zones on different connections.

## The same `localStorage` for every context: `serve --local-storage`

When every context needs the same `localStorage` entries, for example one API token that is not tied to a person, the server can seed them:

```json
{ "https://app.example.com": { "token": "eyJhbGciOi...", "settings": { "theme": "dark" } } }
```

```sh
lightningpanda serve --local-storage seed.json
# or: LIGHTNINGPANDA_LOCAL_STORAGE=seed.json lightningpanda serve
```

Top-level keys are origins (or URLs; only the origin is used). Values are objects of entries. A non-string value is stored as its JSON text, so `settings` reads back as the string `'{"theme":"dark"}'`. Every connection and every new browser context starts with a fresh copy, and a page's writes stay in its own context. It seeds `localStorage` only, not cookies. With `chromium.launch()`, pass it as `args: ['--local-storage', 'seed.json']`.

## Security: these files are credentials

A state file is as good as a password for as long as the session lives, and often better: it skips two-factor authentication. Treat it that way.

- **Keep them out of version control.** Add the directory to `.gitignore`:

  ```gitignore
  profiles/*.json
  !profiles/*.identity.json
  ```

  (Identity files may contain proxy passwords; if yours do, ignore them too.)

- **Restrict permissions.** `chmod 700 profiles` and `chmod 600 profiles/*.json`. The store above writes with mode `600`. Run the scraper under its own system user.
- **Encrypt at rest** on shared machines and in backups. Options: keep the directory on an encrypted volume, or encrypt each file with [age](https://age-encryption.org/) or [sops](https://github.com/getsops/sops) and decrypt into memory at load time. `newContext({ storageState })` accepts an object, so the plaintext never has to touch the disk:

  ```ts
  const state = JSON.parse(await decrypt(`profiles/${name}.json.age`)); // your decrypt function
  const context = await browser.newContext({ storageState: state });
  ```

- **Do not log them.** Cookie values and tokens end up in logs, crash reports and CI output easily. Log profile names, not states.
- **Revoke when leaked.** Signing out on the site (or changing the password) usually invalidates the session in the file.
- **Mind the CDP port.** Anyone who can connect to your `lightningpanda serve` can read every cookie of every open context. Keep it on `127.0.0.1` or a private network. See [Running as a service](/scraping/service#securing-it).

## Related

- [Profiles booted once (fork)](/scraping/fork-profiles): the server signs in once and forks a fresh signed-in page per job.
- [Profiles and pre-booted pages](/guide/profiles): the concept in general.
- [Proxies, user agents, headers](/scraping/identity): every identity setting.
