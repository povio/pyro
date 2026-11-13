# Proxies, user agents, headers

What a site sees of your browser: the IP address it connects from, the `User-Agent` and other request headers, and what scripts read from `navigator`, `Intl` and the screen. This page lists every setting Pyro has for those, which API sets it, and what is not supported.

## Quick reference

The Playwright library's context options do what Playwright's do. Each one sends the CDP command in the last column, which the native SDK, Puppeteer and raw CDP clients send themselves. Releases up to 0.0.33 accepted most of these options and applied only the first four.

| Setting | Playwright context option | Limits | CDP |
| --- | --- | --- | --- |
| User agent | `userAgent` | | `Emulation.setUserAgentOverride` |
| Request headers | `extraHTTPHeaders` | | `Network.setExtraHTTPHeaders` |
| Viewport | `viewport` | | `Emulation.setDeviceMetricsOverride` |
| Color scheme, reduced motion | `colorScheme`, `reducedMotion` | | `Emulation.setEmulatedMedia` |
| Proxy | `proxy` | HTTP proxies only | `Target.createBrowserContext { proxyServer }`, or `serve --proxy` |
| Locale | `locale` | `Intl` inside a worker keeps the host's locale | `Emulation.setLocaleOverride` (`navigator` and `Accept-Language` only) |
| Time zone | `timezoneId` | an error on the Intel macOS browser | `Emulation.setTimezoneOverride` (per connection) |
| Geolocation | `geolocation`, `permissions` | | `Emulation.setGeolocationOverride` + `Browser.grantPermissions` |
| HTTP Basic auth | `httpCredentials` | Basic only | `Fetch` auth, or an `authorization` header |
| Pixel ratio | `deviceScaleFactor` | | `Emulation.setDeviceMetricsOverride` |
| Mobile, touch | `isMobile`, `hasTouch` | no pinch zoom, no touch input | `Emulation.setDeviceMetricsOverride { mobile: true }`, `Emulation.setTouchEmulationEnabled` |
| Offline | `offline` | | `Network.emulateNetworkConditions` |
| Ignore TLS errors | `ignoreHTTPSErrors` | | `Security.setIgnoreCertificateErrors` |
| JavaScript off | `javaScriptEnabled: false` | | `Emulation.setScriptExecutionDisabled` |

