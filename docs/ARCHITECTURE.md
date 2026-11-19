# pyro architecture and module contract

pyro is a Rust rewrite of [lightpanda-io/browser](https://github.com/lightpanda-io/browser)
(reference checkout pinned at `2ad38773fd674d3c9cea26a263b814ee46f6ba84`). It uses **JavaScriptCore**
instead of V8. Its goal is fast, memory-efficient browser E2E testing through CDP (Playwright `connectOverCDP`).

This document is the interface contract between the three parallel workers. It is not a statement of parity:
see [PARITY.md](PARITY.md) for what exists today.

## Crates and ownership

| Crate | Owner | Depends on | Responsibility |
|---|---|---|---|
| `pyro-jsc-sys` | W1 runtime | patched source JSC (or system JSC) | Raw C API FFI. Runtime-resolved private SPI (`spi::get()`). C module-loader shim (source JSC) or ObjC shim (system JSC). |
| `pyro-js` | W1 | pyro-jsc-sys | Engine, realms/worlds, values, `Persistent`, host classes, errors, promises, modules, console, termination. |
| `pyro-runtime` | W1 | — | Single-threaded event loop: tasks, timers, `LoopHandle` cross-thread posting. |
| `pyro-browser` | W1 (W2 provides document loading) | pyro-js, pyro-runtime, later pyro-dom/pyro-net/pyro-webapi | Browser → Page → Frame tree, navigation state machine, loader ids, worlds, init scripts, window timers, ordered `PageEvent`s. |
| `pyro` (bin) | W1 | all | CLI: `version`, `eval`; `serve`/`fetch` once CDP/loading land. |
| `pyro-dom` | W2 DOM/net | — (no JS) | Arena DOM, `NodeId`, html5ever parsing, selectors, events, serialization. Unit-testable without JS. |
| `pyro-net` | W2 | — (no JS) | hyper/tokio HTTP client on an I/O thread, cookies, CORS, fetch model, request observers. |
| `pyro-webapi` | W2 | pyro-js, pyro-dom, pyro-net, pyro-canvas | JS bindings for DOM/network WebAPIs using `pyro_js::JsClass`. |
| `pyro-canvas` | W2 | — (no JS, no pyro crate) | Canvas 2D surfaces rasterized with vello_cpu (batched source-over, per-pixel compositing for the other operators, shadows, clip masks), canvas text with Parley, image decode/encode, and the process's system fonts (shared with pyro-render). |
| `pyro-cdp-protocol` | W3 automation | pyro-net | CDP wire protocol without the engine: WebSocket/HTTP server, messages, `Target` domain, `Network`/`Fetch`/`Runtime` translation helpers. Builds in parallel with pyro-dom/pyro-webapi/pyro-browser. |
| `pyro-cdp` | W3 automation | pyro-cdp-protocol, pyro-browser, pyro-js, pyro-webapi | CDP domains on the engine (`engine/`), the per-connection browser thread (`serve`, `conn`); re-exports pyro-cdp-protocol's modules. Runtime remote objects implemented natively. |
| `pyro-render` | W3 automation | pyro-canvas (fonts) | Screenshots: a page snapshot (HTML with form state, live sheets, viewport) rendered by Blitz (html5ever, Stylo, Taffy, Parley; vello_cpu, optional vello/wgpu) on a render thread per connection; display-list hash, pixel compare, PNG. Built only with the `render` feature. |
| `tests/e2e`, `bench/` | W3 | binary | Real TOS E2E compatibility and benchmarks. |

W1 owns the root `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml` and integration branches. Workers add dependencies
in their own crate manifests. W1 reconciles the lockfile on integration.

**V8 is forbidden.** `scripts/check-no-v8.sh` checks the dependency graph and sources.

## Threading model

- **One browser thread per CDP connection** (as upstream does). It owns the `pyro_js::Engine` (one JSC context group =
  one VM/heap), the DOM, and an `EventLoop<Browser>`. Every JS/DOM type is `!Send`. No locks guard JS or DOM state.
- **The CDP I/O thread** (tokio, current-thread) accepts sockets, answers the HTTP discovery endpoints and performs
  the WebSocket upgrade. It then hands the socket to the connection's browser thread and only holds the connection
  slot until that thread has finished (`SessionFactory::start_owned`).
- **The browser thread serves its own connection** (`pyro-cdp/src/conn.rs`): it reads, decodes, runs and answers each
  command, and writes replies and events, all on that thread. The non-blocking socket is registered with the
  browser loop (`pyro_runtime::FdWatch`), so an idle thread sleeps in one `poll(2)` on its wake pipe and the socket.
  Between loop tasks the connection is serviced (read at most every 100 µs while page work keeps the loop busy, at
  once when the socket wakes it). While JavaScript runs, the engine watchdog polls the connection
  (`Engine::set_interrupt_check`, about every 50 ms): it keeps reading and decoding, answers pings, and stops the
  script when the client closes or leaves, the outbound budget is exceeded, or the session takes nothing for the
  stall timeout. Guarantees kept from the channel design: in-order handling with at most 64 decoded messages
  waiting; replies and events written in production order, with pongs and close frames at frame boundaries; the
  64 MiB outbound budget (frames are pulled from the ordered outbound channel one at a time, so it counts what the
  socket has not taken) closing with 1008; frame, write, stall and close timeouts.
- **Other I/O threads** (pyro-net's tokio runtime for HTTP) post work to the browser thread with
  `LoopHandle::post(Box<dyn FnOnce(&mut Browser) + Send>)`. The loop processes posted work in arrival order and
  writes its wake pipe only while asleep. `post` returns `Err(LoopClosed)` once the browser thread has exited.
- **At most one `Engine` per thread** (enforced). Callbacks find their engine and realm through thread-local state.
- **A dedicated worker is a thread of its own**, with its own `Engine` and event loop; it shares no JS or DOM state
  with its page, and messages cross as serialized tasks (`docs/webapi/WORKER.md`).
- On connection close the browser thread runs `Browser::close_all()`, which disposes the realms and cancels the
  timers, then writes what is still queued and a close frame within the close timeout, and shuts the socket.

## pyro-js contract (implemented, tested)

```rust
let engine = Engine::new()?;                    // per thread; errors if JSWeak SPI missing
engine.register::<MyClass>();                   // before creating realms that expose it
let realm = engine.create_realm(RealmOptions { name, global_class: Some(id), origin })?;
realm.set_global_data::<Window>(data)?;
let s = realm.scope()?;                         // Scope<'_>: ctx + realm
let v = s.eval(src, Some(url), line)?;          // Value<'s>
let p = s.eval_module(src, url)?;               // promise; imports via ModuleFetcher
let r = s.call_function("function(a){...}", Some(this), &[arg])?;   // CDP callFunctionOn shape
let o = s.wrap::<C>(data, Some(IdentityKey(doc, node)))?;           // one wrapper per key per realm
let d = s.unwrap::<C>(value);                   // Option<Unwrapped<C::Data>>: protects the wrapper while borrowed
let d: Option<&C::Data> = s.unwrap_arg::<C>(&args, 0);              // callback args are rooted by the frame
let w: Option<&Window> = s.global_data::<Window>();                 // realm global payload (set once)
s.set_hidden(obj, "listeners", v)?; s.get_hidden(obj, key)?;        // GC-traced, cycle-safe
let f = s.function("name", |s, this, args| Ok(s.undefined()))?;     // capturing native closure
s.then(value, |s, v| .., |s, err| ..)?;  let d = s.deferred()?;     // promises without nested loops
s.describe(v)?; s.own_properties(obj, own_only)?;                   // CDP RemoteObject helpers
s.report_exception(&err);                                           // -> EngineHooks::exception_thrown
engine.set_hooks(Some(Rc<dyn EngineHooks>));  // console_api / exception_thrown / unhandled_rejection / realm_*
engine.terminate_handle()?.terminate();       // Send; aborts running JS within ~50 ms -> JsError::Terminated
let keep = Persistent::new(&s, v);            // JSValueProtect; !Send; Clone; Drop unprotects
```

### Host classes

```rust
struct Node;
impl JsClass for Node {
    const NAME: &'static str = "Node";
    type Data = NodeRef;                // Rust state per wrapper (interior mutability)
    type Parent = EventTarget;          // or NoParent
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|n: &NodeRef| &n.target);   // only if Data differs from the parent's
        def.constant("ELEMENT_NODE", 1.0);
        def.getter("nodeName", |s, this| Ok(s.string(&this.name())));
        def.accessor("textContent", get, set);
        def.method("appendChild", 1, |s, this, args| { ... });
        def.constructor(0, |s, new_target, args| s.wrap_new::<Node>(data, new_target, key)); // else "Illegal constructor"
        def.indexed(|s, this, i| ..);  def.named(override_builtins, |s, this, name| ..);
        def.named_setter(..); def.named_deleter(..); def.property_names(..); def.indexed_iterable();
    }
}
```

- Callbacks are plain fn pointers (non-capturing closures coerce). Each member carries its fn pointer plus a per-class
  invoker (`pyro-js` `MemberCall`): one `extern "C"` entry for all members, one invoker per class and member kind. A
  call reads the pointer from the function's data and looks nothing else up. With the patched JSC an operation is an
  internal function made by `JSObjectMakeFunctionWithCallbackAndData` and an attribute a custom accessor with a
  trampoline per (class, attribute) (see "JavaScriptCore source build", patch `0012`); with the system framework they
  are callback objects, as before. One full trampoline per member was the earlier design; it made pyro-webapi's codegen
  serial and the binary 6 MB larger.
- `def.pure_getter` / `pure_getter_typed` / `def.pure_method` declare a member that runs no script, changes nothing
  script can observe and may be run again after an error (tree navigation, `nodeType`, `length`, `matches`,
  `getAttribute`, `querySelector`). Such a member takes the lean dispatch (`accessors.rs` `lean_getter_h`,
  `class.rs` `pure_member_call`, `get_by_index_lean` for indexed getters): the scope comes from the realm the
  receiver's payload keeps (`Header::realm_ref`), the data from the payload's class chain (boxed `ClassInfo`
  entries, no registry), and it skips what exists for callbacks that can run script or mutate: the nesting depth,
  the termination state, the post-callback hook. An error or a panic reruns the member under the full dispatch,
  which reports it. With patch `0013` a pure getter is also a DOM attribute to the JITs (below).
- Panics are caught at the boundary and become `Error("Internal error: …")`.
- The interface shape follows WebIDL:
  - accessors live on the prototype, so `Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')` works as React needs;
  - methods are real `Function`s;
  - each prototype has a `constructor` and `Symbol.toStringTag`;
  - static inheritance holds (`Object.getPrototypeOf(Node) === EventTarget`);
  - subclassing works through `new.target` (custom elements).
- **Brand checks** use the class id stored in the payload header, never the mutable JS prototype chain. A wrong receiver throws
  `TypeError: Illegal invocation`.
- **Errors:** `JsError::{Type, Range, Syntax, Reference, Dom{name,message}, Thrown(Persistent), Terminated,
  ContextDestroyed, Unsupported}`. A `Dom` error becomes a `DOMException` with the right `code`.
- **Global object:** realms always use JSC's default global object, because only it supports the module loader (a
  custom global `JSClass` rejects modules with "Context does not support module loading"). The default global's
  `[[Prototype]]` is immutable, so `global_class: Some(Window)` is emulated:
  - the members of Window and its ancestors are copied onto the global as own properties, as WebIDL does for `[Global]`
    interfaces;
  - each interface in the chain gets a `Symbol.hasInstance` that also accepts the global;
  - `Symbol.toStringTag` is set to `"Window"`.

  Deviations from browsers: `Object.getPrototypeOf(window)` is `Object.prototype`, and patching
  `EventTarget.prototype.addEventListener` does not affect the copy that lives on `window`. A receiver that is
  `undefined`, `null` or the global maps to the global's payload. That payload is set once
  (`Realm::set_global_data`) and freed only when the realm is dropped.

### GC and lifetime rules (review focus)

1. **`Value<'s>`/`Object<'s>` are stack-only.** JSC scans the native stack conservatively, so only *live* locals are
   roots. Anything on the heap must be a `Persistent` or `RootedVec`. Argument buffers of up to 8 values stay on the
   stack. A borrow derived from a value must not outlive the value's liveness: `Scope::unwrap` returns a guard that
   protects the wrapper for as long as the borrow lives.
2. **Finalizers run no Rust destructors and make no JSC calls.** JSC may finalize on any thread, so the finalizer only
   pushes the payload pointer onto the owning engine's mutex-protected queue. `Engine::drain_finalized()` drops
   payloads and prunes dead identity entries on the engine thread at safe points; the loop calls it after every task.
   Wrapper payloads may therefore safely own `Persistent`s.
3. **Identity maps** are per realm: `IdentityKey -> JSWeak`. `JSWeakGetObject` returns null once the GC has found the
   object dead, so a dead wrapper is never handed out again.
   A node keeps its own wrapper (patch `0014`): `pyro-webapi` `PageDom::node_wrappers` holds, by node index, the
   `pyro_js::WrapperRef` of the wrapper in up to two script worlds (the page's and one isolated world; a third world's,
   and every wrapper under the system framework, go through the identity map as before). Such a wrapper is made with
   `IdentityKey::OWNED`: it is not in the identity map, its payload header carries a weak handle made by
   `JSWeakCreateKeptWhile` and a keep byte, and the collector asks that byte, for a wrapper nothing else reaches,
   whether the owner still wants it. Finding a node's wrapper is a load from the table and a read of the handle; no
   hash map, no pin entry, no protected value, and collections do not walk a table of kept wrappers. The entry is
   removed when the payload is dropped (`NodeData::drop`), which holds a node reference until then, so an index never
   names another node. A realm links its owned payloads through their headers and releases their handles when it is
   disposed (they must not outlive the VM).
   The keep byte is set while the node is in the active document (`wrap_node`; `reconcile_pins` clears it from the
   removal log at the next safe point, and sets it again for a wrapper that holds listeners or properties, which
   patch `0011` reports, when its node comes back). Other browsers keep such wrappers too. Without it, a script that
   walks the document (a selector engine reads every element per query) had its wrappers collected and made again
   each time: 15% of a Playwright locator was `wrap_node` and 13% the collector. A wrapper made for a detached node
   can still be collected and made again without script noticing. Because a page's wrappers all die after it is
   navigated away from, in many small batches, each document counts its externally referenced nodes (`pyro-dom`
   `DocumentData::held`): `Dom::collect_garbage` skips a document that still has one without walking its tree.
4. **Prefer hidden properties to `Persistent`** for state a wrapper owns (event listeners, expandos the DOM needs).
   `Persistent` roots are invisible to cycle collection.
5. **`Realm::dispose()`** (navigation) releases the global and the prototypes, empties the identity map, and
   unregisters the realm. Any later call into one of its native functions throws `Execution context was destroyed.`
   instead of touching freed state. Timers hold realms weakly.
6. **`Engine` is the unique owner of the VM.** Dropping it marks the engine as gone, so native callbacks into its
   realms throw `Execution context was destroyed.` and a new engine can be created on the thread. The VM itself is
   torn down only once no `Realm` handle remains. Every `Scope`, `Value`, `Object` and `Unwrapped` borrows one, so
   none can outlive the VM. Teardown disposes the realms, destroys the VM (finalizing every object) and drains the
   payloads. `Persistent`s hold the engine weakly; after teardown they are inert, so dropping them is always sound.
7. **Every `extern "C"` entry point** (trampolines, property hooks, closures, console, module fetch) runs realm lookup,
   the body, error conversion and realm-handle release inside one `catch_unwind`. A panic becomes
   `Error("Internal error: …")` and never unwinds into JSC.
8. **Termination:**
   - The C API cannot propagate JSC's uncatchable termination through native frames: a value rethrown from a callback
     is catchable. When native code sees a termination, it rethrows the termination value and re-arms the watchdog,
     so JS that catches it is terminated again within about 50 ms.
   - Native code that ignores an inner `Terminated` cannot continue the calling JS either: while termination is
     pending, a callback's `Ok` result is converted to `Terminated` as well.
   - Back at depth 0, the call returns `JsError::Terminated`, and JSC's pending termination state is flushed so it
     cannot abort the next entry.
   - Each watchdog poll is a VM trap. `webkit/patches/0021-vmtraps-walk-only-after-breakpoints.patch` stops
     `VMTraps::handleTraps` from walking every code block of the heap on each trap: the walk jettisons code blocks
     carrying trap breakpoints, which only the signal sender installs (never with the polling traps pyro
     uses), so it now runs only after one was installed. With a 2 MB bundle loaded, that walk was about 10% of a
     paginating document's script time.
9. **Heap-collected call arguments** (for example CDP `callFunctionOn` converting arguments one by one, where each
   conversion can GC) must use `RootedVec` with `Object::call_rooted` or `Scope::call_function_rooted`, never a
   `Vec<Value>`.
10. **`Number.prototype.toLocaleString(locales, options)`:** JSC built a new ICU formatter per call, and the first
    format on one loads `DecimalFormatSymbols` from the system ICU's locale data: about 176 µs a call, against
    Chrome's 12 µs with its trimmed ICU (pdfgen formats every amount this way). `webkit/patches/0022-to-locale-string-shares-number-formatters.patch`
    still reads the options on every call, as the spec requires (getters and proxies observe the reads), but formats
    with the ICU formatter of one of the last four calls whose options resolved to the same skeleton and data
    locale, as JSC already did for `toLocaleString()` and `toLocaleString(locale)`: 2 µs a call. Formatting does not
    observe which formatter formats. A language change (the `languagesEpoch`) drops them.

### Private SPI (runtime-resolved, capability-gated)

Every private entry point is resolved with `dlsym` (`pyro_jsc_sys::spi::get()`); none is linked. A missing symbol
yields `JsError::Unsupported` or a feature that is reported as absent (`Engine::capabilities()`). It never causes a
load failure or a crash.

| Capability | Symbols | Use | If missing |
|---|---|---|---|
| `weak_refs` | `JSWeakCreate/Release/GetObject` | wrapper identity | `Engine::new` fails: identity is required |
| `modules` | ObjC `JSScript` (module type), `-[JSContext setModuleLoaderDelegate:]`, `evaluateJSScript:` | ES modules, `import()` | `eval_module` → `Unsupported` |
| `unhandled_rejections` | `JSGlobalContextSetUnhandledRejectionCallback` | `unhandledrejection` / CDP exceptions | no reports |
| rejection tracker (patch 0036) | `JSGlobalContextSetPromiseRejectionTracker` | HTML's rejected promise lists (`pyro-js` `rejections.rs`): `unhandledrejection` from a task, not for a promise handled before it runs; `rejectionhandled` | rejections are reported as the microtask queue empties; no `rejectionhandled` |
| `termination` | `JSContextGroupSetExecutionTimeLimit` | `TerminateHandle` | `terminate_handle()` → `Unsupported` |
| `hidden_properties` | `JSObject{Get,Set,Delete}PrivateProperty` | listener storage | `set_hidden` → `Unsupported` |

Optional extras: `JSReportExtraMemoryCost` and `JSSynchronousGarbageCollectForDebugging`.

### ES modules (macOS only; works in host-global page realms)

- **Verified on this host:**
  - static imports with live bindings;
  - `import.meta.url`;
  - top-level await;
  - `import()` from both modules and classic scripts;
  - rejection of failed fetches.
- **How fetching works:** module fetching calls `ModuleFetcher::fetch(realm, url, ModuleRequest)`, which resolves or
  rejects asynchronously.
- **Measured caveats:**
  - **Script without a URL (patched JSC):** a specifier whose script has no URL to resolve against (CDP evaluate, an
    init script) resolves against the document base URL, read through `ModuleFetcher::document_base_url` when needed.
    It therefore gets the same module instance as the document's own scripts.
  - **Root-relative specifiers (system framework; the patched JSC resolves them against the referrer):** JSC resolves
    `"/x.js"` against `file:///`. The runtime fetches the document-origin
    URL, but the module is registered under `file:///x.js`, so that module's `import.meta.url` reads `file:///x.js`.
    Only `file:///…` (empty authority) is remapped; `file://host/…` stays unchanged. An explicit `import("file:///x")`
    from an http page is therefore also fetched from the page origin.
  - **Bare specifiers and import maps:** `webkit/patches/0005-import-maps.patch` adds `JSGlobalContextAddImportMap`, which
    feeds JSC's own `ImportMap` (HTML parse, resolve and merge), and consults it in `moduleLoaderResolve` once a context
    has a map. It also adds `import.meta.resolve`. The system framework has no such hook: there these stay unsupported.
  - **Import attributes (JSON and CSS module scripts):** `webkit/patches/0020-module-import-attributes.patch` adds
    `JSGlobalContextSetTypedModuleLoader`, whose fetch callback gets the request's `type` attribute (JSC keys its
    module map by URL and type), and `JSModuleSourceCreateWithType` for JSON sources. A CSS request is resolved with
    the embedder's `CSSStyleSheet`, which becomes the module's `default` export; JSC's `moduleTypeIsAllowed` host hook
    allows `css`, and the patch flips the generated JSC option defaults so `css` parses (`useCSSModuleScripts`) and
    `text` does not (`useImportText`, not an HTML module type). `ModuleRequest::module_type` carries the type to the
    fetchers (`pyro-browser` `PageModuleFetcher`, `pyro-webapi` `WorkerModuleFetcher`), which check the response MIME type
    (`pyro_webapi::module_script`) and complete with `resolve`, `resolve_json` or `resolve_object`. The system framework's
    delegate never sees the attribute: every request looks like JavaScript, and a JSON or CSS response fails the MIME
    check with a message naming the missing patch.
  - **Inline modules:** the registry is keyed by URL, so each inline module script needs a distinct URL. The browser
    appends a fragment.
  - **Linux:** WebKitGTK's JSC has no module loader, so **ESM on Linux is unsupported** until a tested alternative
    exists.

