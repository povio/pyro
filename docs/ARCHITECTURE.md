# lightningpanda architecture and module contract

lightningpanda is a Rust rewrite of [lightpanda-io/browser](https://github.com/lightpanda-io/browser)
(reference checkout pinned at `2ad38773fd674d3c9cea26a263b814ee46f6ba84`). It uses **JavaScriptCore**
instead of V8. Its goal is fast, memory-efficient browser E2E testing through CDP (Playwright `connectOverCDP`).

This document is the interface contract between the three parallel workers. It is not a statement of parity:
see [PARITY.md](PARITY.md) for what exists today.

## Crates and ownership

| Crate | Owner | Depends on | Responsibility |
|---|---|---|---|
| `lp-jsc-sys` | W1 runtime | patched source JSC (or system JSC) | Raw C API FFI. Runtime-resolved private SPI (`spi::get()`). C module-loader shim (source JSC) or ObjC shim (system JSC). |
| `lp-js` | W1 | lp-jsc-sys | Engine, realms/worlds, values, `Persistent`, host classes, errors, promises, modules, console, termination. |
| `lp-runtime` | W1 | — | Single-threaded event loop: tasks, timers, `LoopHandle` cross-thread posting. |
| `lp-browser` | W1 (W2 provides document loading) | lp-js, lp-runtime, later lp-dom/lp-net/lp-webapi | Browser → Page → Frame tree, navigation state machine, loader ids, worlds, init scripts, window timers, ordered `PageEvent`s. |
| `lightningpanda` (bin) | W1 | all | CLI: `version`, `eval`; `serve`/`fetch` once CDP/loading land. |
| `lp-dom` | W2 DOM/net | — (no JS) | Arena DOM, `NodeId`, html5ever parsing, selectors, events, serialization. Unit-testable without JS. |
| `lp-net` | W2 | — (no JS) | hyper/tokio HTTP client on an I/O thread, cookies, CORS, fetch model, request observers. |
| `lp-webapi` | W2 | lp-js, lp-dom, lp-net | JS bindings for DOM/network WebAPIs using `lp_js::JsClass`. |
| `lp-cdp-protocol` | W3 automation | lp-net | CDP wire protocol without the engine: WebSocket/HTTP server, messages, `Target` domain, `Network`/`Fetch`/`Runtime` translation helpers. Builds in parallel with lp-dom/lp-webapi/lp-browser. |
| `lp-cdp` | W3 automation | lp-cdp-protocol, lp-browser, lp-js, lp-webapi | CDP domains on the engine (`engine/`), the per-connection browser thread (`serve`, `conn`); re-exports lp-cdp-protocol's modules. Runtime remote objects implemented natively. |
| `lp-render` | W3 automation | — (no JS, no lp crate) | Screenshots: a page snapshot (HTML with form state, live sheets, viewport) rendered by Blitz (html5ever, Stylo, Taffy, Parley; vello_cpu, optional vello/wgpu) on a render thread per connection; display-list hash, pixel compare, PNG. Built only with the `render` feature. |
| `tests/e2e`, `bench/` | W3 | binary | Real TOS E2E compatibility and benchmarks. |

W1 owns the root `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml` and integration branches. Workers add dependencies
in their own crate manifests. W1 reconciles the lockfile on integration.

**V8 is forbidden.** `scripts/check-no-v8.sh` checks the dependency graph and sources.

## Threading model

- **One browser thread per CDP connection** (as upstream does). It owns the `lp_js::Engine` (one JSC context group =
  one VM/heap), the DOM, and an `EventLoop<Browser>`. Every JS/DOM type is `!Send`. No locks guard JS or DOM state.
- **The CDP I/O thread** (tokio, current-thread) accepts sockets, answers the HTTP discovery endpoints and performs
  the WebSocket upgrade. It then hands the socket to the connection's browser thread and only holds the connection
  slot until that thread has finished (`SessionFactory::start_owned`).
- **The browser thread serves its own connection** (`lp-cdp/src/conn.rs`): it reads, decodes, runs and answers each
  command, and writes replies and events, all on that thread. The non-blocking socket is registered with the
  browser loop (`lp_runtime::FdWatch`), so an idle thread sleeps in one `poll(2)` on its wake pipe and the socket.
  Between loop tasks the connection is serviced (read at most every 100 µs while page work keeps the loop busy, at
  once when the socket wakes it). While JavaScript runs, the engine watchdog polls the connection
  (`Engine::set_interrupt_check`, about every 50 ms): it keeps reading and decoding, answers pings, and stops the
  script when the client closes or leaves, the outbound budget is exceeded, or the session takes nothing for the
  stall timeout. Guarantees kept from the channel design: in-order handling with at most 64 decoded messages
  waiting; replies and events written in production order, with pongs and close frames at frame boundaries; the
  64 MiB outbound budget (frames are pulled from the ordered outbound channel one at a time, so it counts what the
  socket has not taken) closing with 1008; frame, write, stall and close timeouts.
- **Other I/O threads** (lp-net's tokio runtime for HTTP) post work to the browser thread with
  `LoopHandle::post(Box<dyn FnOnce(&mut Browser) + Send>)`. The loop processes posted work in arrival order and
  writes its wake pipe only while asleep. `post` returns `Err(LoopClosed)` once the browser thread has exited.
- **At most one `Engine` per thread** (enforced). Callbacks find their engine and realm through thread-local state.
- **A dedicated worker is a thread of its own**, with its own `Engine` and event loop; it shares no JS or DOM state
  with its page, and messages cross as serialized tasks (`docs/webapi/WORKER.md`).
- On connection close the browser thread runs `Browser::close_all()`, which disposes the realms and cancels the
  timers, then writes what is still queued and a close frame within the close timeout, and shuts the socket.

## lp-js contract (implemented, tested)

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
  invoker (`lp-js` `MemberCall`): one `extern "C"` entry for all members, one invoker per class and member kind. A
  call reads the pointer from the function's data and looks nothing else up. With the patched JSC an operation is an
  internal function made by `JSObjectMakeFunctionWithCallbackAndData` and an attribute a custom accessor with a
  trampoline per (class, attribute) (see "JavaScriptCore source build", patch `0012`); with the system framework they
  are callback objects, as before. One full trampoline per member was the earlier design; it made lp-webapi's codegen
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
   A node keeps its own wrapper (patch `0014`): `lp-webapi` `PageDom::node_wrappers` holds, by node index, the
   `lp_js::WrapperRef` of the wrapper in up to two script worlds (the page's and one isolated world; a third world's,
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
   navigated away from, in many small batches, each document counts its externally referenced nodes (`lp-dom`
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
9. **Heap-collected call arguments** (for example CDP `callFunctionOn` converting arguments one by one, where each
   conversion can GC) must use `RootedVec` with `Object::call_rooted` or `Scope::call_function_rooted`, never a
   `Vec<Value>`.

### Private SPI (runtime-resolved, capability-gated)

Every private entry point is resolved with `dlsym` (`lp_jsc_sys::spi::get()`); none is linked. A missing symbol
yields `JsError::Unsupported` or a feature that is reported as absent (`Engine::capabilities()`). It never causes a
load failure or a crash.

| Capability | Symbols | Use | If missing |
|---|---|---|---|
| `weak_refs` | `JSWeakCreate/Release/GetObject` | wrapper identity | `Engine::new` fails: identity is required |
| `modules` | ObjC `JSScript` (module type), `-[JSContext setModuleLoaderDelegate:]`, `evaluateJSScript:` | ES modules, `import()` | `eval_module` → `Unsupported` |
| `unhandled_rejections` | `JSGlobalContextSetUnhandledRejectionCallback` | `unhandledrejection` / CDP exceptions | no reports |
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
  - **Inline modules:** the registry is keyed by URL, so each inline module script needs a distinct URL. The browser
    appends a fragment.
  - **Linux:** WebKitGTK's JSC has no module loader, so **ESM on Linux is unsupported** until a tested alternative
    exists.

## lp-runtime contract (implemented, tested)

`EventLoop<S: LoopState>`:
- `scheduler()` → `Scheduler<S>` (`Rc`), with `post(source, FnOnce(&mut S))`, `set_timer(delay, repeat, keeps_alive, Fn(&mut S)) -> TimerId`, `clear_timer` and `stop`.
- `handle()` → `LoopHandle<S>` (Send), with `post` and `Err(LoopClosed)`.
- `run_one`, `run_until(state, deadline, stop_when_idle, done)`, `run_until_idle`.
- After every task it calls `LoopState::after_task`.

## lp-browser contract (partially implemented)

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

`lp-jsc-sys` links a patched JavaScriptCore built from WebKit source, so the engine can be changed (the goal is
copy-on-write page snapshots: load an app's JS once, then start each test from that state). Inputs live in
`crates/lp-jsc-sys/webkit/`: `REVISION` (WebKit commit) and `patches/*.patch`, kept small and few, like Bun's fork.

- `scripts/jsc/build.sh` builds WebKit's portable JSCOnly CMake port (the Apple framework build needs full Xcode)
  into `~/Library/Caches/lightningpanda/jsc/<revision>-<patch hash>/lib/libJavaScriptCore.dylib`, with a copy of
  the inputs. Needs `brew install cmake ninja`. A clean build took a few minutes at `-j10` on an M1 Max; after a
  patch change ninja rebuilds incrementally.
- The dylib has an absolute install name and is linked dynamically, so Rust edit rebuilds cost the same as with the
  system framework.
- `build.rs` links the cached build whose inputs match the checkout. Without one it warns and falls back to the
  system framework (patched-only entry points are then reported unsupported). `LP_JSC=system` forces the system
  framework, `LP_JSC_DIR=<dir>` uses `<dir>/lib/libJavaScriptCore.dylib`.
- JSCOnly has no Objective-C API, so patch `0001` adds a C module-loader SPI (`JSGlobalContextSetModuleLoader`,
  `JSModuleSourceCreate`, `JSLoadAndEvaluateModule`) to `JSAPIGlobalObject`, ported from `JSAPIGlobalObject.mm`.
  `shim/modules_source.c` maps the crate's `lp_jsc_module_*` interface onto it.
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

Navigating or closing a page no longer always collects: `Engine::release_closed` runs `settle` only once the process's
physical footprint is 32 MB or 25%, whichever is larger, above its lowest reading. That collection runs inside the
command that closed the page, with the closing code on the stack (scanned conservatively) and perhaps a compilation
still running, so it can miss the closed page's heap; it therefore also arms the idle collection. `serve` collects what
is left after 1 s without a command, with `Engine::settle_idle`: every compiled and unlinked code is dropped first
(the bytecode cache included, so the next page re-parses its scripts; a running suite's gaps are shorter than 1 s and
keep theirs), then the heap is collected and the allocator's free pages are returned. Measured after closing a React
app: 97 MB footprint before, 39 MB after (upstream Lightpanda: 34 MB). `LIGHTNINGPANDA_RELEASE_GROWTH=<MB>[:<percent>]`
overrides the thresholds (`0` collects every time) and `LIGHTNINGPANDA_RELEASE_ON_CLOSE=0` turns it off, for benchmarks.

`JsString::new` keeps an all-ASCII string of any length as 8-bit characters in JSC (a script source was converted to
UTF-16 before, twice its size, and lexed as such).

`lp_js::configure_process` defaults `JSC_heapGrowthMaxIncrease` to 1 (JSC's default is 3, used on machines with at least
16 GB), so the heap grows to about 2x its live size between collections rather than 4x. Peak footprint with a large SPA
loaded went from 242 MB to 202 MB for about 5% more CPU on that SPA; small pages see no CPU change. An allocation-heavy
page with a large live set is the worst case measured: 18% more CPU, 39% lower peak. 0.5 is not worth it: on that page
it costs 5.6x the CPU. A value set in the environment wins.

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
data, so `lp-js` gives each (class, attribute) a trampoline of its own (`build.rs` generates 1,024 of them;
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
`LIGHTNINGPANDA_DECOMMIT=madvise` keeps the stock behaviour. Measured on Playwright's `locator.textContent` over a
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
a real getter costs above that is `lp-js` and `lp-dom` (see the measurements in `docs/OPTIMIZATION.md`).

Patch `0014` adds `JSWeakCreateKeptWhile`: a weak handle whose owner (a `WeakHandleOwner`) answers the collector's
"is this object still wanted" from a byte the host owns. It is what lets a DOM node own its wrapper (see "GC and
lifetime rules"): WebCore does the same through opaque roots, and the C API had only `JSValueProtect`, a counted
hash set that every collection visits in full.

To change JSC: edit the checkout in `~/Library/Caches/lightningpanda/webkit-src`, `ninja -C ../webkit-build
JavaScriptCore` to iterate, then regenerate the patch with `git diff` (use `git add -N` for new files) and rerun
`scripts/jsc/build.sh`, which resets the checkout and applies `patches/` from scratch.

## Fork mode (`serve --fork`)

Copy-on-write page snapshots: an app's startup (parsing and running its JavaScript) is paid once, not per test.

- The server spawns a **template process** (`serve --template <socket>`, same options). It boots one page per
  prepared profile (`--prepare`, and every `Lightningpanda.registerPreparedPage` from any connection) on its main
  thread, settles each once loaded, and listens on a Unix socket.
- Each upgraded connection is passed to the template with `SCM_RIGHTS`, plus the write end of a pipe. When the
  booted pages are loaded and quiet (or after 2 s of waiting), the template stops lp-net's I/O threads
  (`lp_net::client::suspend_for_fork`), calls `Engine::prepare_for_fork`, checks that no other thread is alive,
  and forks. The child restarts both and serves that one connection on its main thread with every booted page
  adopted (`Browser::set_fork_child`: no pool refills; registrations go to the template). It exits when the
  connection closes, which closes the pipe and releases the server's connection slot.
- The template holds the protocol state of a connection without a socket (`Browser<EngineHost>` with its own
  outbox, drained). A profile's `setup` steps run there once the page is loaded and quiet: the template
  acquires the page, runs them as an `LP.run`, hands the page back to the pool (`Browser::return_to_prepared`)
  and settles it. The forked child serves its socket with that same state (`serve::serve_browser`).
- Snapshots build on snapshots: a profile with `base: <id>` is never booted. The root template asks the template
  holding the base (itself or a derived one) to fork off a **derived template** listening on
  `<root socket>.<id>`; it runs only the profile's `setup` on its copy of the base page, relabels the page, and
  serves forks itself. Clients ask for a snapshot with `ws://host/?profile=<id>`: the root forwards that
  connection's fds to the template holding it. Registrations always go to the root, which assigns every id.
- **Spares** take the fork off a test's path: each template keeps `LIGHTNINGPANDA_FORK_SPARES` (default 2) forked
  children waiting, pages running. They accept connections themselves on the template's connection socket
  (`<socket>.conn`), so a test never waits for a template that is busy forking; a spare that took one closes its
  pair with the template and sends `Taken`, and the template forks a replacement. Registering a profile drops the
  spares (their pages are out of date). Measured on the 8 MB test app: 3.6 ms from connect to an evaluated
  expression in a booted page.
- The template stops when the server does: its stdin is a pipe only the server writes to.
- Dedicated workers are threads, so a template holds them: they start in the child that takes a connection
  (`lp_webapi::hold_for_fork`, `release_after_fork`). A thread entering a VM in a child cannot register with JSC's
  mach exception port, which a fork does not inherit, so a template runs with `JSC_useMachForExceptions=false`: faults
  go through POSIX signal handlers, which a fork does inherit.
- The source JavaScriptCore's timers run on WTF's run loop, so the CoreFoundation run-loop service
  (`enable_native_runloop_service`) is off with it; CoreFoundation must not be used in a fork child.
- Measured on an M1 Max with a release build: an app whose startup runs 0.8 s of JavaScript is handed to a new
  connection, booted, in about 20 ms (the fork itself takes 12-18 ms).

## Platform support

| Platform | JSC | Status |
|---|---|---|
| macOS 27.0.1 (26A434) arm64, SDK 27.0, JavaScriptCore 22625.1.29.11.28 (dylib 625.1.29), rustc 1.99.1 | system framework | builds; all tests pass |
| macOS 27.0.1 arm64, Command Line Tools only (no Xcode), WebKit `0e0dd70` JSCOnly + `webkit/patches` | source dylib | builds; all tests pass |
| other macOS versions | system framework | untested. Deployment target is 14.0; SPI is probed at runtime |
| Linux | WebKitGTK `libjavascriptcoregtk` (C API only) | **not supported yet** (`build.rs` panics). No module loader exists there |

On some hosts the default SDK hits a linker/TAPI mismatch (`unknown arm64e.x1`). Worker 3 worked around it with a
command-scoped `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`. Do not hardcode that path.

## License and attribution

Upstream Lightpanda is AGPL-3.0-only (see its `LICENSE` and `LICENSING.md`). This rewrite is a derivative work.
It keeps AGPL-3.0-only (`LICENSE` copied verbatim) and credits upstream in `NOTICE`. Upstream's Rust html5ever glue
(`src/rust/html5ever`) is AGPL code: reuse it only with attribution.
