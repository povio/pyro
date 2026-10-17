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
| Microtasks | explicit checkpoints | Partial | JSC drains microtasks when the outermost call returns. There is no explicit per-context queue control and no `queueMicrotask` ordering relative to native microtasks such as MutationObserver. |
| Unhandled rejections | `PromiseRejection.zig` | Partial | Reported through hooks. There is no `unhandledrejection`/`rejectionhandled` DOM event yet. |
| ES modules | `Module.zig`, `ImportMap.zig` | Partial (macOS) | Static/dynamic import, TLA and `import.meta.url` work, including in Window-global page realms. Missing: import maps, bare specifiers, `import.meta.resolve`, correct `import.meta.url` for root-relative imports, module preload, and Linux support. |
| Inspector | `v8_inspector` | Not started | CDP `Runtime.*` will be native (W3) on top of `describe`/`own_properties`/`call_function`. There is no `Debugger`/`Profiler`. |
| Termination / watchdog | `Watchdog.zig` | Partial | `TerminateHandle` aborts within ~50 ms and returns `JsError::Terminated`. Page code catching termination inside nested native frames is re-terminated within ~50 ms, not immediately. The CLI `eval --timeout-ms` uses it. No stall-watchdog policy. |
| Window global shape | V8 global template | Partial | Members are own properties of the global (default JSC global for modules). `Object.getPrototypeOf(window)` is `Object.prototype`. |
| Event loop | `Runner.zig`, `Scheduler.zig` | Partial | Tasks, timers, intervals, remote posting. Missing: HTML timer nesting clamp, task-source prioritization, idle callbacks. |
| Window timers | `webapi/Timers.zig` | Done | `setTimeout`, `setInterval` and their clears, `queueMicrotask`, `requestAnimationFrame` (fixed 16 ms tick) and `cancelAnimationFrame`. |
| Console | `webapi/Console.zig` | Partial | Levels are forwarded to hooks. `count`/`time` bookkeeping and format specifiers (`%s`) are not implemented. |
| Page lifecycle | `Browser/Session/Page/Frame.zig` | Partial | Pages, main frame, worlds, init scripts, navigation event ordering and realm disposal are done, for `about:blank` only. Missing: real document loading, iframes, history, same-document navigation, sessions/contexts, popups, dialogs. |
| CLI | `cli.zig`, `Config.zig` | Partial | `version` and `eval` work. `serve`, `fetch`, `mcp`, `agent` and `run` are missing, as are most flags. |
| Workers (Dedicated/Shared/Service) | yes | Not started | |
| Platforms | Linux, macOS | Partial | macOS only. |

## DOM / WebAPI / network (W2)

The DOM implementation lives in `lp-dom`, bindings in `lp-webapi`, and networking in `lp-net`.
Unit/integration tests exercise real JavaScriptCore and local HTTP. This is not yet TOS compatibility evidence.

| Area | Status | Notes / gaps |
|---|---|---|
| DOM tree, HTML parser, selectors | Partial | Arena identities, mutation records, html5ever incremental script yields, templates, HTML serialization and selector matching. Parser handles and queued resources survive collection. XML/DOMParser bindings not implemented. Character data currently uses Rust UTF-8 strings; lone UTF-16 surrogates are not preserved. |
| Events, collections, forms and input | Partial | Cross-realm wrappers, event dispatch/activation, listener lifecycle, forms/validation, keyboard/mouse input and basic collections. Range supports a subset; live range adjustment and several content manipulation methods remain unsupported. |
| Style/layout/media | Partial | CSS declaration/rule subset, cascade, computed style, matchMedia, upstream-style approximate geometry and hit testing. No full CSS layout/rendering. |
| Encoding, files and form data | Partial | TextEncoder/TextDecoder, base64, Blob/File/FormData primitives. Native DataView brand checks and typed-array subview offsets are covered. Streaming decoder support is incomplete. |
| HTTP, cookies and CORS | Partial | Local HTTP, redirects, compression, cancellation, cookie jar and CORS/preflight subset. HTTPS is explicitly unsupported in this build. Embedded PSL handles domain cookies and sites. Origin/credential/redirect and Secure-cookie regressions are covered, but full Fetch response filtering and network parity are not claimed. |
| Network observation | Partial | Bounded per-subscription event/logical-byte queues, ordered events, optional wake callback and explicit terminal overflow/closure. Overflow clears retained payloads; consumer must resynchronize. CDP body retention is separately owned by W3. |
| Fetch/XHR/Headers/URL JS bindings | Not started | Interrupted predecessor tasks performed discovery only; no implementation files recovered. Native HTTP support is not a fetch binding. |
| Storage and observers | Not started | No localStorage/sessionStorage, MutationObserver, ResizeObserver or IntersectionObserver JS bindings. Native mutation records exist. |
| Other platform APIs | Not started | WebSocket, crypto, streams, canvas, IndexedDB, workers and media APIs. |

Independent correctness/performance review is unresolved after provider-policy blocks; passing project tests do not satisfy that gate. Separate organization/cleanliness approval on the actual current head is also required before parent merge.

## Automation (W3)

The CDP server and its domains (Target, Page, Runtime, DOM, Network, Fetch, Input, Emulation, Accessibility, …),
BiDi, MCP and the TOS E2E harness are **not started in this repository**. W3's branch tracks them.

## Required for the TOS E2E milestone (from tos-monorepo `apps/fe/e2e/LIGHTPANDA.md`)

A Playwright-compatible CDP surface, an accessibility tree with role/name, ESM with dynamic import, streaming
`fetch`, `localStorage`, `ResizeObserver`/`IntersectionObserver`, viewport-aware `matchMedia`, and stable execution
contexts across navigation.
