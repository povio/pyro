# Scraping and automation

Pyro is a headless browser. It loads a page, runs its JavaScript, and lets you read the result. This section covers using it for scraping, signed-in automation, and making PDFs and screenshots. It does not cover testing.

The browser speaks the Chrome DevTools Protocol (CDP). You can drive it in four ways:

| Tool | When to use it | Page |
| --- | --- | --- |
| `pyro fetch` | One URL, rendered HTML on stdout, shell pipelines | [One-off fetches (CLI)](/scraping/fetch) |
| `@povio/pyro/wright` | Scripts written against Playwright's API: contexts, locators, `storageState` | [Scripting a browser](/scraping/library) |
| `@povio/pyro` (native SDK) | Many pages fast: a whole job is one command, and signed-in pages are booted ahead or forked | [Scripting a browser](/scraping/library), [Profiles booted once](/scraping/fork-profiles) |
| Puppeteer or raw CDP | Existing Puppeteer code, or any language with a WebSocket client | [Puppeteer and raw CDP](/scraping/cdp) |

## Use cases

**Crawling pages rendered by JavaScript.** A single-page app sends an almost empty HTML shell, and its scripts fetch the data and build the DOM. `curl` sees only the shell. Pyro runs the scripts, waits until the page is quiet, and gives you the DOM as the user would see it. See [Extracting data](/scraping/extracting).

**Signed-in scraping, one identity per account.** Sign in once per account and save the cookies and `localStorage` to a file. Later runs load that file into a fresh browser context and start signed in. Each identity can also keep its own proxy and user agent. This is the main topic of [Stored profiles](/scraping/profiles). With fork mode, the server can sign in once and give every job a fresh copy of that signed-in page. See [Profiles booted once (fork)](/scraping/fork-profiles).

**PDFs and screenshots.** The browser lays out and paints pages on the CPU, with no GPU and no Chrome. `page.pdf()` and `page.screenshot()` work. See [Screenshots and PDFs](/scraping/pdf).

**Low memory and fast start.** A server starts in milliseconds and holds one JavaScriptCore engine per connection. Closed pages return their memory to the operating system. In fork mode each job's page starts already booted in about 20 ms. See [Concurrency at scale](/scraping/concurrency).

## How it compares with Chrome

Pyro is not Chromium. It is a Rust browser on JavaScriptCore (Safari's JavaScript engine) with its own DOM and network stack, and Blitz/Stylo for layout and painting. Most sites that work in Chrome work here. Some do not.

What works:

- HTML parsing, the DOM, ES modules, `fetch`, XHR, WebSockets, cookies, `localStorage`, `sessionStorage`, IndexedDB (in memory), dedicated workers, service workers (partly), Web Crypto, Canvas 2D.
- HTTP/1.1 and HTTP/2, redirects, gzip/deflate/brotli, CORS, a forward HTTP proxy with Basic auth.
- Navigation, forms, clicks, typing, scrolling, file uploads, downloads, popups, dialogs.
- Request interception: mock, block, rewrite, and read response bodies.
- The accessibility tree: `getByRole`, `ariaSnapshot()`, CDP `Accessibility.*`.
- PDF and PNG/JPEG output, rendered by the browser itself.

What does not work, or works differently:

- **No persistent profile directory.** All storage is in memory and per browser context. There is no `--user-data-dir` and no `launchPersistentContext`. Save state to a file with `storageState` instead. See [Stored profiles](/scraping/profiles).
- **No SOCKS proxies and no HTTPS proxies.** Only `http://` forward proxies. See [Proxies, user agents, headers](/scraping/identity).
- **Some emulation is missing in the Playwright library.** The `proxy`, `locale`, `timezoneId`, `geolocation`, `httpCredentials`, `deviceScaleFactor`, `offline` and `javaScriptEnabled` context options are accepted but have no effect. Several of them can be set through a CDP session instead.
- **Layout and pixels are not Chromium's.** Positions can differ by a few pixels, and screenshots do not match Chrome's pixel for pixel. Text extraction is not affected.
- **No video, no tracing, no trace viewer, no `Page.startScreencast`.**
- **Other gaps.** Some web APIs are partial. A site that depends on one of them may break. [Web platform support](/reference/web-platform) and [CDP coverage](/reference/cdp) list the details.
- **Browser fingerprint.** The default user agent says `pyro`, and the JavaScript engine is JavaScriptCore, not V8. Sites that block automated or non-Chrome browsers can tell. Pyro has no stealth mode or captcha solving.

When a site fails, check it with Chrome first. If it works there and not here, see [Troubleshooting](/guide/troubleshooting).

## Platforms

The npm package `@povio/pyro` bundles the browser for:

| Platform | Fork mode |
| --- | --- |
| macOS 14+ on Apple silicon | yes |
| macOS 14+ on Intel | no (uses the system JavaScriptCore) |
| Linux x64, glibc 2.39+ (for example Ubuntu 24.04) | yes |
| Linux arm64, glibc 2.39+ | yes |

The JavaScript API runs on Node.js 22+ and on Bun. On other systems, run the browser on a supported machine and connect to it over the network. See [Platforms and limits](/guide/platforms) and [Running as a service](/scraping/service).

## Scraping responsibly

Pyro does what you tell it, as fast as you tell it. It does not read `robots.txt`, does not rate-limit, and does not identify you to a site beyond the user agent you send. Those are your job.

- **Read `robots.txt`** and follow it. Fetch it yourself before a crawl and skip disallowed paths. Respect `Crawl-delay` if the site sets one.
- **Rate-limit per site.** Fast start and many connections make it easy to send hundreds of requests a second. Cap concurrency per host, add delays, and back off on `429` and `503` responses. Block images, fonts and media you do not need (see [Extracting data](/scraping/extracting#cut-what-you-do-not-need)).
- **Read the site's terms.** Many sites forbid automated access, especially behind a login. Signing in with an account and scraping it may break the terms you agreed to when you made the account. Personal data falls under privacy law, such as the GDPR in the EU.
- **Identify yourself when you can.** A user agent with a contact URL (`MyCrawler/1.0 (+https://example.com/bot)`) lets site owners reach you instead of blocking you.
- **Prefer an API** when the site offers one.

This is not legal advice. When in doubt, ask the site owner or a lawyer.

## License

Pyro is licensed under AGPL-3.0-only. It is a derivative of Lightpanda. The AGPL adds one condition to the GPL: if you modify the program and let users interact with your modified version over a network, you must offer those users its source code (section 13). Your scripts run as separate programs and talk to the browser over CDP. See [Running as a service](/scraping/service#license) for more, and read the license itself or ask a lawyer for your case.
