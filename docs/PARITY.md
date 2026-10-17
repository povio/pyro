# Parity with upstream Lightpanda (2ad38773)

Upstream is about 266k lines of Zig. This rewrite is at an early milestone. Each status below reflects code that exists
and is tested in this repository, not plans.

Status values:
- **Done**: implemented and tested here.
- **Partial**: subset implemented; the gaps are listed.
- **Not started**.

## Runtime (W1)

| Area | Upstream | Status | Notes / gaps |
|---|---|---|---|
| JS engine | V8 + snapshot (`src/browser/js`, 13.3k lines) | Partial | JSC engine/realm/value/class layer done. No startup snapshot: each realm builds its classes at creation. Lazy installation is a planned optimization (not measured yet). |
| Host class bridge | `bridge.zig`, `Caller.zig` | Partial | Methods, accessors, constants, statics, constructors (`new.target`), indexed/named/setter/deleter/enumerator hooks, value iterators and upcasts are done. Missing: WebIDL argument conversion helpers (dictionaries, unions, overloads), `[Unforgeable]`, `[SameObject]` caching, `[CEReactions]`, pair iterators, `[LegacyUnenumerableNamedProperties]`. |
| Wrapper identity + GC | `Identity.zig`, weak finalizers | Done | Per-realm JSWeak maps; finalizers deferred to safe points. |
| Microtasks | explicit checkpoints | Partial | JSC drains microtasks when the outermost call returns. MutationObserver queues Promise reactions at native callback boundaries; parser/native changes use browser safe points. No explicit per-context queue control. |
| Unhandled rejections | `PromiseRejection.zig` | Partial | Reported through hooks. There is no `unhandledrejection`/`rejectionhandled` DOM event yet. |
| ES modules | `Module.zig`, `ImportMap.zig` | Partial (macOS) | Static/dynamic import, TLA and `import.meta.url` work, including in Window-global page realms. Missing: import maps, bare specifiers, `import.meta.resolve`, correct `import.meta.url` for root-relative imports, module preload, and Linux support. |
| Inspector | `v8_inspector` | Partial | CDP Runtime is native over lp-js (see Automation below). There is no `Debugger`/`Profiler`. |
| Termination / watchdog | `Watchdog.zig` | Partial | `TerminateHandle` aborts within ~50 ms and returns `JsError::Terminated`. Page code catching termination inside nested native frames is re-terminated within ~50 ms, not immediately. The CLI `eval --timeout-ms` uses it. No stall-watchdog policy. |
| Window global shape | V8 global template | Partial | Members are own properties of the global (default JSC global for modules). `Object.getPrototypeOf(window)` is `Object.prototype`. |
| Event loop | `Runner.zig`, `Scheduler.zig` | Partial | Tasks, timers, intervals, remote posting. Missing: HTML timer nesting clamp, task-source prioritization, idle callbacks. |
| Window timers | `webapi/Timers.zig` | Done | `setTimeout`, `setInterval` and their clears, `queueMicrotask`, `requestAnimationFrame` (fixed 16 ms tick) and `cancelAnimationFrame`. |
| Console | `webapi/Console.zig` | Partial | Levels are forwarded to hooks. `count`/`time` bookkeeping and format specifiers (`%s`) are not implemented. |
| Page lifecycle | `Browser/Session/Page/Frame.zig` | Partial | HTTP document loading, parser-blocking/deferred/async scripts, shared DOM in isolated worlds, init scripts, DOM lifecycle and disposal are wired. Contexts isolate network/storage; local storage survives same-origin navigation and session storage is page-local. Location/history support a subset. Linked stylesheets load with bounded source retention, link load/error events and parser/deferred-script/load blocking. Missing: child frames, image and script-inserted script resource loading, script load/error events, popstate/hashchange, full structured history state, popups and dialogs. |
| CLI | `cli.zig`, `Config.zig` | Partial | `version`, `eval`, `serve` and HTTP `fetch` with script execution/DOM dump work. `eval` and `fetch` enforce JavaScript deadlines. `mcp`, `agent` and `run` are missing, as are most upstream flags. |
| Workers (Dedicated/Shared/Service) | yes | Not started | |
| Platforms | Linux, macOS | Partial | macOS only. |

## DOM / WebAPI / network (W2)

The DOM implementation lives in `lp-dom`, bindings in `lp-webapi`, and networking in `lp-net`.
Unit/integration tests exercise real JavaScriptCore and local HTTP. This is not yet TOS compatibility evidence.