## pyro-runtime contract (implemented, tested)

`EventLoop<S: LoopState>`:
- `scheduler()` → `Scheduler<S>` (`Rc`), with `post(source, FnOnce(&mut S))`, `set_timer(delay, repeat, keeps_alive, Fn(&mut S)) -> TimerId`, `clear_timer` and `stop`.
- `handle()` → `LoopHandle<S>` (Send), with `post` and `Err(LoopClosed)`.
- `run_one`, `run_until(state, deadline, stop_when_idle, done)`, `run_until_idle`.
- After every task it calls `LoopState::after_task`.

## pyro-browser contract (partially implemented)

Implemented:
- `BrowserThread::new()`;
- `Browser::{create_page, navigate(page, NavigateOpts{url, reason, cdp_request_id, referrer}), add_init_script(page, InitScript{id, source, world}), remove_init_script, isolated_world(page, name), close_page, close_all}`;
- `PageObserver::on_page_event(page, &PageEvent)`. The events mirror upstream `Notification.zig`: `FrameNavigate`, `FrameNavigated`, `FrameNavigatedWithinDocument`, `FrameNavigateFailed`, `RealmCreated`/`RealmDestroyed`, `FrameDomContentLoaded`, `FrameLoaded`, `FrameNetworkAlmostIdle`, `FrameNetworkIdle`, `FrameChildFrameCreated`/`Detached`, `FrameDestroyed` and `JavascriptDialogOpening`.

