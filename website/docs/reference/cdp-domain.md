# Pyro CDP domain

`Lightningpanda.*` is a browser-level CDP domain that Pyro adds to the standard protocol. It manages **prepared pages**: pages booted ahead of a test in their own browser context, so a test takes a page that has already loaded the app instead of waiting for it. In fork mode it also manages **shared profiles** that every client of one server can use.

The SDK ([`lp.profile()`, `lp.page()`, `lp.adopt()`](/reference/sdk)) and the Playwright drop-in ([boot pools](/playwright/boot-pools)) call these commands for you. Use them directly from Puppeteer, raw CDP or another client. For the concepts, see [Profiles and pre-booted pages](/guide/profiles) and [Connections and fork mode](/guide/fork-mode).

The per-page driver commands `LP.run`, `LP.idle` and the rest of the `LP.*` domain are documented in [LP.run wire format](/reference/lp-run).

## Calling convention

- Send these commands on the **browser session**: no `sessionId`, or a session from `Target.attachToBrowserTarget`. Nothing here is page-scoped.
- They are per connection. Without `--fork`, each CDP connection has its own profiles and pools. With [`--fork`](/reference/serve#fork), profiles live in the server's template process and every connection that opens later shares them.
- Errors use the standard CDP error object. Bad parameters return `-32602` (invalid params). Fork-registry failures return `-32000` (server error). An unknown `Lightningpanda.*` method returns `-32601` `'Lightningpanda.x' wasn't found`.

```json
{ "id": 1, "method": "Lightningpanda.capabilities" }
{ "id": 1, "result": { "fork": false, "sharedProfiles": false } }
```

## Methods at a glance

| Method | Needs | Purpose |
| --- | --- | --- |
| [`capabilities`](#capabilities) | — | Is this server in fork mode? Are profiles shared? |
| [`registerPreparedPage`](#registerpreparedpage) | — (`name`, `setup`, `base` need `--fork`) | Declare a profile: start booting its pages. |
| [`acquirePreparedPage`](#acquirepreparedpage) | a profile with a waiting page | Take a booted page as a new target. |
| [`preparedPages`](#preparedpages) | — | List the pages waiting on this connection. |
| [`unregisterPreparedPage`](#unregisterpreparedpage) | — (`name` needs `--fork`) | Retire a profile or release a named one. |
| [`adoptPreparedPage`](#adoptpreparedpage) | `--fork` | Hold a profile another client registered by name. |
| [`preparedProfiles`](#preparedprofiles) | — (empty without `--fork`) | List named profiles and their holders. |

Profiles come from two places:

- [`serve --prepare <URL>`](/reference/serve#prepare) (repeatable): one profile per URL, in order, so the first `--prepare` is `profileId` 0. Each keeps one page booted (`pool: 1`). [`--prepare-refill`](/reference/serve#prepare-refill) sets when the replacement boots.
- `Lightningpanda.registerPreparedPage` at run time. Ids continue after the `--prepare` ones.

## The profile object

`registerPreparedPage` takes a profile, and `adoptPreparedPage` returns one. Its fields:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | string | required | Absolute URL each page navigates to. Not absolute: `-32602 url is not an absolute URL`. |
| `initScripts` | string[] | `[]` | Scripts run before the document's own scripts in every page of the profile's context, popups included. They replace the `Page.addScriptToEvaluateOnNewDocument` calls a test would otherwise make after the page exists. |
| `viewport` | `{width, height}` (integers) | server default (1280x720) | Viewport set before the navigation. |
| `pool` | integer 1–8 | `1` | Pages kept booted per profile. Each acquisition starts booting the next one. Outside 1–8: `-32602 pool must be between 1 and 8`. Ignored in fork mode, where the template keeps exactly one page per profile. The SDK sends `2` by default. |
| `refillAfterClose` | boolean | `false` | Boot the replacement only once the acquired page's browser context closes, not as soon as the thread is quiet. You get at most `pool` booted apps instead of `pool` + 1, but the next test waits for the boot when tests run back to back. The per-request form of `--prepare-refill after-close`. Ignored in fork mode. |
| `setup` | object[] | `[]` | [`LP.run` steps](/reference/lp-run) run once on the booted page before any test gets it. **`--fork` only**: the template runs them, and every connection forked afterwards starts from the state they leave. The steps must finish within 60 s. |
| `base` | integer (profile id) | — | Start from another profile's page instead of booting `url`. The template forks that profile's snapshot and runs only this profile's `setup` on it. **`--fork` only.** Pages of a derived profile are never booted from `url`. |
| `name` | string | — | Register as a shared, named profile. **`--fork` only.** Must not be empty. See [Shared profiles](#shared-profiles). |

Without `--fork`, `setup` or `base` returns `-32602 setup and base need serve --fork`, and `name` returns `-32602 profile names need serve --fork`.

## capabilities

Reports what the server supports. The SDK calls it once after connecting. An older server without the domain answers with a CDP error, which the SDK treats as "no fork mode".

**Params:** none.

**Result:**

| Field | Type | Description |
| --- | --- | --- |
| `fork` | boolean | The server runs with [`--fork`](/reference/serve#fork): open one connection per test. |
| `sharedProfiles` | boolean | Profiles can have names every client of the server can adopt. Currently equal to `fork`. |

## registerPreparedPage

Declares a profile and starts booting its pages.

- **Without `--fork`:** the profile belongs to this connection. Its `pool` pages start booting on the connection's browser thread right away.
- **With `--fork`:** the template process boots the profile once. Connections opened **after** the call start with its page already booted and share its memory copy-on-write. The registering connection is a fork made earlier, so it does not get the page itself: open a new connection (see [Connecting to a profile's fork](#connecting-to-a-profile-s-fork)). Registering a definition identical to an active profile returns that profile's id; nothing boots again.

**Params:** the [profile object](#the-profile-object).

**Result:** `{ "profileId": number }`

**Errors:** the invalid-params cases in the profile table. In fork mode the registry can also return:

- `profile <name> is registered with a different definition (release it first)`
- `no profile <id> to start from` (`base` is not an active profile)
- `cannot boot a page at <url>`
- `the connection's lease has ended`

```json
{ "id": 2, "method": "Lightningpanda.registerPreparedPage",
  "params": { "url": "http://localhost:5173/app",
              "initScripts": ["localStorage.setItem('token', 'abc')"],
              "viewport": { "width": 1280, "height": 800 },
              "pool": 2 } }
{ "id": 2, "result": { "profileId": 0 } }
```

## acquirePreparedPage

Takes one waiting page and makes it a real target in its own browser context. The page is announced like a new target: `Target.targetCreated` when discovery is on, and `Target.attachedToTarget` under auto-attach. Playwright therefore sees it as a new page of its default context. The profile's next page starts booting once the thread is quiet.

**Params:**

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `profileId` | integer | any | Take a page of this profile. |
| `url` | string | any | Take a page whose profile `url` equals this, exactly. |
| `attach` | boolean | `false` | Attach a flat session in the same call, as `Target.attachToTarget {flatten: true}` would. Ignored when browser-level auto-attach is on, because auto-attach already attaches the page. |

With both filters empty, any waiting page matches. A loaded page is preferred. Otherwise you get the one that has been booting longest.

**Result:**

| Field | Type | Description |
| --- | --- | --- |
| `targetId` | string | The page target. |
| `browserContextId` | string | Its browser context. Each prepared page has its own context, seeded with [`--local-storage`](/reference/serve#local-storage). |
| `sessionId` | string \| null | The session, with `attach: true` or under auto-attach. Otherwise `null`. |
| `url` | string | The profile's URL. |
| `ready` | boolean | `true`: the page has loaded. `false`: the page is still navigating to `url`. Wait for that navigation, for example with a `loaded` step (the SDK prepends one). |

**Errors:**

- No page matches: `-32602 No prepared page for that url: register one with Lightningpanda.registerPreparedPage or serve --prepare`. The SDK catches this code and opens a fresh page instead.
- The connection already has 256 targets: `-32000 Too many targets`.

```json
{ "id": 3, "method": "Lightningpanda.acquirePreparedPage", "params": { "profileId": 0, "attach": true } }
{ "id": 3, "result": { "targetId": "…03", "browserContextId": "…01", "sessionId": "…02",
                       "url": "http://localhost:5173/app", "ready": true } }
```

## preparedPages

Lists the pages waiting on this connection, booted or still booting. Acquired pages are not listed. A call to this method does not count as client activity. Replacements boot once the connection has no open page or has gone 15 ms without commands, so polling this method does not hold them back.

**Params:** none.

**Result:** `{ "pages": [{ "profileId": number, "url": string, "ready": boolean }] }`

`ready` is `true` once the page has loaded. The SDK's `lp.warm(name)` polls this until the profile's pool is ready.

## unregisterPreparedPage

Retires a profile, or, in fork mode, releases this connection's hold on a named profile.

**Params:** exactly one of these:

| Field | Type | Description |
| --- | --- | --- |
| `profileId` | integer | Retire this profile. Its waiting pages close. Pages already acquired stay valid until their owner closes them. In fork mode, this retires an unnamed profile in the template. |
| `name` | string | **`--fork` only.** Drop this connection's hold on the named profile. The profile is retired once no connection holds it and no other profile derives from it. |

**Result:**

- With `profileId`: `{ "removed": boolean }`. `false` means the profile was already retired.
- With `name`: `{ "removed": boolean, "retired": boolean }`. `removed` means this connection held the name. `retired` means the profile went with it.

**Errors:**

- `-32602 profileId or name is required`
- `-32602 Unknown prepared profile` (id out of range)
- `-32602 Prepared profile still has dependent profiles` (another active profile uses it as `base`)
- `-32602 profile names need serve --fork` (`name` without fork mode)

## adoptPreparedPage

**`--fork` only.** Holds the profile another client registered as `name`, so this connection can fork pages from it (and derive from it) without registering it again. The hold ends with `unregisterPreparedPage {name}` or when the connection closes.

**Params:** `{ "name": string }` (required)

**Result:**

| Field | Type | Description |
| --- | --- | --- |
| `profileId` | integer | The profile's id in the template. |
| `url` | string | |
| `initScripts` | string[] | |
| `viewport` | `{width, height}` \| null | |
| `setup` | object[] | The profile's setup steps. |
| `base` | integer \| null | The profile it derives from. |

**Errors:**

- `-32602 adopting a profile needs serve --fork`
- `-32602 name is required`
- `-32000 no profile <name>`

## preparedProfiles

Lists the named (shared) profiles the fork template keeps. Without `--fork`, it returns an empty list.

**Params:** none.

**Result:** `{ "profiles": [{ "name": string, "profileId": number, "holders": number, "url": string, "base"?: number }] }`

`holders` is the number of connections that hold the name.

## Shared profiles

With [`--fork`](/reference/serve#fork), several clients can share one server, for example test workers in separate processes ([One server, many workers](/e2e/shared-server)):

1. A client calls `registerPreparedPage { name: "admin", url, … }`. The template boots the profile, and the client holds the name.
2. Registering the **same definition** under the same name finds the profile already booted and adds a holder. A **different definition** is refused while another client holds the name. With no holders left, the name moves to the new definition.
3. Another client calls `adoptPreparedPage { name: "admin" }` to hold it without knowing the definition.
4. `unregisterPreparedPage { name }` releases a hold. A connection's holds also end when it closes. A profile is retired once nobody holds it and no other profile is derived from it.

## Connecting to a profile's fork

In fork mode, each WebSocket connection is a `fork()` of a template that already booted every profile. Add `?profile=<id>` to the WebSocket URL to get a fork of that profile's snapshot:

```text
ws://127.0.0.1:9222/?profile=3
```

This matters for derived profiles (`base`), which live in a template forked off their base. On the new connection, `acquirePreparedPage {profileId: 3, attach: true}` returns the booted page at once. The SDK opens one such connection per `lp.page({ as })`.

## Example: raw CDP

```ts
// bun run prepared.ts  (with `lightningpanda serve` running on 9222)
const ws = new WebSocket('ws://127.0.0.1:9222/');
await new Promise((r) => (ws.onopen = r));
let next = 0;
const waiting = new Map<number, (m: any) => void>();
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  waiting.get(m.id)?.(m);
};
const send = (method: string, params = {}, sessionId?: string) =>
  new Promise<any>((resolve) => {
    const id = ++next;
    waiting.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params, sessionId }));
  });

const { result: { profileId } } = await send('Lightningpanda.registerPreparedPage', {
  url: 'https://example.com/',
});
// Wait until it has booted (optional: acquire returns ready: false otherwise).
while (!(await send('Lightningpanda.preparedPages')).result.pages.some((p: any) => p.ready)) {
  await Bun.sleep(10);
}
const { result: page } = await send('Lightningpanda.acquirePreparedPage', { profileId, attach: true });
const title = await send('Runtime.evaluate', { expression: 'document.title', returnByValue: true }, page.sessionId);
console.log(title.result.result.value); // "Example Domain"
ws.close();
```