| Area | Status | Notes / gaps |
|---|---|---|
| DOM tree, HTML parser, selectors | Partial | Arena identities, mutation records, html5ever incremental script yields, templates, HTML serialization and selector matching. Parser handles and queued resources survive collection. [DOMParser](webapi/DOM_PARSER.md) parses `text/html`; XML parsing (DOMParser XML types, XMLDocument loading) is not implemented. Character data currently uses Rust UTF-8 strings; lone UTF-16 surrogates are not preserved. |
| XPath | Done | XPath 1.0 engine (all axes, node tests, predicates, unions, 27 core functions) with `document.evaluate/createExpression/createNSResolver`, `XPathEvaluator`, `XPathExpression` and `XPathResult` (all result types, mutation-invalidated iterators, namespace resolvers). Deviations: page-wide invalidation counter, no `Attr` identity, empty namespace axis, 32-level nesting cap; see [contract](webapi/XPATH.md). |
| Events, collections, forms and input | Partial | Cross-realm wrappers, event dispatch/activation, listener lifecycle, `window.event` (DOM current event; see [contract](webapi/WINDOW_EVENT.md)), forms/validation, keyboard/mouse input and basic collections. Range supports a subset; live range adjustment and several content manipulation methods remain unsupported. |
| Style/layout/media | Partial | CSS declaration/rule subset, cascade, computed style, matchMedia, upstream-style approximate geometry and hit testing. No full CSS layout/rendering. |
| Encoding, files and form data | Partial | TextEncoder/TextDecoder, base64, Blob/File bindings and native form serialization; `FormData` (form/submitter construction, entry list, iteration, multipart fetch/XHR bodies) — see [contract](webapi/FORM_DATA.md). Native DataView brand checks and typed-array subview offsets are covered. Streaming decoder support is incomplete. |
| HTTP, cookies and CORS | Partial | Local HTTP, redirects, compression, cancellation, cookie jar and CORS/preflight subset. Opt-in auth challenges support bounded server Basic retries; declining returns 401/407, unsupported credential schemes/proxy transport fail explicitly. No credential cache or authentication UI. HTTPS is explicitly unsupported in this build. Embedded PSL handles domain cookies and sites. Direct cookie insertion validates attributes atomically; enumeration excludes expired records. Page-local UA/language request defaults and navigator overrides are available for browser integration; see [contract](webapi/USER_AGENT.md). Origin/credential/redirect and Secure-cookie regressions are covered, but full Fetch response filtering and network parity are not claimed. |
| Network observation | Partial | Bounded per-subscription event/logical-byte queues, ordered events, optional wake callback and explicit terminal overflow/closure. Overflow clears retained payloads; consumer must resynchronize. CDP body retention is separately owned by W3. |
| Fetch/XHR/Headers/URL JS bindings | Partial | Headers supports records/iterables, ByteString validation, normalization, duplicate cookies, live sorted iteration and reentrant conversions. `fetch(url, init)` and network-created Response implement bounded callback-driven buffering, readers/clones, abort/disposal and response filtering; see [contract and limits](webapi/FETCH.md). Async XHR adds state/events, download progress, text/JSON/binary responses, timeout and disposal; see [XHR contract](webapi/XHR.md). URL parsing/components, live searchParams and `URL.createObjectURL/revokeObjectURL` registration are implemented (blob: fetch/XHR/navigation and downloads are not); see [URL contract](webapi/URL.md). The public Response constructor and `Response.json/redirect/error` are implemented for in-memory bodies; the Request constructor and streams remain unimplemented; sync/upload/XML XHR features remain explicit gaps. Browser installation supplies the context network client, owner-thread callback delivery and exact frame/document metadata; navigation disposes pending fetch/XHR state. |
| Web Storage | Partial | In-memory localStorage/sessionStorage, property access, key order, reentrant conversion and atomic per-origin UTF-16-byte quota. Embedders explicitly share local namespaces per context and session namespaces per top-level page; opaque-origin documents get per-document ephemeral local/session areas for upstream compatibility (upstream never throws; Chrome throws SecurityError — a documented Chrome deviation). Those areas are shared only by the worlds of that document, never via a common "null" bucket or across documents/pages/contexts, and are discarded on document replacement/disposal (retained `Storage` objects then read as empty and throw InvalidStateError on writes); IndexedDB still rejects opaque origins. No cross-document storage events or disk persistence. Lone UTF-16 surrogates share the current string-model limitation. |
| IndexedDB | Discovery only | `indexedDB.databases()` snapshots the context/origin catalog; this build has no database creator, so catalogs start and remain empty. Opaque origins reject. `open`, deletion, transactions and nonempty storage-state restoration are unsupported and fail explicitly; no supplied database state is discarded. See [discovery contract](webapi/INDEXEDDB.md). |
| Observers | Partial | MutationObserver records/options, coalesced microtask delivery, realm disposal and detached subtree lifetime are implemented; see [contract](webapi/MUTATION_OBSERVER.md). IntersectionObserver is partially supported over the faux layout (thresholds, root/rootMargin, overflow clipping, element/window scrolling, task delivery; explicit NotSupported limits) — see [contract](webapi/INTERSECTION_OBSERVER.md). ResizeObserver ports upstream's faux-size semantics with task delivery — see [contract](webapi/RESIZE_OBSERVER.md). Cross-PageDom observation and slotchange remain unsupported. `on*` event handler IDL attributes (GlobalEventHandlers/WindowEventHandlers) and `setAttribute('on*')` are implemented; parser-created inline handler attributes are not compiled — see [contract](webapi/EVENT_HANDLERS.md). |
| URL query parameters | Partial | URLSearchParams parsing, ordered mutations, live iteration and fetch/XHR form bodies, including live URL.searchParams association; see [contract](webapi/URL_SEARCH_PARAMS.md). |
| Web Crypto | Partial | `crypto.getRandomValues`, `crypto.randomUUID` and `crypto.subtle.digest` (SHA-1/256/384/512); other SubtleCrypto operations reject NotSupportedError; see [contract](webapi/CRYPTO.md). |
| Structured cloning | Partial | Bounded structuredClone supports ordinary graphs and selected builtins with proxy/host rejection. Transfers, Error/host serialization and complete altered-exotic classification remain unsupported; see [contract](webapi/STRUCTURED_CLONE.md). Existing window.postMessage/history are not upgraded to this serializer. |
| MessageChannel / MessagePort | Partial | Same-realm channels: structured-cloned messages are delivered in FIFO order, one task each, as trusted MessageEvents. `start()` and `onmessage` start a port; `close()` disentangles it and drops its undelivered messages; a started, entangled port is pinned until it is closed or the page is disposed. Transfers, including ports, throw NotSupportedError, and ports are not cloneable as data. See [contract](webapi/MESSAGE_CHANNEL.md). |
| Other platform APIs | Not started | WebSocket, streams, canvas, workers and media APIs. |