Order on navigation:
1. `FrameNavigate`.
2. Old realms are disposed (`RealmDestroyed`).
3. New realms are created for the main world and every init-script world. Init scripts run before any page script.
4. `RealmCreated` per realm.
5. `FrameNavigated`, then lifecycle events.

**Only `about:blank` loads today.** Unlike the pinned upstream build, it has an executable realm. Other URLs emit
`FrameNavigateFailed("net::ERR_NOT_IMPLEMENTED …")` until W2's loader is integrated.

## JavaScriptCore source build

`pyro-jsc-sys` links a patched JavaScriptCore built from WebKit source, so the engine can be changed (the goal is
copy-on-write page snapshots: load an app's JS once, then start each test from that state). Inputs live in
`crates/pyro-jsc-sys/webkit/`: `REVISION` (WebKit commit) and `patches/*.patch`, kept small and few, like Bun's fork.

- `scripts/jsc/build.sh` builds WebKit's portable JSCOnly CMake port (the Apple framework build needs full Xcode)
  into `~/Library/Caches/pyro/jsc/<revision>-<patch hash>/lib/libJavaScriptCore.a`, with a copy of
  the inputs: one static archive of JavaScriptCore, its JIT subtarget's objects, WTF and bmalloc
  (`scripts/jsc/archive.sh`). Needs `brew install cmake ninja`. A clean build took a few minutes at `-j10` on an M1 Max; after a
  patch change ninja rebuilds incrementally.
- The archive is linked into the binary (static since 2026-11). As a 48 MB dylib it cost dyld 1.2 ms of every
  launch on the M1 Max (process start to `main` 7.2-7.7 ms, 6.0-6.1 ms linked in) and was a second file to ship,
  sign and find at run time; the release binary grew from 57.0 MB to 99.1 MB where binary plus dylib were 105.1 MB.
  ICU (`libicucore` on macOS, the distribution's on Linux) and the C++ runtime stay shared. The SPI is referenced
  directly instead of through `dlsym`, so a missing entry point is a link error. A directory that holds only
  `lib/libJavaScriptCore.dylib` (an older build) is still linked dynamically on macOS.
- `build.rs` links the cached build whose inputs match the checkout. Without one it warns and falls back to the
  system framework (patched-only entry points are then reported unsupported). `PYRO_JSC=system` forces the system
  framework, `PYRO_JSC_DIR=<dir>` uses `<dir>/lib/libJavaScriptCore.a`.
- JSCOnly has no Objective-C API, so patch `0001` adds a C module-loader SPI (`JSGlobalContextSetModuleLoader`,
  `JSModuleSourceCreate`, `JSLoadAndEvaluateModule`) to `JSAPIGlobalObject`, ported from `JSAPIGlobalObject.mm`.
  `shim/modules_source.c` maps the crate's `pyro_jsc_module_*` interface onto it.
- Patch `0002` makes `fork()` safe for a process that has run JavaScript (see "Fork mode"): `JSPrepareForFork`
  finishes a collection under way and the JIT plans, then parks every JSC helper thread (WTF `AutomaticThread`s
  are told to exit and joined; the libpas scavenger and prefault threads are suspended and joined).
  `JSDidFork(child)` resumes them; in the child it also refreshes the thread's mach port, marks the allocator
  caches of threads that did not survive as orphans, restirs WTF's random generator and restarts the VMTraps
  queue. `JSGlobalContextReseedRandom` gives a context a new `Math.random()` seed.
- Patch `0003` adds `JSCompleteJITPlans(group, wait)`. A finished concurrent JIT plan stays on the worklist until the
  next tier-up check in the VM, and the GC marks its CodeBlock, so a function that tiered up just before its page
  closed kept that closed realm through every collection until then, an effect that looked like class-registration
  order or stack noise. `Engine::settle` (page close, boot) installs finished plans before collecting without waiting
  for running ones; `Engine::collect_garbage_now` (tests) also waits, so retention checks depend on reachability alone.

- Patch `0006` adds `JSContextGroupSetTimeZone(group, id)`, behind `Emulation.setTimezoneOverride`. `DateCache` is per  VM, and `Intl` and `Temporal` read their default zone from it, so one override field there covers them; setting it  marks the cache stale, and JSC's own host-zone-change path applies it at the next outermost entry (date caches  flushed, cached local times in `Date` objects dropped). The default formats a global object keeps are bypassed while  an override is set. The scope is one VM, so one connection: a per-context zone would mean moving this state from the  VM to the global object, through every `vm.dateCache` user including the DFG.`0004` adds `JSGlobalContextInstallNamedProperties(ctx, filter, getter)`, for named access on the Window object.  The global's prototype is immutable to JavaScript and to `JSObjectSetPrototype`, so a page realm gets the object  when its context is created (`resetPrototype` replaces the global proxy, so nothing may hold it yet). A `JSClass`  object was measured first and made misses and new globals much slower (OpaqueJSString per lookup, dropped locks, a  put hook that sends every new global down the receiver path), so it is a native object like WebCore's  `JSDOMWindowProperties`: the global's own properties never reach it and stay inline cached; a name the global lacks  costs one call to `filter`, which `Dom::window_name_may_exist` answers from a hash set kept current from the change  journal, so a miss does not walk the tree after each mutation.
- Patch `0008` marks code from `lightpanda://internal/` (`helpers.js`) as never optimized. It runs at realm setup, and
  each realm otherwise compiled `makeInterface`, `lazyInterface` and `accessor` in the DFG again.
- Patch `0023` interns `JSObjectSetFastGetter`'s shared DOMJIT getters (patch `0013`) in a hash map keyed by
  (getter, fast callback, context, result type, readsOnly). It was a linear search of every fast getter the process
  had made, so defining a class's attributes on a realm's first use was quadratic: `CSSStyleDeclaration`'s ~1,600
  attributes cost each realm ~1.5 ms, and were installed as plain custom accessors to dodge it. They are JIT getters
  again.
- Patch `0024` adds `JSClassSetOwnPropertyNames` (`ClassDef::own_property_name`): names a class's wrappers have as
  own properties while the prototype's accessors serve reads. Chrome has computed-style properties as named
  properties of the object, so `hasOwnProperty('color')`, `Object.keys`, spread, `JSON.stringify` and
  `getOwnPropertyDescriptor` see them there. JSC lists the names in `getOwnPropertyNames` (as a realm's identifiers,
  made once on its first listing) and answers an own-property lookup (`InternalMethodType::GetOwnProperty`) with a
  custom value: `hasOwnProperty`, `Object.hasOwn` and `propertyIsEnumerable` (patched to read the slot's attributes,
  not a descriptor) answer without computing it, so they never restyle or lay out; a descriptor or a copy reads it
  through the class's `getValue` callback (`ClassDef::own_property_value`: the property's value whatever the
  prototype chain holds now, as in Chrome). The answer is not cached in `HasOwnPropertyCache`, because wrappers of
  different classes share a structure when they share a prototype. A `[[Get]]` never consults the names (a cacheable
  wrapper skips `getOwnPropertySlot` altogether), so reads stay inline-cached. The same patch makes the indexed
  callbacks' properties enumerable, as WebIDL's are.
- Patch `0025`: `JSObjectMakeWrapper`, `JSObjectGet/SetProperty` and `JSObjectGet/SetPrivateProperty` skip
  `JSLockHolder` when the thread already holds the VM's lock (every call from inside a callback). An API string that
  remembered its atom per VM (to skip the atom-table probe for constant names) was tried and dropped: its gain was
  within noise, and an atom reference outlives its table, which leaked ~270 KB per CDP connection.
- Patch `0026` closes two gaps of `0002` around the one JSC thread a fork does not stop, the VMTraps queue's, which
  fires watchdog timers. It kept running through the fork, so a lock it held at that instant stayed locked in the
  child: a watchdog's (the child's timers never notify its VM), a VM's trap lock (the child hangs handling its first
  trap), a ParkingLot bucket (a thread that parks there later hangs) or an allocator lock (the child aborts on it).
  `JSPrepareForFork` now holds that thread between two of its functions, where it has no lock, until `JSDidFork`.
  The forking thread waits for it in 10 µs sleeps, which adds about 30 µs to a fork (55 µs on a loaded machine): no
  `Thread::yield()`, which on Darwin depresses the thread for a millisecond, and no spinning, which measured worse
  under load. And a watchdog skips arming a timer while one that fires early enough is pending: a child inherited
  that state for a timer on the parent's queue, so its first VM entry within the parent's remaining time ran without
  a watchdog (no termination, no CDP polling while script runs). A watchdog now remembers the queue its timer is on.
  `crates/pyro-jsc-sys/tests/fork.rs` covers both.

