# serve options

`lightningpanda serve` starts the CDP server: the Chrome DevTools Protocol over WebSocket on `ws://HOST:PORT/`. Every connection gets its own browser thread with its own JavaScriptCore engine (or, with [`--fork`](#fork), its own forked process). This page lists every option of 0.0.33 plus what is newer on `main`; `lightningpanda serve --help` prints the list for the installed version.

```sh
lightningpanda serve [OPTIONS]
```

Options take their value as the next argument (`--port 9333`; the `--port=9333` form is not accepted). An unknown option or a bad value prints the error and the usage, and exits `2`. Repeatable options add one entry per use.

## All options {#summary}

| Option | Value | Default | Environment |
| --- | --- | --- | --- |
| [`--host`](#host) | IP address or `localhost` | `127.0.0.1` | |
| [`--port`](#port) | port | `9222` | |
| [`--advertise-host`](#advertise-host) | host | the bound address | |
| [`--cdp-max-connections`](#cdp-max-connections) | integer | `16` | |
| [`--cdp-max-message-size`](#cdp-max-message-size) | bytes | `67108864` (64 MiB) | |
| [`--cdp-max-http-message-size`](#cdp-max-http-message-size) | bytes | `1048576` | |
| [`--log-level`](#log-level) | `debug` \| `info` \| `warn` \| `error` \| `fatal` | `info` | |
| [`--experimental-features`](#experimental-features) | `cors` | | |
| [`--load-resources`](#load-resources) | `stylesheet` \| `iframe` | | |
| [`--local-storage`](#local-storage) | JSON file | | `LIGHTNINGPANDA_LOCAL_STORAGE` |
| [`--static`](#static) | `ORIGIN=DIR` (repeatable) | | `LIGHTNINGPANDA_STATIC` (`;`-separated) |
| [`--static-fallback`](#static-fallback) | `spa` \| `network` | `spa` | |
| [`--proxy`](#proxy) | URL | | |
| [`--proxy-bypass`](#proxy-bypass) | list | | |
| [`--prepare`](#prepare) | URL (repeatable) | | `LIGHTNINGPANDA_PREPARE` (`;`-separated) |
| [`--prepare-refill`](#prepare-refill) | `quiet` \| `after-close` | `quiet` | `LIGHTNINGPANDA_PREPARE_REFILL` |
| [`--fork`](#fork) | flag | off | `LIGHTNINGPANDA_FORK=1` |
| [`--spare-engine`](#spare-engine) | `on` \| `off` | on with `--prepare`, else off | `LIGHTNINGPANDA_SPARE_ENGINE` |
| [`--engine-reuse`](#engine-reuse) (main, after 0.0.33) | `on` \| `off` \| `thread` \| `always` | `on` | `LIGHTNINGPANDA_ENGINE_REUSE` |
| [`--render-prewarm`](#render-prewarm) | `on` \| `off` | on with `--prepare`, else off | `LIGHTNINGPANDA_RENDER_PREWARM` |
| [`--hyphen-digit-breaks`](#hyphen-digit-breaks) | `on` \| `off` | `off` | `LIGHTNINGPANDA_HYPHEN_DIGIT_BREAKS` |
| [`--parent-pid`](#parent-pid) | PID | | `LIGHTNINGPANDA_PARENT_PID` |

`on`/`off` switches also accept `1`/`0` and `true`/`false`. Where an option has an environment variable, the variable sets the default and the flag wins; for `--static` and `--prepare` the flags add to the variable's entries. An environment value that does not parse is ignored (the flag would be an error). More variables that tune the server are in [Environment variables](/reference/env#serve).

## Network {#network}

### `--host` {#host}

`--host <HOST>`, default `127.0.0.1`. The address to listen on: an IPv4 or IPv6 address, or `localhost` (taken as `127.0.0.1`). Host names other than `localhost` are rejected (`'…' is not an IP address`). Use `0.0.0.0` to accept connections from other machines, for example in a container, together with [`--advertise-host`](#advertise-host).

### `--port` {#port}

`--port <INT>`, default `9222`. TCP port. `0` picks a free port; the chosen address is printed on stderr (`CDP server listening on ws://127.0.0.1:54321/`). `Lightningpanda.launch()` picks a free port itself and passes it.

### `--advertise-host` {#advertise-host}

`--advertise-host <HOST>`, default: the bound address. The host written into `webSocketDebuggerUrl` of `GET /json/version` (the port stays the bound one). Set it when clients reach the server under another name than the one it binds, e.g. `--host 0.0.0.0 --advertise-host browser.internal`.

```sh
curl -s http://127.0.0.1:9222/json/version
# {"Browser":"Chrome/124.0.6367.29","Protocol-Version":"1.3",...,"webSocketDebuggerUrl":"ws://127.0.0.1:9222/"}
```

The HTTP endpoints and the WebSocket upgrade only answer requests whose `Host` header is an IP address or `localhost`; see [CDP coverage: connecting](/reference/cdp#connecting).

## Limits {#limits}

### `--cdp-max-connections` {#cdp-max-connections}

`--cdp-max-connections <INT>`, default `16` (values below 1 become 1). Live TCP connections the server serves at once, counting WebSocket sessions and HTTP requests alike. At the limit the listener stops accepting until one closes; further connections wait in the kernel's backlog rather than being refused. Raise it for many parallel workers or [fork mode](#fork) with one connection per page: `lightningpanda e2e` passes `max(64, 4 × concurrency + 8)`.

### `--cdp-max-message-size` {#cdp-max-message-size}

`--cdp-max-message-size <INT>`, default `67108864` (64 MiB). Largest WebSocket message the server accepts from a client. Large `LP.run` payloads (screenshot baselines are sent inline as base64, uploads as `text`/`base64`) count against it.

### `--cdp-max-http-message-size` {#cdp-max-http-message-size}

`--cdp-max-http-message-size <INT>`, default `1048576`. Largest HTTP request head (the `/json/*` requests and the WebSocket upgrade). The server caps it at 16 KiB whatever the value; a larger head gets `413 request head too large`.

## Logging {#logging}

### `--log-level` {#log-level}

`--log-level <LEVEL>`, one of `debug`, `info`, `warn`, `error`, `fatal`; default `info`. At `info` and below the server prints its start-up lines on stderr: the listening address, each `--local-storage` origin (`localStorage preload for <origin>: <n> entries`), each `--static` mapping (`serving <origin> from <dir>`) and each `--prepare` URL (`preparing a page at <url> per connection`). `warn` and above keep start-up quiet. Errors that end the server are printed at every level.

## Compatibility flags {#compatibility}

Accepted so that command lines written for Lightpanda keep working; the loader configuration is fixed, so they change nothing.

### `--experimental-features` {#experimental-features}

`--experimental-features <NAME>`, repeatable. Only `cors` is accepted; any other name is an error.

### `--load-resources` {#load-resources}

`--load-resources <KIND>`, repeatable. `stylesheet` or `iframe`; any other kind is an error. Stylesheets and iframes always load.

## Static files and storage seeding {#seeding}

### `--local-storage` {#local-storage}

`--local-storage <FILE>`, env `LIGHTNINGPANDA_LOCAL_STORAGE` (the flag wins). A JSON file preloaded into every browser context's `localStorage`:

```json
{
  "http://localhost:5173": { "token": "abc", "settings": { "theme": "dark" } },
  "https://app.example.com/any/path": { "lang": "en" }
}
```

- Top-level keys are origins, or URLs of which only the origin is used.
- Values are objects of entries. A string is stored as-is; any other JSON value is stored as its JSON text, so `settings` above reads back as the string `'{"theme":"dark"}'`.
- Every connection and every `Target.createBrowserContext` starts with a fresh copy; pages write to their own context only.
- An unreadable file or invalid JSON stops the server with exit `2`.

Per-profile storage from the SDK (`lp.profile(name, { localStorage })`) is an init script instead; see [Profiles](/guide/profiles).

### `--static` {#static}

`--static <ORIGIN>=<DIR>`, repeatable; env `LIGHTNINGPANDA_STATIC` with entries separated by `;` (flags add to them). Serves requests for `ORIGIN` from the directory `DIR`, like a static file server for a front-end build, so nothing needs to listen at that origin.

- `DIR/<path>` answers GET (and HEAD) requests; a directory answers with its `index.html`.
- A navigation to a path with no file gets `DIR/index.html` (single-page-app routes), unless [`--static-fallback network`](#static-fallback).
- Requests the directory cannot answer (no such file, POST and other methods, other origins) go to the network as usual, so API calls on the same origin still reach their server.
- Playwright routes, `mock` steps and CDP `Fetch` interception still see these requests (the overlay runs after interception and before the HTTP cache; its responses are not cached).
- Files are read into memory at startup (up to a budget) and served from there; files added later are looked up on disk. `..` traversal is refused.
- The entry splits at the first `=` after the origin's `://`; a malformed entry or unusable directory is an error (exit `2`).

```sh
lightningpanda serve --static http://app.test=./dist --static http://cdn.test=./public
LIGHTNINGPANDA_STATIC='http://app.test=./dist;http://cdn.test=./public' lightningpanda serve
```

In [fork mode](#fork) the template also precompiles the module scripts of the static roots once for every fork (`LIGHTNINGPANDA_FORK_PRECOMPILE`, see [env](/reference/env#lightningpanda-fork-precompile)). Guide: [Serving a build](/e2e/static).

### `--static-fallback` {#static-fallback}

`--static-fallback <spa|network>`, default `spa`. What a navigation to a missing path under a `--static` origin gets:

| Value | Behaviour |
| --- | --- |
| `spa` | `DIR/index.html`, so client-side routes work without a server. |
| `network` | The real server at the unchanged URL. Existing files stay local (a files-only overlay); that server owns any SPA routing and dynamic documents. |

## Proxy {#proxy-section}

### `--proxy` {#proxy}

`--proxy <URL>`. Sends every context's requests through this HTTP forward proxy: `[http://][user:pass@]host[:port]` (scheme defaults to `http`, port to `80`). HTTPS goes through `CONNECT`; plain HTTP uses absolute-form requests. Credentials (percent-decoded) are sent as Basic `Proxy-Authorization`. `direct://` means no proxy.

Not supported: SOCKS (`socks`, `socks4`, `socks4a`, `socks5`, `socks5h`), HTTPS proxies (`https://`), per-scheme lists (`http=…;https=…`), a path or query. These are errors at start-up (exit `2`).

A context's own proxy (Playwright `newContext({ proxy })`, `Target.createBrowserContext {proxyServer}`) replaces the server-wide one for that context. Guide: [Proxies, user agents, headers](/scraping/identity).

### `--proxy-bypass` {#proxy-bypass}

`--proxy-bypass <LIST>`, needs `--proxy` (alone it is an error). Hosts that skip the proxy, in Chrome's `--proxy-bypass-list` syntax: comma- or semicolon-separated `[scheme://]pattern[:port]` entries, `*` matching any run of characters, `<local>` for host names without a dot. Loopback addresses always bypass the proxy unless the list contains `<-loopback>`.

```sh
lightningpanda serve --proxy http://user:secret@proxy.corp:3128 --proxy-bypass '*.internal,<local>'
```

## Prepared pages {#prepared-pages}

Pages booted ahead of the test that takes them. Guide: [Profiles and pre-booted pages](/guide/profiles); protocol: [Pyro CDP domain](/reference/cdp-domain).

### `--prepare` {#prepare}

`--prepare <URL>`, repeatable; env `LIGHTNINGPANDA_PREPARE` with `;`-separated URLs (flags add to them). Each URL must be absolute (else exit `2`). Every connection boots a page at `URL` in its own browser context (with the [`--local-storage`](#local-storage) seed) before any command arrives, and boots the next one each time a test takes it.

A client takes a page with the browser-level command `Lightningpanda.acquirePreparedPage {url?, profileId?, attach?}` → `{targetId, browserContextId, sessionId?, ready}`. The page is announced like a new target (`Target.targetCreated` / `attachedToTarget`), so Playwright sees it as a page of its default context. `Lightningpanda.registerPreparedPage {url, initScripts?, viewport?, pool?}` declares more profiles at run time; `Lightningpanda.preparedPages` lists what is waiting. A `--prepare` profile keeps one page booted.

Giving `--prepare` turns on test-suite defaults: [`--spare-engine`](#spare-engine) and [`--render-prewarm`](#render-prewarm) default to `on`.

### `--prepare-refill` {#prepare-refill}

`--prepare-refill <quiet|after-close>`, default `quiet`; env `LIGHTNINGPANDA_PREPARE_REFILL` (only the value `after-close` has an effect). When the replacement for a taken page boots:

| Value | When | Cost |
| --- | --- | --- |
| `quiet` | Once the connection has no open page, or after 15 ms without commands | One extra booted app per profile while a test runs |
| `after-close` | Once the taken page's browser context closes | No extra app; the next test may wait for the boot |

Applies to `--prepare` profiles; `registerPreparedPage` takes its own `refillAfterClose`.

## Fork mode {#fork-mode}

### `--fork` {#fork}

`--fork`, env `LIGHTNINGPANDA_FORK=1`. Serves every connection from a `fork()` of a template process that booted the prepared pages once. Needs the patched JavaScriptCore of the release builds (`lightningpanda version` shows `fork: true`); otherwise the server exits `2` with `--fork needs the patched JavaScriptCore`.

- One template process boots one page per prepared profile: `--prepare` URLs and every `Lightningpanda.registerPreparedPage` from any connection. Registering a profile boots it in the template for the connections that follow.
- Each connection is a fork of the template: it starts with every profile's page booted, shares the template's memory until it writes to it, and its process exits when the connection closes. Open one connection per test; `Lightningpanda.capabilities` reports `{fork: true}`.
- A profile's `setup` (LP.run steps, `registerPreparedPage {setup}`) runs once in the template after the page loads; every connection starts from the state the steps left. A profile can derive from another (`base`), forking from its snapshot.
- Connecting to `ws://HOST:PORT/?profile=<id>` gives a connection forked from that profile's snapshot (what `lp.page({ as })` does).
- Each template keeps `LIGHTNINGPANDA_FORK_SPARES` (default `2`) forks waiting, so a connection does not wait for its fork.
- Forks run without JavaScriptCore's DFG/FTL tiers (connections are too short for them to pay off) unless `JSC_useDFGJIT` is set; see [env](/reference/env#jsc).
- The template starts while the server begins listening; the first connection waits for it. A template that fails to start ends the server (exit `1`).
- `--render-prewarm` does not apply (the template must not start threads before it forks).

**Shared profiles.** Several clients (test workers in other processes) can use one server. `registerPreparedPage {name, ...}` registers a profile under a name every client shares: the same definition under the same name finds it already booted; a different one is refused while another client holds it. `Lightningpanda.adoptPreparedPage {name}` → `{profileId, url, initScripts, viewport, setup, base}` holds a profile another client registered, `unregisterPreparedPage {name}` releases this connection's hold, and `Lightningpanda.preparedProfiles` lists the names with their holders. A connection's holds end when it closes; a profile is retired once no connection holds it and no other profile derives from it.

```sh
lightningpanda serve --fork --static http://app.test=./dist --prepare http://app.test/ --cdp-max-connections 128
```

Guides: [Connections and fork mode](/guide/fork-mode), [Fork mode in tests](/e2e/fork), [One server, many workers](/e2e/shared-server).

## Engine and render warm-up {#warm-up}

### `--spare-engine` {#spare-engine}

`--spare-engine <on|off>`; env `LIGHTNINGPANDA_SPARE_ENGINE=on|off`. Default: `on` with [`--prepare`](#prepare), `off` otherwise. Keeps one browser thread with its JavaScriptCore engine created ahead of the next connection: about 8 ms off that connection's first command, about 5 MB while idle. With `--prepare`, the spare thread also boots the prepared pages (for up to 30 s) before a connection takes it. Not used in fork mode.

### `--engine-reuse` {#engine-reuse}

::: info Newer than 0.0.33
`--engine-reuse` is on `main` (PR #304) and not in the 0.0.33 binary, which rejects it as an unknown option.
:::

`--engine-reuse <on|off|thread|always>`, default `on`; env `LIGHTNINGPANDA_ENGINE_REUSE`. What a connection that ends leaves to the next connection. At most one thread waits, for at most 10 s, then it ends and its engine is destroyed.

| Value | Kept for the next connection |
| --- | --- |
| `on` | The browser thread and its engine, only when the connection never made a page (an untouched engine). After pages, thread and engine are dropped. |
| `off` | Nothing: every connection gets a new thread and a new engine. |
| `thread` | The browser thread, also after pages; the engine is always destroyed and recreated on it. |
| `always` | The thread and the engine, also after pages, once the engine is proven to hold nothing of them (all code deleted, full collection, heap walk). Costs slightly more CPU than a new engine. |

Not used in fork mode. `LIGHTNINGPANDA_ENGINE_REUSE_LOG=1` prints one stderr line per connection end saying what became of its engine.

### `--render-prewarm` {#render-prewarm}

`--render-prewarm <on|off>`; env `LIGHTNINGPANDA_RENDER_PREWARM=on|off`. Default: `on` with [`--prepare`](#prepare), `off` otherwise. Scans the system fonts and warms the renderer on a background thread at startup, so the first screenshot or PDF does not pay for it (about 3 MB while idle). Ignored in fork mode.

## Rendering {#rendering}

### `--hyphen-digit-breaks` {#hyphen-digit-breaks}

`--hyphen-digit-breaks <on|off>`, default `off`; env `LIGHTNINGPANDA_HYPHEN_DIGIT_BREAKS=on|off`. Allows a line break between a hyphen and a digit when the hyphen follows a letter or digit (`4b87-|916e`, `10-|20`), as Chrome does; a minus sign stays with its number (`-120`, `A -5`). Applies to line breaking and min-content widths (table columns), so it changes layout, screenshots and PDFs. Off by default for stable baselines.

```ts
const lp = await Lightningpanda.launch({ args: ['--hyphen-digit-breaks', 'on'] });
```

Guide: [Rendering: screenshots and PDF](/guide/rendering).

## Lifetime {#lifetime}

### `--parent-pid` {#parent-pid}

`--parent-pid <PID>`; env `LIGHTNINGPANDA_PARENT_PID`. Exit when process `PID` exits, however it ends (crash and `SIGKILL` included), and remove fork-mode template sockets on the way out. Checked before anything slow: a parent already gone ends the server at once. `Lightningpanda.launch()` sets the variable to its own PID, so a launched server never outlives the test process. Must be a positive integer.

### Signals and shutdown {#signals}

There is no shutdown command: stop the server with a signal. In fork mode the server removes its templates' sockets when killed by a signal; forked connection processes exit when their connection closes. `Lightningpanda.launch()` starts the server in its own process group and `close()` sends `SIGTERM` to the group.

## Internal options {#internal}

`--template <socket>` makes the process a fork-mode template; the server passes it to itself. Do not use it directly.
