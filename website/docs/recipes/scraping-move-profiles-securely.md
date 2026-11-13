# Moving profiles between machines securely

A saved profile (`storageState` JSON) is a credential. Whoever has the file is signed in as that account until the session ends, often without the second factor. This recipe keeps profile files encrypted at rest, loads them on another machine without writing plaintext to disk, rotates the encryption key, and revokes a profile when it leaks.

It uses AES-256-GCM from `node:crypto`, which works the same on Bun and Node.js, so there is nothing else to install.

## What you get

- `profiles/<name>.json.sealed`: the state, encrypted and authenticated, mode `600`. Changing a byte, or renaming alice's file to bob's, makes it fail to open.
- The key comes from the environment (`PROFILE_KEYS`), so it can come from your secret manager, CI secrets or a systemd credential, not from the disk next to the files.
- Several keys at once: the first one encrypts, all of them decrypt. That lets you rotate keys without stopping jobs.
- Files are decrypted in memory and passed to `newContext({ storageState })` as an object.

## Files

```
move/
  site.ts            test site (sign-in, session cookie + localStorage token, expiry)
  sealed.ts          seal/open, load and save sealed state files
  sign-in.ts         sign in and save a sealed profile
  use.ts             load a sealed profile and scrape with it
  profiles-cli.ts    seal existing files, rotate keys, list files
  profiles/          mode 700
```

```sh
mkdir move && cd move
bun init -y
bun add @povio/pyro
```

Until `@povio/pyro` is published under that name, install it as [Installation](/guide/installation#try-today) describes.

## The test site

The same site as in the [identity pool recipe](/recipes/scraping-identity-pool): `/login` sets an `httpOnly` session cookie and a `localStorage` token, `/account` needs both, and `/admin/expire?user=<name>` ends a user's sessions, as signing out everywhere does on a real site.

::: details site.ts
```ts
// site.ts: a local test site with sign-in, a session that expires, and a page
// that needs both the session cookie and a localStorage token.
// bun site.ts 4601
const PORT = Number(process.argv[2] ?? 4601);
const TTL_MS = Number(process.env.SESSION_TTL_MS ?? 60_000);
const USERS: Record<string, string> = { alice: 'pw-alice', bob: 'pw-bob', carol: 'pw-carol' };
const sessions = new Map<string, { user: string; expires: number; token: string }>();

const html = (body: string, status = 200, headers: Record<string, string> = {}) =>
  new Response(`<!doctype html><meta charset="utf-8">${body}`, { status, headers: { 'content-type': 'text/html', ...headers } });
const session = (req: Request) => {
  const sid = /(?:^|; )sid=([^;]+)/.exec(req.headers.get('cookie') ?? '')?.[1];
  const s = sid ? sessions.get(sid) : undefined;
  return s && s.expires > Date.now() ? s : undefined;
};

Bun.serve({
  port: PORT,
  hostname: '127.0.0.1',
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === '/login' && req.method === 'GET') {
      return html(`<title>Sign in</title><form method="post">
        <label>User <input name="user"></label>
        <label>Password <input name="password" type="password"></label>
        <button>Sign in</button></form>`);
    }
    if (url.pathname === '/login' && req.method === 'POST') {
      const form = await req.formData();
      const user = String(form.get('user'));
      if (USERS[user] !== form.get('password')) return html('<p>Wrong password</p>', 401);
      const sid = crypto.randomUUID();
      const token = crypto.randomUUID();
      sessions.set(sid, { user, expires: Date.now() + TTL_MS, token });
      console.log(`sign-in ${user} from ${req.headers.get('user-agent')}`);
      // The app keeps an API token in localStorage, next to the httpOnly cookie.
      return html(`<script>localStorage.setItem('apiToken', ${JSON.stringify(token)}); location.href = '/account';</script>`, 200, {
        'set-cookie': `sid=${sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${Math.ceil(TTL_MS / 1000)}`,
      });
    }
    if (url.pathname === '/account') {
      const s = session(req);
      if (!s) return new Response(null, { status: 302, headers: { location: '/login' } });
      // Orders come from an API call that needs the localStorage token.
      return html(`<title>Account</title><h1 id="who">${s.user}</h1><ul id="orders"></ul>
        <script>
          fetch('/api/orders', { headers: { authorization: 'Bearer ' + localStorage.getItem('apiToken') } })
            .then((r) => r.json())
            .then((orders) => { for (const o of orders) document.querySelector('#orders').insertAdjacentHTML('beforeend', '<li class="order">' + o + '</li>'); });
        </script>`);
    }
    if (url.pathname === '/api/orders') {
      const s = session(req);
      if (!s || req.headers.get('authorization') !== `Bearer ${s.token}`) return Response.json({ error: 'unauthorized' }, { status: 401 });
      return Response.json([`${s.user}-1`, `${s.user}-2`]);
    }
    if (url.pathname === '/whoami') {
      return Response.json({ ua: req.headers.get('user-agent'), via: req.headers.get('x-test-proxy') });
    }
    if (url.pathname === '/admin/expire') {
      // Ends every session of one user, as a site does when it signs you out.
      for (const [sid, s] of sessions) if (s.user === url.searchParams.get('user')) sessions.delete(sid);
      return new Response('ok');
    }
    return new Response('not found', { status: 404 });
  },
});
console.log(`site on http://127.0.0.1:${PORT}`);
```
:::

```sh
SESSION_TTL_MS=3600000 bun site.ts 4601
```

## The code

`sealed.ts` does the encryption and the file handling:

```ts
// sealed.ts: profile files encrypted at rest with AES-256-GCM (node:crypto; Bun and Node.js).
//
// Keys come from PROFILE_KEYS: "id:base64key,id:base64key,...". The first key
// encrypts; any listed key decrypts, so you can rotate without a flag day.
// Make a key with: bun -e "console.log(require('crypto').randomBytes(32).toString('base64'))"
import { createCipheriv, createDecipheriv, randomBytes } from 'node:crypto';
import { chmod, readFile, rename, writeFile } from 'node:fs/promises';

type Envelope = { v: 1; kid: string; iv: string; tag: string; data: string };

function keys(): { kid: string; key: Buffer }[] {
  const raw = process.env.PROFILE_KEYS;
  if (!raw) throw new Error('set PROFILE_KEYS=id:base64key[,id:base64key...]');
  return raw.split(',').map((entry) => {
    const [kid, b64] = entry.split(':');
    const key = Buffer.from(b64 ?? '', 'base64');
    if (!kid || key.length !== 32) throw new Error(`PROFILE_KEYS: key "${kid}" must be 32 bytes, base64`);
    return { kid, key };
  });
}

export function seal(plaintext: string, name: string): string {
  const { kid, key } = keys()[0];
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  cipher.setAAD(Buffer.from(name)); // binds the file to its profile name: alice's file cannot pass as bob's
  const data = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()]);
  const env: Envelope = { v: 1, kid, iv: iv.toString('base64'), tag: cipher.getAuthTag().toString('base64'), data: data.toString('base64') };
  return JSON.stringify(env);
}

