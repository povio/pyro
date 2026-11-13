# Persistent page identity

`Browser::set_user_agent_override(PageId, Option<pyro_webapi::UserAgentOverride>)`
updates a page's current navigator state and its native request defaults. It
returns `JsError::Type` for a missing page or invalid HTTP header value before
changing state. `None` clears every field; an empty `user_agent` selects the
context's default UA while supplied language/platform fields remain active.

The browser retains the override and prevalidated RequestDefaults per Page.
Every replacement PageDom receives the override before main/isolated world
installation and init scripts. Existing navigator wrappers in every world see
updates immediately. NetLoader snapshots the defaults for each new document,
classic/module script and stylesheet request; the WebAPI bindings snapshot the
same defaults for fetch/XHR. Already-started requests retain their snapshot.
Sibling pages, including those sharing the same context NetClient, are unchanged.
Explicit request and context extra-header precedence remains the native client's
contract; this API never mutates shared client settings.

Protocol parameter validation and Emulation.setUserAgentOverride mapping are
separate CDP work. `UserAgentOverride::metadata` (CDP userAgentMetadata) drives
`navigator.userAgentData` and the low-entropy Sec-CH-UA request defaults
(`RequestDefaults::client_hints`, potentially trustworthy origins only). This API
does not implement locale/timezone emulation or a platform change outside
navigator.platform.
Custom ResourceLoader implementations remain responsible for their transport.

A browser context may have a default UA of its own instead of the browser's
(`Browser::create_context_with(ContextNet { user_agent, proxy })`): the
configured `User-Agent` of its NetClient, which its pages and workers report as
`navigator.userAgent` unless a page override takes its place. Prepared profiles
(`Pyro.registerPreparedPage {userAgent, proxyServer}`) and
`Target.createBrowserContext {userAgent}` create their contexts this way.
What the engine loads for those pages by itself (layout's images, fonts and
imported sheets, screenshots, PDFs) goes through that NetClient too, with the
page's request defaults, and a context with a UA or proxy of its own lays out
only with bodies loaded through it (ARCHITECTURE.md, Engine loads of a page).

Tests cover current/new isolated worlds, init scripts across navigation, reset
and locale-only requests, atomic rejection, and real redirected HTTP document,
script/module, stylesheet, fetch and XHR requests from two pages in one context.
The TOS locale-only command is `{userAgent:"",acceptLanguage:"en-US"}`; browser
API tests are not a full TOS compatibility result.
