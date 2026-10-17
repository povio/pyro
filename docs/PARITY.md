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
| ES modules | `Module.zig`, `ImportMap.zig` | Partial (macOS) | Static/dynamic import, TLA and `import.meta.url` work. Missing: import maps, bare specifiers, `import.meta.resolve`, correct `import.meta.url` for root-relative imports, module preload, and Linux support. |
| Inspector | `v8_inspector` | Not started | CDP `Runtime.*` will be native (W3) on top of `describe`/`own_properties`/`call_function`. There is no `Debugger`/`Profiler`. |
| Termination / watchdog | `Watchdog.zig` | Partial | `TerminateHandle` aborts within ~50 ms. No stall watchdog policy. |
| Event loop | `Runner.zig`, `Scheduler.zig` | Partial | Tasks, timers, intervals, remote posting. Missing: HTML timer nesting clamp, task-source prioritization, idle callbacks. |
| Window timers | `webapi/Timers.zig` | Done | `setTimeout`, `setInterval` and their clears, `queueMicrotask`, `requestAnimationFrame` (fixed 16 ms tick) and `cancelAnimationFrame`. |
| Console | `webapi/Console.zig` | Partial | Levels are forwarded to hooks. `count`/`time` bookkeeping and format specifiers (`%s`) are not implemented. |
| Page lifecycle | `Browser/Session/Page/Frame.zig` | Partial | Pages, main frame, worlds, init scripts, navigation event ordering and realm disposal are done, for `about:blank` only. Missing: real document loading, iframes, history, same-document navigation, sessions/contexts, popups, dialogs. |
| CLI | `cli.zig`, `Config.zig` | Partial | `version` and `eval` work. `serve`, `fetch`, `mcp`, `agent` and `run` are missing, as are most flags. |
| Workers (Dedicated/Shared/Service) | yes | Not started | |
| Platforms | Linux, macOS | Partial | macOS only. |

## DOM / WebAPI / network (W2)

DOM, parser, selectors, events, collections, forms, CSS, storage, fetch/XHR/WebSocket, cookies, URL, encoding,
crypto, streams, canvas, IndexedDB and media are all **not started in this repository**. W2's branch tracks them.

## Automation (W3)

Detail per method: [automation/CDP_COVERAGE.md](automation/CDP_COVERAGE.md).

| Area | Upstream | Status | Notes / gaps |
|---|---|---|---|
| `serve` CLI, discovery, WebSocket | `server/Server.zig`, `WS.zig`, `http.zig` | Partial | `/json/version`, `/json/list` and the WebSocket on `/`. Implements upstream's connection/message limits and Host/Origin checks, plus request-head, frame and write deadlines and an outbound byte budget. TOS flags are accepted; `--experimental-features`/`--load-resources` have no effect until the loader lands. Missing: `--cookie`, `/metrics`, WebDriver BiDi, `--remote-allow-origins`. |
| Target/Browser | `domains/target.zig`, `browser.zig` | Partial | Flattened sessions, multiple contexts per process, a separate default context, and page filters. Missing: permissions, downloads, child targets (OOPIF/workers). |
| Runtime | v8_inspector | Partial | Native over lp-js: `evaluate`, `callFunctionOn` (`awaitPromise`/`returnByValue`), `getProperties`, release, context events, console, exceptions with V8-style descriptions. Missing: `addBinding`, object previews, `queryObjects`, `compileScript`. |
| Page | `domains/page.zig` | Partial | Frame tree, lifecycle, init scripts with worlds, isolated worlds, navigate/reload with upstream event order. Only about:blank loads. Missing: history, dialogs, screenshots/PDF, child frames. |
| Emulation | `domains/emulation.zig` | Partial | Viewport, media and user agent are recorded but not observable yet. Locale/timezone succeed only when they match the process. |
| DOM, Input, Network, Fetch, Storage, Accessibility | yes | Not started | Wire-level pieces exist (`fetch_protocol.rs`), but the domains need W2's DOM, faux layout and network. |
| BiDi, MCP, LP domain | yes | Not started | |
| TOS E2E harness | n/a | Partial | `bench/` (Playwright 1.59.1 smoke, startup, RSS, CDP recording proxy) and `tests/e2e/about-blank.mjs`. The TOS lane baseline is pending (see bench/README.md). |

## Required for the TOS E2E milestone (from tos-monorepo `apps/fe/e2e/LIGHTPANDA.md`)

A Playwright-compatible CDP surface, an accessibility tree with role/name, ESM with dynamic import, streaming
`fetch`, `localStorage`, `ResizeObserver`/`IntersectionObserver`, viewport-aware `matchMedia`, and stable execution
contexts across navigation.