export function open(sealed: string, name: string): string {
  const env = JSON.parse(sealed) as Envelope;
  const k = keys().find((k) => k.kid === env.kid);
  if (!k) throw new Error(`${name}: sealed with key "${env.kid}", which is not in PROFILE_KEYS (revoked?)`);
  const decipher = createDecipheriv('aes-256-gcm', k.key, Buffer.from(env.iv, 'base64'));
  decipher.setAAD(Buffer.from(name));
  decipher.setAuthTag(Buffer.from(env.tag, 'base64'));
  try {
    return Buffer.concat([decipher.update(Buffer.from(env.data, 'base64')), decipher.final()]).toString('utf8');
  } catch {
    throw new Error(`${name}: cannot decrypt (wrong key, or the file was changed)`);
  }
}

/** Reads profiles/<name>.json.sealed into the object `newContext({ storageState })` takes. */
export async function loadState(dir: string, name: string) {
  return JSON.parse(open(await readFile(`${dir}/${name}.json.sealed`, 'utf8'), name));
}

/** Writes state sealed, mode 600, atomically: readers see the old file or the new one. */
export async function saveState(dir: string, name: string, state: unknown) {
  const path = `${dir}/${name}.json.sealed`;
  const tmp = `${path}.${process.pid}.${randomBytes(6).toString('hex')}.tmp`;
  await writeFile(tmp, seal(JSON.stringify(state), name), { mode: 0o600 });
  await chmod(tmp, 0o600);
  await rename(tmp, path);
}
```

`sign-in.ts` signs in and writes the sealed file directly. The plaintext state never touches the disk:

```ts
// sign-in.ts: PASSWORD=... bun sign-in.ts alice   (writes profiles/alice.json.sealed)
import { chromium } from '@povio/pyro/wright';
import { mkdir } from 'node:fs/promises';
import { saveState } from './sealed';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4601';
const DIR = process.env.PROFILE_DIR ?? 'profiles';
const name = process.argv[2];
const password = process.env.PASSWORD;
if (!name || !password) throw new Error('usage: PASSWORD=... bun sign-in.ts <name>');