Navigating or closing a page no longer always collects: `Engine::release_closed` runs `settle` only once the process's
physical footprint is 32 MB or 25%, whichever is larger, above its lowest reading. That collection runs inside the
command that closed the page, with the closing code on the stack (scanned conservatively) and perhaps a compilation
still running, so it can miss the closed page's heap; it therefore also arms the idle collection. `serve` collects what
is left after 1 s without a command (`Engine::settle`: the heap is collected and the allocator's free pages are
returned) and after 5 s with `Engine::settle_idle`: every compiled and unlinked code is dropped first (the bytecode
cache included, so the next page re-parses its scripts; a running suite's gaps are shorter and keep theirs). A driver
command parked on its page (`Pyro.run` between steps) is the client waiting, not a quiet client: the time counts from its
reply, so a long run is not collected, nor its code dropped, in the middle of a test. Measured after closing a React
app: 97 MB footprint before, 39 MB after (upstream Lightpanda: 34 MB). `PYRO_RELEASE_GROWTH=<MB>[:<percent>]`
overrides the thresholds (`0` collects every time) and `PYRO_RELEASE_ON_CLOSE=0` turns it off, for benchmarks.

`JsString::new` keeps an all-ASCII string of any length as 8-bit characters in JSC (a script source was converted to
UTF-16 before, twice its size, and lexed as such).

`pyro_js::configure_process` defaults `JSC_heapGrowthMaxIncrease` to 1 (JSC's default is 3, used on machines with at least
16 GB), so the heap grows to about 2x its live size between collections rather than 4x. Peak footprint with a large SPA
loaded went from 242 MB to 202 MB for about 5% more CPU on that SPA; small pages see no CPU change. An allocation-heavy
page with a large live set is the worst case measured: 18% more CPU, 39% lower peak. 0.5 is not worth it: on that page
it costs 5.6x the CPU. A value set in the environment wins.

It also defaults `JSC_smallHeapSize` to 4 MB. The C API makes its VM with JSC's small-heap setting, a collection
after every megabyte allocated, and JSC lets the young generation reach 8 times that step while collections are
frequent. A script that allocates steadily (Playwright's selector engine builds arrays of every element it visits)
spent 20% of its time collecting; with 4 MB the engine's query takes 3.0 ms of browser CPU instead of 4.1 ms, which
is what puts a Playwright locator level with Chrome. The speed follows the cap (8 MB: 3.6 to 4.1 ms, 16 MB: 3.6,
32 MB: 3.0), so a smaller step with a larger multiplier buys nothing. The price is memory while script runs: a loaded
mid-size SPA held 8 to 23 MB more `ps` RSS across runs, and the same as before once its page was closed.
`JSC_smallHeapSize=1048576` in the environment restores the old trade.

That young generation is garbage a VM keeps once its script stops: JSC collects when script allocates, and the timers
that collect an idle VM need a run loop only the system framework has. A thread of `serve` that has been quiet for a
second with its pages open (no command or task, under 5 ms of CPU in the last half second: `pyro_runtime::IdleWatch`),
the page's and each worker's alike, therefore runs `Engine::settle_if_grown`: a full collection with the free pages
returned, when the process grew by 16 MB or 12% since that VM was last settled. This buys memory with CPU. Measured
with a 2.7 MB app whose service worker precaches 17 MB, page open and quiet: 148 MB footprint instead of 198 MB (the
page alone 106 instead of 118 MB), for 110 to 150 M instructions and 23 to 32 ms of CPU per collected page VM, 2 to 4%
of that app's load, at a moment nothing else ran; a run whose commands follow one another never pays it. A forked
connection's process does not do it (it exits with its connection; pages that paused 2.5 s there cost 1.4% more
instructions for 44 MB less while open). `PYRO_IDLE_COLLECT=0` turns it off, `=1` on in forks too. Capping
the young generation instead (`JSC_maxEdenSizeForRateLimitingMultiplier`) gives the same memory and costs the locator
its speed: 16% more instructions per locator call over 2000 rows at 4 MB, 8% at 8 MB, 3% at 16 MB. Bun's process
options (`useConcurrentJIT=false`, `numberOfGCMarkers=1`, `useConcurrentGC=false`) were measured in both modes and
change instructions per page by less than the run-to-run spread (-1.6% to +2.1%), so none is set.

Patch `0009` keeps the JS lock held across C API callbacks, because a VM stays on one thread: stock JSC drops and
retakes it around every callback. `JSC_dropLocksInAPICallbacks=true` restores the stock behavior.

Patch `0010` gives wrappers of classes without property hooks their own root structure, without
`ProhibitsPropertyCaching`, `OverridesGetOwnPropertySlot` and `OverridesPut`, so inline caches remember their lookups.
Classes with named getters, callable objects and the global stay on the stock structure. These wrappers also have no own
`Symbol.toStringTag`: it is found on the prototype, as in browsers. `JSC_cacheCallbackObjectLookups=false` restores the stock
structure for new wrappers.

Patch `0012` gives host-object members the paths WebCore's bindings have, since the C API's `JSCallbackObject` can
neither be inline-cached for a property it answers nor linked by a call site. `JSObjectSetCustomAccessor` installs an
attribute as a JSC `CustomGetterSetter`: an inline cache calls the getter's C function directly with the receiver, no
call frame, and `Object.getOwnPropertyDescriptor` still reifies `{get, set}` functions. JSC passes a custom getter no
data, so `pyro-js` gives each (class, attribute) a trampoline of its own (`build.rs` generates 1,024 of them;
`src/accessors.rs` dispatches). `JSObjectMakeFunctionWithCallbackAndData` makes an operation an ordinary internal
function carrying its `MemberCall`, which call inline caches link to. `JSClassSetIndexedCallbacks` takes `list[i]` to an
indexed callback with the index as a number (stock JSC turns it into an atom string and runs the named hook), also for
`"3"` keys through `hasOwnProperty` and friends, so a class with only indexed hooks keeps cacheable wrappers and
`list.length` is an inline-cached prototype read. `JSObjectGetPrivate` checks the plain-wrapper class first.
`JSContextGroupReleaseFreeMemory` (the libpas scavenger, run now) and `JSContextGroupDeleteAllCode` back
`Engine::settle` and `Engine::settle_idle`. `JSValueGetStringCharacters` hands a string argument's characters out
without a copy (`Value::string_units`): the selector cache answers `matches`/`querySelector` by their hash, with no
`String`. `JSSetDecommitReplacesPages` makes libpas give freed pages back by mapping fresh ones over them: Darwin
keeps `madvise`d pages in the resident set until it needs them (the physical footprint drops, `ps` RSS does not),
so a process that closed a page kept reporting its memory; `configure_process` turns it on,
`PYRO_DECOMMIT=madvise` keeps the stock behaviour. Measured on Playwright's `locator.textContent` over a
2,000-row page (about 44,000 DOM calls per locator): 11.1 ms to 5.4 ms, browser CPU 12.2 ms to 6.6 ms;
`dom.mutate.1k` 2.3 to 1.9 ms.

Patch `0013` takes the two paths that still went through generic code into the JITs. `JSObjectSetFastGetter` installs
a pure getter as a JSC `DOMAttributeGetterSetter` whose `DOMJIT::GetterSetter` carries a context (the stock one is a
bare function, so a getter installed at runtime could not have a snippet): the baseline, DFG and FTL inline caches and
the DFG's `CallDOMGetter` prove the receiver is a C API wrapper and call `fast_getter_dispatch` with the receiver and
its private data, no property name, no decoding of `this`, with the declared result type and a reads-only effect (the
compiler drops a dead read and types what follows). `AccessCase::IndexedCallbackObjectLoad` makes `wrapper[i]` an
inline-cache case that calls the class's indexed getter (`operationGetByValCallbackObject`) where stock JSC gives up
on the cache and takes `operationGetByValGeneric`, the prototype walk and a `PropertySlot` per read. The collector
check in `JSLock::APICallbackScope` (patch `0009`) is a debug assertion now: it looked the current thread up twice on
every indexed read and method call. A getter that
returns a constant costs 7.5 ns per call through this path, loop included, which is what Chrome's bindings cost; what
a real getter costs above that is `pyro-js` and `pyro-dom` (see the measurements in `docs/OPTIMIZATION.md`).

Patch `0014` adds `JSWeakCreateKeptWhile`: a weak handle whose owner (a `WeakHandleOwner`) answers the collector's
"is this object still wanted" from a byte the host owns. It is what lets a DOM node own its wrapper (see "GC and
lifetime rules"): WebCore does the same through opaque roots, and the C API had only `JSValueProtect`, a counted
hash set that every collection visits in full.

Patch `0015` keeps the lookups on a wrapper with named properties cacheable. A class with a `getProperty` hook makes
every lookup on its wrappers uncacheable: a read of `document.nodeType` ran the hook and a prototype check before
finding the member (270 ns instead of 61). A class made with `ClassDef::named_properties_are_announced` (Document,
whose `named(false)` getter answers `document.formName`) gets the `kJSClassAttributeImpureNamedProperties` attribute;
its wrappers then have a structure marked impure (`GetOwnPropertySlotIsImpure | NewImpurePropertyFiresWatchpoints`)
instead of `ProhibitsPropertyCaching`, as WebCore's `HTMLDocument` does. An inline cache that remembers a lookup
watches its name, and `JSAddImpureProperty(name)` drops those caches. pyro-dom calls the hook from
`Dom::set_named_item_hook` (installed by `pyro_webapi::install`, through `pyro_js::ImpureNames`) for every name a
Document may start to answer: an embed, form, iframe, img or object entering a tree, or getting a `name` (or, for an
img, an `id`). What a hook answers is never cached. Stock JavaScriptCore ignores the attribute and keeps such
wrappers uncacheable.

Patch `0016` makes strings from host characters in one step. `JSValueMakeString` takes a `JSStringRef`, so every
string a getter returned was built twice: `JSStringCreateWithUTF8CString` (a NUL-terminated copy, `strlen`, UTF-8
decoding, a `StringImpl` and an `OpaqueJSString`), then `JSValueMakeString`, which takes the API lock and copies the
characters again (`OpaqueJSString::string()` is an isolated copy), then the release. `JSValueMakeStringLatin1` and
`JSValueMakeStringUTF16` build the value from a pointer and a length: one `StringImpl`, one copy, none for the empty
and single-character strings the VM keeps, and the lock only when the caller does not hold it (C API callbacks keep
it, patch `0009`). `Scope::string` passes an ASCII `&str` as it is and transcodes other text once, to Latin-1 when every
scalar fits (as JSC keeps parsed text) and to UTF-16 otherwise, on the stack when short. `JSStringCreateWithLatin1Characters`
does the same for `JsString::new` (property names, script sources), and `JSStringGetStoredCharacters` reads a
`JSStringRef` without `JSStringGetCharactersPtr`'s UTF-16 widening of an 8-bit string (JSON results, property names).
`JSValueGetStringCharacters` (patch `0012`) was only called inside callbacks; `0016` makes it safe anywhere, so
`Value::to_string` reads a string's own characters anywhere, CDP serialization included, instead of copying them into
a `JSStringRef` and widening them. A flat string is read with no lock and no exception check (the check would also run
the VM's traps, outside the VM too). Only a rope takes the lock, when the caller does not hold it, and there an
out-of-memory error from resolving it is cleared, since nothing above the call would see it; inside a callback it stays
pending and reaches the script. With the system framework (or an older build) all of this falls back to the
`JSStringRef` path. The functions are declared in `JSFork.cpp`, not `JSForkPrivate.h`, which every precompiled
header includes.

To change JSC: edit the checkout in `~/Library/Caches/pyro/webkit-src`, `ninja -C ../webkit-build
JavaScriptCore` to iterate, then regenerate the patch with `git diff` (use `git add -N` for new files) and rerun
`scripts/jsc/build.sh`, which resets the checkout and applies `patches/` from scratch.

## Fork mode (`serve --fork`)

Copy-on-write page snapshots: an app's startup (parsing and running its JavaScript) is paid once, not per test.

- The server forks a **template process** first thing (one thread, no engine yet; it goes on as
  `serve --template <socket>` would with the same options) and listens while it starts. It boots one page per
  prepared profile (`--prepare`, and every `Pyro.registerPreparedPage` from any connection) on its main
  thread, settles each once loaded, and listens on a Unix socket.
- Each upgraded connection is passed to the template with `SCM_RIGHTS`, plus the write end of a pipe. When the
  booted pages are loaded and quiet (or after 2 s of waiting), the template stops pyro-net's I/O threads
  (`pyro_net::client::suspend_for_fork`), calls `Engine::prepare_for_fork`, checks that no other thread is alive,
  and forks. The child restarts both and serves that one connection on its main thread with every booted page
  adopted (`Browser::set_fork_child`: no pool refills; registrations go to the template). It exits when the
  connection closes, which closes the pipe and releases the server's connection slot.
- The template holds the protocol state of a connection without a socket (`Browser<EngineHost>` with its own
  outbox, drained). A profile's `setup` steps run there once the page is loaded and quiet: the template
  acquires the page, runs them as an `Pyro.run`, hands the page back to the pool (`Browser::return_to_prepared`)
  and settles it. The forked child serves its socket with that same state (`serve::serve_browser`).
- **Profile status**: the root keeps each profile's `ProfileStatus` (`booting`, `ready`, `failed` with an error),
  which `registerPreparedPage`'s reply, `Pyro.preparedProfileStatus`, `adoptPreparedPage` and `preparedProfiles`
  report; clients wait on it (the SDK's `profile()` does). A setup step that fails (`setup step N (…): …`), a page
  that has not loaded after 30 s with its setup pending, or a derived template that cannot start or exits fails the
  profile: `Browser::fail_prepared_profile` closes its waiting page, and acquiring one fails with the error
  (code -32000, `data.setupError`) in every connection forked from then on. A derived template reports its own
  profile's outcome to the root (`Outcome`, no reply); the root fails the profiles derived from a failed one,
  drops their pending derives and serves their connections itself, so they never wait for a template that will
  not come. A failed profile stays registered (named, held) until retired as usual, so that adopters get its
  error (`adoptPreparedPage` refuses it without holding it); registering the same definition again boots a new
  attempt, and under a name the name and its holders move to it.
- A profile's `userAgent` and proxy (`proxyServer`, `proxyBypassList`) are its page's browser context's
  (`Host::create_context_with`: a `NetClient` whose configured `User-Agent` and proxy they are), so the
  document, subresources, fetch/XHR and setup steps of the template's page carry them, and so does every fork:
  the client and its settings are copied, its I/O thread restarted. A derived profile has its base's. What the
  engine loads for a page by itself goes out the same way (see [Engine loads](#engine-loads-of-a-page)).
- Snapshots build on snapshots: a profile with `base: <id>` is never booted. The root template asks the template
  holding the base (itself or a derived one) to fork off a **derived template** listening on
  `<root socket>.<id>`; it runs only the profile's `setup` on its copy of the base page, relabels the page, and
  serves forks itself. Clients ask for a snapshot with `ws://host/?profile=<id>`: the root forwards that
  connection's fds to the template holding it. Registrations always go to the root, which assigns every id.
  A derive whose base's template is not listening yet (it forks once its own base is ready) waits in the root
  and is sent when it is, for up to 90 s; so do connections for that profile.
- **Shared profiles**: the root keeps profiles by name for every client of the server (`registerPreparedPage
  {name}`, `adoptPreparedPage {name}`, `unregisterPreparedPage {name}`, `preparedProfiles`). A client's
  connection process opens a **lease** with its first named profile, a Unix stream to the root that it keeps
  until it exits: the root holds each name for the leases that registered or adopted it and drops a lease's
  holds when the stream reads EOF, however the process ended. A profile is retired once no lease holds any of
  its names and no active profile derives from it (then its base is checked). The same definition under a name
  finds the profile booted; a different one is refused while the name is held, and moves the name when it is
  not. Profiles registered without a name (and `--prepare`'s) stay until unregistered by id.
- **Spares** take the fork off a test's path: each template keeps `PYRO_FORK_SPARES` (default 2) forked
  children waiting, pages running. They accept connections themselves on the template's connection socket
  (`<socket>.conn`), so a test never waits for a template that is busy forking; a spare that took one closes its
  pair with the template and sends `Taken`, and the template forks a replacement. Registering a profile drops the
  spares (their pages are out of date). Measured on the 8 MB test app: 3.6 ms from connect to an evaluated
  expression in a booted page.
- The template stops when the server does: its stdin is a pipe only the server writes to.
- The server stops with the process it runs for (`--parent-pid`, which `Pyro.launch()` passes; a
  kqueue `NOTE_EXIT` or pidfd wait, `pyro_cdp::lifetime`). On that, and on SIGTERM, SIGINT or SIGHUP, it kills the
  template and removes every `pyro-<pid>.sock*` file; a server starting removes those of servers that
  are gone (killed with SIGKILL).
- Dedicated workers are threads, so a template holds them: they start in the child that takes a connection
  (`pyro_webapi::hold_for_fork`, `release_after_fork`). A thread entering a VM in a child cannot register with JSC's
  mach exception port, which a fork does not inherit, so a template runs with `JSC_useMachForExceptions=false`: faults
  go through POSIX signal handlers, which a fork does inherit.
- The source JavaScriptCore's timers run on WTF's run loop, so the CoreFoundation run-loop service
  (`enable_native_runloop_service`) is off with it; CoreFoundation must not be used in a fork child.
- **What a forked page costs** (`PYRO_FORK_STATS=<file>` logs each fork, each child's CPU, page faults
  and instructions at each step of its life, and each reaped child, as JSON lines). The template runs with its
  own JavaScriptCore defaults (`FORK_JSC_DEFAULTS`, over the server's own unless the user set them): a 32 MB JIT
  pool, since macOS's `fork()` walks the whole `MAP_JIT` reservation (15 ms per fork at JSC's 512 MB, 3.5 ms at
  32 MB; pages of a mid-size SPA use about 6 MB, and at 16 MB JSC already compiles less), and JSC's own heap
  growth (`heapGrowthMaxIncrease=3`), so that pages collect less often (a mid-size SPA's page still collects 5
  times): a collection in a fork marks memory it shares with the template, for a process that exits seconds later.
  A child whose connection ends runs its pages' unload handlers, closes the socket and `_exit`s: no realm
  disposal, collection or engine teardown (`PYRO_FORK_FAST_EXIT=0` tears down as a shared process
  does). Measured on the ToS suite (M1 Max,
  `-j 16`): a page's test costs about 3.4 G instructions, 0.55 s user and 0.08 s system time, mostly the test's
  own work, with about 18k page faults (5k of them copy-on-write).
- **Modules compiled once** (JavaScriptCore patch 0027, `Engine::precompile_module`). Before its first page the
  root template parses every JavaScript file of the static roots as a module and compiles its top-level code
  (`precompile_static_modules`; 1,348 files, 16 MB, in 0.3 s for the ToS build). The VM keeps each module's record
  (imports, exports) and unlinked code in a table keyed by the source text and its URL, which every fork inherits:
  a page that loads the same text takes both from the table, with no parse and no code generation, and compiles
  only the functions it calls. Another text at the URL, or the text from another origin, is parsed as before;
  `VM::deleteAllCode` (an idle process, coverage) drops the table. `PYRO_FORK_PRECOMPILE=off` turns it
  off; `functions` also compiles every function, which costs a page more memory touched than it saves in work.
  Measured on the ToS suite (2,160 tests, three passes each): 3,079 M instructions a page without, 2,816 M with
  (-8.5%), and 6% more page faults. The two thirds of the pages that load the app go from 4,218 M to 3,883 M;
  a page that only navigates inside its prepared app loads no module and gains nothing (580 M to 598 M).
- Measured on an M1 Max with a release build: an app whose startup runs 0.8 s of JavaScript is handed to a new
  connection, booted, in about 20 ms (the fork itself took 12-18 ms with the 512 MB JIT pool).

## Engine loads of a page

Besides the page's own requests, the engine loads what it lays out and paints: the images, `@font-face` fonts
and `@import`ed sheets of a document's layout (Blitz geometry, which also prints `Page.printToPDF`), and of the
render thread's copy of it (screenshots, `matchScreenshot`). Both go through the page's browser context, as its
own sub-resources do: its proxy and bypass list, `User-Agent`, cookies, extra headers, offline state, `Pyro.run`
mocks and Fetch interception apply, and CDP sees the requests on the document's frame.

- The render thread asks the browser thread (`Browser::fetch_subresource`) and caches per document (loader id).
- Layout: pyro-browser gives every document it creates its network (`pyro_dom::layout::SubresourceLoader`,
  `bindings::LayoutLoader`), from which `engine/blitz_geometry.rs` makes the document's fetcher. A miss starts
  `Browser::fetch_layout_subresource` in a task of its own and answers nothing now: a layout never waits for the
  network (a request held by interception needs the browser thread). A document without a network loads nothing
  from it; there is no fallback to another context's client.
- Layout bodies are kept by URL per network identity (`SubresourceBodies`): a context with a proxy or user agent
  of its own (`ContextNet`, prepared profiles, `Target.createBrowserContext {proxyServer, userAgent}`) has its own,
  gone with it, so one identity never lays out with what another loaded, and two identities through the same
  proxy share nothing either. The contexts that go out as the browser does share one set per process and
  browser-wide proxy and user agent, kept for the process's life (later contexts, connections and forks lay out
  with them at once, as before). A body that arrives lays out again only the documents of its identity. Only
  bodies are shared: a failed load (an interception abort, a 401 for this page's cookies) is remembered by its
  document alone, and the identity's other documents ask again.

## Platform support

| Platform | JSC | Status |
|---|---|---|
| macOS 27.0.1 (26A434) arm64, SDK 27.0, JavaScriptCore 22625.1.29.11.28 (dylib 625.1.29), rustc 1.99.1 | system framework | builds; all tests pass |
| macOS 27.0.1 arm64, Command Line Tools only (no Xcode), WebKit `0e0dd70` JSCOnly + `webkit/patches` | source, static archive | builds; all tests pass |
| other macOS versions | system framework | untested. Deployment target is 14.0; SPI is probed at runtime |
| Linux x64/arm64, Ubuntu 24.04 (glibc 2.39), clang 21 | source, static archive (JSCOnly + `webkit/patches`) | built in CI (`linux-build.yml`); no system fallback (WebKitGTK has no module loader). Release bundles ICU in `lib/` |

On some hosts the default SDK hits a linker/TAPI mismatch (`unknown arm64e.x1`). Worker 3 worked around it with a
command-scoped `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`. Do not hardcode that path.

## License and attribution

Upstream Lightpanda is AGPL-3.0-only (see its `LICENSE` and `LICENSING.md`). This rewrite is a derivative work.
It keeps AGPL-3.0-only (`LICENSE` copied verbatim) and credits upstream in `NOTICE`. Upstream's Rust html5ever glue
(`src/rust/html5ever`) is AGPL code: reuse it only with attribution.