A native SDK [profile](/guide/profiles) carries a user agent and a proxy of its own (`userAgent`, `proxyServer`, `proxyBypassList`), so each signed-in identity can have both without a CDP call. See [A proxy per context](#a-proxy-per-context).

In the Playwright library, a CDP command the options do not cover goes through a session on the page:

```ts
const page = await context.newPage();
const cdp = await context.newCDPSession(page);
await cdp.send('Emulation.setLocaleOverride', { locale: 'de-DE' });
```

In the native SDK, use `page.send(method, params)` for page commands and `pyro.cdp.send(method, params)` for browser-level ones. In Puppeteer, `page.createCDPSession()`.

## User agent

The default `User-Agent` is:

```
Mozilla/5.0 (compatible; pyro/X.Y.Z)
```

with the installed version. `navigator.userAgent` returns the same string. Many sites serve this user agent normally; some block it or serve a reduced page. Set a different one per context:

```ts
const context = await browser.newContext({
  userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36',
});
```

The override applies to the `User-Agent` header of every request the page makes (documents, scripts, stylesheets, `fetch`, XHR) and to `navigator.userAgent`, and it survives navigations. The CDP command behind it is `Emulation.setUserAgentOverride` (or `Network.setUserAgentOverride`). Over CDP it also takes `userAgentMetadata`, which drives `navigator.userAgentData` and the low-entropy `Sec-CH-UA`, `Sec-CH-UA-Mobile` and `Sec-CH-UA-Platform` headers (sent to secure origins only); `acceptLanguage`; and `platform`, which changes `navigator.platform`:

```ts
await page.send('Emulation.setUserAgentOverride', {
  userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36',
  acceptLanguage: 'en-GB',
  userAgentMetadata: {
    brands: [{ brand: 'Chromium', version: '147' }, { brand: 'Google Chrome', version: '147' }],
    platform: 'Linux', platformVersion: '', architecture: 'x86', model: '', mobile: false,
  },
});
```

A Chrome user agent changes only what the site is told. The engine is still JavaScriptCore, and a script that probes engine behaviour can tell. Pyro has no anti-detection mode.

`/json/version` and CDP `Browser.getVersion` report a Chrome 124 user agent string. That string is for CDP clients, so Playwright and Puppeteer take their Chrome code paths. Sites never see it.

::: tip Google Fonts and the user agent
Google Fonts picks the font files by user agent. For Pyro's default user agent it serves one TTF file per weight, without `unicode-range`. For a Chrome user agent it serves variable WOFF2 files split by script (latin, cyrillic, ...). Text renders either way, but screenshots and PDFs embed different font files. If you compare output with Chrome's, use a Chrome user agent.
:::

## Request headers

`extraHTTPHeaders` adds headers to every request of the context. Use it for `Accept-Language`, an API key the site expects, or a contact header:

```ts
const context = await browser.newContext({
  extraHTTPHeaders: { 'accept-language': 'de-DE,de;q=0.9', from: 'crawler@example.com' },
});
```

`context.setExtraHTTPHeaders(headers)` changes them later. Over CDP the command is `Network.setExtraHTTPHeaders`. The browser's own default headers are `Accept`, `Accept-Encoding: gzip, deflate, br`, `Accept-Language: en-US,en;q=0.9` and the `Sec-Fetch-*` set; headers you set replace defaults of the same name.

Extra headers go to every origin the page talks to, third parties included. Do not put secrets in them unless the page only talks to one site, or scope them with a [route](/scraping/extracting#intercepting-json-apis) that adds the header for one host.

## Proxies

Pyro supports HTTP forward proxies: `http://[user:pass@]host[:port]` (port 80 by default). HTTPS requests go through a `CONNECT` tunnel, plain HTTP requests in absolute form. **SOCKS proxies (`socks4`, `socks5`, `socks5h`) and HTTPS proxies (`https://` proxy URLs) are not supported**, and are refused with an error rather than silently ignored. Per-scheme lists (`http=a;https=b`) are refused too. `direct://` means no proxy.

### One proxy for the whole server

```sh
pyro serve --proxy http://user:pass@proxy.example.net:3128 --proxy-bypass '*.internal,10.0.0.0/8'
```

With `chromium.launch()` or `Pyro.launch()`, pass them as `args`:

```ts
const browser = await chromium.launch({ args: ['--proxy', 'http://user:pass@proxy.example.net:3128'] });
```

Every context of the server uses the proxy unless it has its own. `--proxy-bypass` takes Chrome's `--proxy-bypass-list` syntax, comma- or semicolon-separated:

| Rule | Bypasses |
| --- | --- |
| `example.com` | exactly `example.com` |
| `.example.com` | subdomains of `example.com`, not `example.com` itself |
| `*.example.com` | any host ending in `.example.com` |
| `*` | everything |
| `https://secure.test` | that host, HTTPS only |
| `host.test:8080` | that host on that port |
| `192.168.0.0/16`, `fd00::/8` | IP ranges |
| `<local>` | host names without a dot |
| `<-loopback>` | removes the default loopback bypass |

Loopback (`localhost`, `127.0.0.1`, `[::1]`) always skips the proxy, as in Chrome, unless the list contains `<-loopback>`. `--proxy-bypass` without `--proxy` is an error.

### A proxy per context

A browser context can have its own proxy. The CDP command is `Target.createBrowserContext` with `proxyServer` and `proxyBypassList`. A context with a proxy of its own does not use the server's `--proxy`.

Playwright library:

```ts
const context = await browser.newContext({
  proxy: { server: 'http://proxy-de.example.net:3128', bypass: '*.internal', username: 'user', password: 'pass' },
});
```

`server` is `host:port` or `http://host:port`. The credentials are sent to the proxy with the first request; Chromium waits for a 407. Loopback hosts go through the proxy too, as in Playwright, unless `bypass` lists them. `context.request` does not go through the proxy.

Native SDK, in a profile. The profile's pages send every request through the proxy, with the profile's user agent: the document, subresources, `fetch` and XHR, and the images, fonts and imported style sheets that layout, screenshots and PDFs load. In fork mode that includes the `setup` steps.

```ts
await pyro.profile('de', {
  url: 'https://example.com/',
  userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36',
  proxyServer: 'http://user:pass@proxy-de.example.net:3128',
  proxyBypassList: '*.internal',
});
const page = await pyro.page({ as: 'de' });
```

Both are part of the profile's definition: on a [shared server](/e2e/shared-server), a different user agent or proxy under a name another client holds is refused. A profile with a `base` has its base's.

Native SDK, without a profile, over CDP:

```ts
import { Pyro, css, goto } from '@povio/pyro';

const pyro = await Pyro.launch();
const cdp = pyro.cdp;
const { browserContextId } = await cdp.send('Target.createBrowserContext', {
  proxyServer: 'http://user:pass@proxy-de.example.net:3128',
  proxyBypassList: '*.internal',
});
const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank', browserContextId });
const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
const r = await cdp.send('Pyro.run', { steps: [goto('https://httpbin.org/ip').toStep(), css('body').read('ip').toStep()] }, sessionId);
console.log(r.reads.ip);
await cdp.send('Target.disposeBrowserContext', { browserContextId });
pyro.close();
```

Puppeteer:

```ts
const context = await browser.createBrowserContext({
  proxyServer: 'http://proxy-de.example.net:3128',
  proxyBypassList: ['*.internal'],
});
const page = await context.newPage();
await page.authenticate({ username: 'user', password: 'pass' }); // answers the proxy's 407
```

Releases up to 0.0.33 did not apply the Playwright library's `proxy` option, so the [profile store](/scraping/profiles#a-profile-store) and the [identity pool recipe](/recipes/scraping-identity-pool) start one server per distinct proxy: `chromium.launch({ args: ['--proxy', url] })`. That still works. Servers are cheap: a freshly started one measured about 8 MB resident on macOS, and each grows with the pages it holds.

### Proxy authentication

Two ways, both Basic only:

- **Credentials in the URL** (`http://user:pass@host:port`). They are sent as `Proxy-Authorization` on every proxied request. Percent-encode special characters in the password (`@` is `%40`).
- **A 407 challenge** answered over CDP: enable `Fetch` with `handleAuthRequests: true`, and answer `Fetch.authRequired` (with `authChallenge.source: "Proxy"`) with `Fetch.continueWithAuth`. Puppeteer's `page.authenticate()` does this. The answer is kept for the context.

Digest, NTLM and other schemes are not supported.

### Proxies that inspect TLS

Pyro trusts Mozilla's root certificates, built in. It does not read the operating system's certificate store. A proxy that re-signs HTTPS traffic with its own CA fails with `net::ERR_CERT_AUTHORITY_INVALID`. To accept it, turn certificate checks off for the context with CDP `Security.setIgnoreCertificateErrors { ignore: true }` on a page session (that page's context), or on the browser session for every context. Only do this for a proxy you control.

### Mapping identities to proxies

Give each identity one proxy and keep it. Write it in the identity's file next to its user agent (see [Stored profiles](/scraping/profiles#directory-layout)), and send every job of that identity through it. How you apply it depends on the API:

| API | How an identity gets its proxy | Tested |
| --- | --- | --- |
| Playwright library | one server; `browser.newContext({ proxy })` per identity. Or one server per distinct proxy, `chromium.launch({ args: ['--proxy', url] })`, with the identity's contexts on that server | the second way: [A pool of signed-in identities](/recipes/scraping-identity-pool). The `proxy` option was not run for this page |
| Native SDK | one server; a profile per identity with `proxyServer` and `userAgent` | not run for this page |
| Raw CDP | one server; each context created with its own `proxyServer` (above) | two contexts on one server, each through its own proxy with its own user agent |
| Puppeteer | one server; `browser.createBrowserContext({ proxyServer })` per identity | see [Puppeteer and raw CDP](/scraping/cdp#puppeteer) |
| Fork mode | one fork server; a profile per identity with `proxyServer` and `userAgent`. Or one fork server per distinct proxy | the second way: [Fork-mode signed-in jobs](/recipes/scraping-fork-signed-in-jobs#how-the-parts-work) |

Rotate jobs over identities, not proxies under one identity. A session that suddenly arrives from another IP address is a common reason for a site to end it, or to ask for a second factor. If your proxy provider changes the exit IP per request, use its "sticky session" option for signed-in identities. When an identity's proxy fails, pause that identity instead of moving it to another proxy.

## Locale

```ts
const context = await browser.newContext({ locale: 'de-DE' });
```

The Playwright library's `locale` sets `navigator.language`, `navigator.languages`, the `Accept-Language` header, and the default locale of `Intl` and the `toLocale*String` methods in documents. `Intl` inside a worker keeps the host's locale.

The CDP command alone does less:

```ts
await cdp.send('Emulation.setLocaleOverride', { locale: 'de-DE' });
```

`navigator.language`, `navigator.languages` and the `Accept-Language` header follow the locale (unless an explicit `acceptLanguage` override is set). `Intl` formatting and `Date.prototype.toLocaleString` keep the process locale, because JavaScriptCore's locale is process-wide; the Playwright library changes their default with a script that runs first in every document. An empty locale resets it. For `Accept-Language` alone, `extraHTTPHeaders` is enough.

## Time zone

```ts
const context = await browser.newContext({ timezoneId: 'America/New_York' });
```

`Date`, `Intl` and `Temporal` use the zone. The Playwright library puts the contexts of each zone on a CDP connection of their own, so contexts of one browser can be in different zones. The option is an error on the Intel macOS browser, which runs the system's JavaScriptCore.

Over CDP:

```ts
await cdp.send('Emulation.setTimezoneOverride', { timezoneId: 'America/New_York' });
```

The override applies to the whole connection (one JavaScript engine per connection), so every page on that connection sees it. Asking for a different zone while another page of the connection holds one fails with `Cannot set timezone`; use separate connections for identities in different zones. IANA names only; `GMT+02:00` style ids are refused. An empty id resets to the host's zone.

The host's zone is the default, so on a server set `TZ` in its environment to choose it for every connection.

## Geolocation

Geolocation needs both a position and the permission. In the Playwright library:

```ts
const context = await browser.newContext({
  geolocation: { latitude: 46.05, longitude: 14.51, accuracy: 10 },
  permissions: ['geolocation'],
});
```

`context.setGeolocation()`, `context.grantPermissions(permissions, { origin })` and `context.clearPermissions()` change them later. An unknown permission name throws `Unknown permission: <name>`.

Over CDP:

```ts
// native SDK
await pyro.cdp.send('Browser.grantPermissions', {
  permissions: ['geolocation'],
  browserContextId: page.browserContextId,
  origin: 'https://example.com',
});
await page.send('Emulation.setGeolocationOverride', { latitude: 46.05, longitude: 14.51, accuracy: 10 });
```

`Browser.grantPermissions` is a browser-level command; Puppeteer sends it with `context.overridePermissions`. The override applies to the whole browser context. Without the permission, `getCurrentPosition` fails with `User denied Geolocation`.

## HTTP Basic authentication

In the Playwright library, `httpCredentials: { username, password, origin? }` answers a page's `401` challenge with the Basic credentials once, for `origin` only when it is set. `context.request` sends them after a `401` too, or with every request with `send: 'always'`.

Without the option:

- Send the header yourself. Simple, but it goes to every origin:

  ```ts
  const context = await browser.newContext({
    extraHTTPHeaders: { authorization: 'Basic ' + btoa('user:password') },
  });
  ```

- Answer the challenge over CDP (`Fetch.enable { handleAuthRequests: true }`, then `Fetch.continueWithAuth`), which sends credentials only after a `401`. Puppeteer's `page.authenticate()` does this.

## Viewport, pixel ratio, mobile

`viewport` sets the layout size and `innerWidth`/`innerHeight`. The Playwright library's default is 1280 x 720; a native SDK page without a profile viewport, and `pyro fetch`, use 1920 x 1080.

`deviceScaleFactor` sets `devicePixelRatio`, and screenshots are in device pixels. `isMobile: true` lays the page out as Chrome's mobile emulation does: `<meta name="viewport">` decides the layout viewport, and a page without one lays out 980 px wide. `hasTouch` sets `navigator.maxTouchPoints` and the coarse-pointer media queries. There is no pinch zoom and no touch input: `tap()` is a click. See [Mobile emulation](/playwright/runner#mobile-emulation).

```ts
const context = await browser.newContext({
  viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true,
});
```

Over CDP the commands are:

```ts
await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: true });
```

### `devices`

`devices` from `@povio/pyro/wright` is Playwright 1.59.1's table of descriptors: `Desktop Chrome`, `Pixel 7`, `iPhone 15` and the rest.

```ts
import { chromium, devices } from '@povio/pyro/wright';

const context = await browser.newContext({ ...devices['Pixel 7'] });
```

A descriptor's `userAgent`, `viewport`, `deviceScaleFactor`, `isMobile` and `hasTouch` take effect. Its `defaultBrowserType` does not: every context runs on Pyro.

## Next

- [A pool of signed-in identities](/recipes/scraping-identity-pool): identity files with a proxy and user agent each, applied to every job.
- [Stored profiles](/scraping/profiles): keeping these settings per identity in a sidecar file.
- [serve options](/reference/serve): `--proxy`, `--proxy-bypass` and the rest.
- [CDP coverage](/reference/cdp): every Emulation, Network and Fetch command and its limits.