await mkdir(DIR, { recursive: true, mode: 0o700 });
const browser = await chromium.launch();
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(`${BASE}/login`);
  await page.getByLabel('User').fill(name);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL(`${BASE}/account`);
  await page.locator('.order').first().waitFor();
  await saveState(DIR, name, await context.storageState()); // sealed in memory, never written in plaintext
  console.log(`${name}: signed in, saved ${DIR}/${name}.json.sealed`);
} finally {
  await browser.close();
}
```

`use.ts` is what runs on the other machine. It decrypts in memory, scrapes, and saves the state back sealed, so a session the site rolls forward stays current:

```ts
// use.ts: bun use.ts alice   (on any machine that has the sealed files and PROFILE_KEYS)
import { chromium } from '@povio/pyro/wright';
import { loadState, saveState } from './sealed';

const BASE = process.env.BASE ?? 'http://127.0.0.1:4601';
const DIR = process.env.PROFILE_DIR ?? 'profiles';
const name = process.argv[2] ?? 'alice';

const browser = await chromium.launch();
try {
  // Decrypted in memory and handed over as an object: no plaintext file on this machine.
  const context = await browser.newContext({ storageState: await loadState(DIR, name) });
  const page = await context.newPage();
  await page.goto(`${BASE}/account`);
  if (page.url().startsWith(`${BASE}/login`)) {
    console.log(`${name}: signed out; sign in again and save a new state`);
    process.exitCode = 1;
  } else {
    await page.locator('.order').first().waitFor();
    console.log(`${name}: signed in as ${await page.locator('#who').textContent()},`, await page.locator('.order').allTextContents());
    await saveState(DIR, name, await context.storageState()); // keep a rolling session current, sealed again
  }
} finally {
  await browser.close();
}
```

`profiles-cli.ts` seals plaintext files you already have (for example those of the [identity pool](/recipes/scraping-identity-pool)), re-seals with a new key, and lists which key sealed each file:

```ts
// profiles-cli.ts: seal, rotate and check profile files.
//   bun profiles-cli.ts seal <dir>     every <name>.json -> <name>.json.sealed, plaintext removed
//   bun profiles-cli.ts rotate <dir>   re-seal every *.sealed with the first key of PROFILE_KEYS
//   bun profiles-cli.ts list <dir>     which key sealed each file, and when its cookies expire
import { readdir, readFile, unlink } from 'node:fs/promises';
import { open, saveState } from './sealed';

const [command, dir = 'profiles'] = process.argv.slice(2);
const files = (await readdir(dir)).sort();