Independent correctness/performance review remains unresolved after provider-policy blocks. The user-authorized current merge gate is maintainability review of the actual head plus reported validation; tests are not independent correctness/performance approval. Parent owns review routing and merging.

## Automation (W3)

Detail per method: [automation/CDP_COVERAGE.md](automation/CDP_COVERAGE.md).

| Area | Upstream | Status | Notes / gaps |
|---|---|---|---|
| `serve` CLI, discovery, WebSocket | `server/Server.zig`, `WS.zig`, `http.zig` | Partial | `/json/version`, `/json/list` and the WebSocket on `/`. Implements upstream's connection/message limits and Host/Origin checks, plus request-head, frame and write deadlines and an outbound byte budget. TOS flags are accepted; `--experimental-features`/`--load-resources` have no effect until the loader lands. Missing: `--cookie`, `/metrics`, WebDriver BiDi, `--remote-allow-origins`. |
| Target/Browser | `domains/target.zig`, `browser.zig` | Partial | Flattened sessions, multiple contexts per process, a separate default context, and page filters. Missing: permissions, downloads, child targets (OOPIF/workers). |
| Runtime | v8_inspector | Partial | Native over lp-js: `evaluate`, `callFunctionOn` (`awaitPromise`/`returnByValue`), `getProperties`, release, context events, console, exceptions with V8-style descriptions. Missing: `addBinding`, object previews, `queryObjects`, `compileScript`. |
| Page | `domains/page.zig` | Partial | Frame tree, lifecycle, init scripts with worlds, isolated worlds, navigate/reload with upstream event order. HTTP documents load through the browser module. Missing protocol features include history, dialogs, screenshots/PDF and child frames. |
| Emulation | `domains/emulation.zig` | Partial | Viewport, media and user agent are recorded but not observable yet. Locale/timezone succeed only when they match the process. |
| DOM, Input, Network, Fetch, Storage, Accessibility | yes | Not started | Wire-level pieces exist (`fetch_protocol.rs`), but the domains need W2's DOM, faux layout and network. |
| BiDi, MCP, LP domain | yes | Not started | |
| TOS E2E harness | n/a | Partial | `bench/` (Playwright 1.59.1 smoke, startup, RSS, CDP recording proxy) and `tests/e2e/about-blank.mjs`. The TOS lane baseline is pending (see bench/README.md). |

## Required for the TOS E2E milestone (from tos-monorepo `apps/fe/e2e/LIGHTPANDA.md`)

A Playwright-compatible CDP surface, an accessibility tree with role/name, ESM with dynamic import, streaming
`fetch`, `localStorage`, `ResizeObserver`/`IntersectionObserver`, viewport-aware `matchMedia`, and stable execution
contexts across navigation.