if (command === 'seal') {
  for (const f of files) {
    const m = /^([\w.-]+)\.json$/.exec(f);
    if (!m || f.endsWith('.identity.json')) continue;
    await saveState(dir, m[1], JSON.parse(await readFile(`${dir}/${f}`, 'utf8')));
    await unlink(`${dir}/${f}`);
    console.log(`sealed ${m[1]}`);
  }
} else if (command === 'rotate') {
  for (const f of files) {
    const m = /^([\w.-]+)\.json\.sealed$/.exec(f);
    if (!m) continue;
    const state = JSON.parse(open(await readFile(`${dir}/${f}`, 'utf8'), m[1])); // with any listed key
    await saveState(dir, m[1], state); // with the first key
    console.log(`rotated ${m[1]}`);
  }
} else if (command === 'list') {
  for (const f of files) {
    const m = /^([\w.-]+)\.json\.sealed$/.exec(f);
    if (!m) continue;
    const sealed = await readFile(`${dir}/${f}`, 'utf8');
    const { cookies } = JSON.parse(open(sealed, m[1])) as { cookies: { expires: number }[] };
    const ends = cookies.map((c) => c.expires).filter((e) => e > 0);
    const first = ends.length ? new Date(Math.min(...ends) * 1000).toISOString() : 'session cookies only';
    console.log(`${m[1]}\tkey ${JSON.parse(sealed).kid}\t${cookies.length} cookies, first expiry ${first}`);
  }
} else {
  console.error('usage: bun profiles-cli.ts seal|rotate|list <dir>');
  process.exit(2);
}
```

## Run it

Make a key and keep it in your secret store. The examples pass it in `PROFILE_KEYS`:

```sh
export OLD=$(bun -e "console.log(require('crypto').randomBytes(32).toString('base64'))")
export PROFILE_KEYS="k1:$OLD"
```

On machine A, sign in:

```
$ PASSWORD=pw-alice bun sign-in.ts alice
pyro: CDP server listening on ws://127.0.0.1:56605/
alice: signed in, saved profiles/alice.json.sealed
$ PASSWORD=pw-bob bun sign-in.ts bob
pyro: CDP server listening on ws://127.0.0.1:56610/
bob: signed in, saved profiles/bob.json.sealed
$ ls -l profiles
-rw-------@ 505 alice.json.sealed
-rw-------@ 505 bob.json.sealed
```

Copy the sealed files to machine B (`scp`, `rsync`, an object store; they are safe to move because they are encrypted) and give machine B the same `PROFILE_KEYS`. Then:

```
$ bun use.ts alice
pyro: CDP server listening on ws://127.0.0.1:56618/
alice: signed in as alice, [ "alice-1", "alice-2" ]
$ bun profiles-cli.ts list profiles
alice	key k1	1 cookies, first expiry 2026-11-13T05:24:21.075Z
bob	key k1	1 cookies, first expiry 2026-11-13T05:24:21.205Z
```

Machine B is signed in as alice without having seen her password. `list` shows when the first cookie expires; plan to sign in again before that.

::: warning The proxy moves with the profile
Many sites end a session when it shows up from a new IP address. If machine B reaches the site from another address than machine A did, the session may end on first use. Keep each identity on the same proxy wherever it runs: put the proxy in the identity's sidecar file (see [Stored profiles](/scraping/profiles#directory-layout)) and seal that file too if it holds proxy credentials.
:::

## Rotating the key

Rotate on a schedule, and whenever someone who had the key leaves. Make a new key, put it first and keep the old one second while you re-seal:

```sh
export NEW=$(bun -e "console.log(require('crypto').randomBytes(32).toString('base64'))")
```

```
$ PROFILE_KEYS="k2:$NEW,k1:$OLD" bun profiles-cli.ts rotate profiles
rotated alice
rotated bob
$ PROFILE_KEYS="k2:$NEW" bun profiles-cli.ts list profiles
alice	key k2	1 cookies, first expiry 2026-11-13T05:24:21.075Z
bob	key k2	1 cookies, first expiry 2026-11-13T05:24:21.205Z
$ PROFILE_KEYS="k2:$NEW" bun use.ts bob
pyro: CDP server listening on ws://127.0.0.1:56627/
bob: signed in as bob, [ "bob-1", "bob-2" ]
```

Once every machine has the new key and every file is re-sealed, drop the old key from `PROFILE_KEYS`. A file sealed with it (an old backup, a copy left on a laptop) no longer opens:

```
$ PROFILE_KEYS="k2:$NEW" PROFILE_DIR=old bun use.ts bob
error: bob: sealed with key "k1", which is not in PROFILE_KEYS (revoked?)
```

The profile name is bound into the encryption, so a file cannot be swapped for another identity's:

```
$ cp profiles/alice.json.sealed swap/bob.json.sealed
$ PROFILE_KEYS="k2:$NEW" PROFILE_DIR=swap bun use.ts bob
error: bob: cannot decrypt (wrong key, or the file was changed)
```

## Revoking a profile

Rotating the key protects files from now on. It does not end the session inside a file that already leaked together with its key: the site still accepts that cookie. To revoke a profile, end the session **at the site**: sign out of all sessions, change the password, or revoke the token in the site's settings. Then delete the file and sign in again. On the test site:

```
$ curl -s "http://127.0.0.1:4601/admin/expire?user=bob"
ok
$ PROFILE_KEYS="k2:$NEW" bun use.ts bob
pyro: CDP server listening on ws://127.0.0.1:56643/
bob: signed out; sign in again and save a new state
exit 1
```

After revocation the file is useless, wherever copies of it are.

## Other ways to do it

- **age or sops.** [age](https://age-encryption.org/) and [sops](https://github.com/getsops/sops) encrypt files with keys from a file, a cloud KMS or SSH keys. Decrypt into memory (run the tool and read its stdout) and pass the parsed object to `newContext({ storageState })`, as `use.ts` does with `loadState`.
- **An encrypted volume** for the profile directory, if only the disk needs protecting.
- **Fork profiles** keep the session in the server's memory and never write it to disk; see [Fork-mode signed-in jobs](/recipes/scraping-fork-signed-in-jobs). They do not survive a restart.

Whatever you use: keep the files out of version control, log profile names and never state contents, and run the scraper under its own system user.

## Next

- A pool that signs identities in again when their sessions expire: [A pool of signed-in identities](/recipes/scraping-identity-pool).
- What a profile file holds and what it misses: [Stored profiles](/scraping/profiles#what-a-profile-file-holds).
