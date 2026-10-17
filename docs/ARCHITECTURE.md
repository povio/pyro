# lightningpanda architecture and module contract

lightningpanda is a Rust rewrite of [lightpanda-io/browser](https://github.com/lightpanda-io/browser)
(reference checkout pinned at `2ad38773fd674d3c9cea26a263b814ee46f6ba84`). It uses **JavaScriptCore**
instead of V8. Its goal is fast, memory-efficient browser E2E testing through CDP (Playwright `connectOverCDP`).

This document is the interface contract between the three parallel workers. It is not a statement of parity:
see [PARITY.md](PARITY.md) for what exists today.

## Crates and ownership

| Crate | Owner | Depends on | Responsibility |
|---|---|---|---|
| `lp-jsc-sys` | W1 runtime | system JSC | Raw C API FFI. Runtime-resolved private SPI (`spi::get()`). ObjC module-loader shim. |
| `lp-js` | W1 | lp-jsc-sys | Engine, realms/worlds, values, `Persistent`, host classes, errors, promises, modules, console, termination. |
| `lp-runtime` | W1 | — | Single-threaded event loop: tasks, timers, `LoopHandle` cross-thread posting. |
| `lp-browser` | W1 (W2 provides document loading) | lp-js, lp-runtime, later lp-dom/lp-net/lp-webapi | Browser → Page → Frame tree, navigation state machine, loader ids, worlds, init scripts, window timers, ordered `PageEvent`s. |
| `lightningpanda` (bin) | W1 | all | CLI: `version`, `eval`; `serve`/`fetch` once CDP/loading land. |
| `lp-dom` | W2 DOM/net | — (no JS) | Arena DOM, `NodeId`, html5ever parsing, selectors, events, serialization. Unit-testable without JS. |
| `lp-net` | W2 | — (no JS) | hyper/tokio HTTP client on an I/O thread, cookies, CORS, fetch model, request observers. |
| `lp-webapi` | W2 | lp-js, lp-dom, lp-net | JS bindings for DOM/network WebAPIs using `lp_js::JsClass`. |
| `lp-cdp` | W3 automation | lp-browser, lp-js, lp-webapi | CDP WebSocket/HTTP server and domains; Runtime remote objects implemented natively. |
| `tests/e2e`, `bench/` | W3 | binary | Real TOS E2E compatibility and benchmarks. |

W1 owns the root `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml` and integration branches. Workers add dependencies
in their own crate manifests. W1 reconciles the lockfile on integration.

**V8 is forbidden.** `scripts/check-no-v8.sh` checks the dependency graph and sources.

## Threading model

- **One browser thread per CDP connection** (as upstream does). It owns the `lp_js::Engine` (one JSC context group =
  one VM/heap), the DOM, and an `EventLoop<Browser>`. Every JS/DOM type is `!Send`. No locks guard JS or DOM state.
- **I/O thread(s)** run a tokio runtime that handles socket accept, WebSocket framing and HTTP for `lp-net`.
  - They post work to the browser thread with `LoopHandle::post(Box<dyn FnOnce(&mut Browser) + Send>)`.
  - The loop processes posted work in arrival order. `post` returns `Err(LoopClosed)` once the browser thread has exited.
  - Responses and events go back through one ordered channel per connection.
- **At most one `Engine` per thread** (enforced). Callbacks find their engine and realm through thread-local state.
- On connection close, a teardown task runs `Browser::close_all()` before the loop exits. That disposes the realms
  and cancels the timers.

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

- Callbacks must be non-capturing closures or fn items (checked). Each one becomes a monomorphized `extern "C"`
  trampoline, so a call allocates nothing and looks nothing up.
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
  - **Root-relative specifiers:** JSC resolves `"/x.js"` against `file:///`. The runtime fetches the document-origin
    URL, but the module is registered under `file:///x.js`, so that module's `import.meta.url` reads `file:///x.js`.
    Only `file:///…` (empty authority) is remapped; `file://host/…` stays unchanged. An explicit `import("file:///x")`
    from an http page is therefore also fetched from the page origin.
  - **Bare specifiers and import maps:** not supported.
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

## Platform support

| Platform | JSC | Status |
|---|---|---|
| macOS 27.0.1 (26A434) arm64, SDK 27.0, JavaScriptCore 22625.1.29.11.28 (dylib 625.1.29), rustc 1.99.1 | system framework | builds; all tests pass |
| other macOS versions | system framework | untested. Deployment target is 14.0; SPI is probed at runtime |
| Linux | WebKitGTK `libjavascriptcoregtk` (C API only) | **not supported yet** (`build.rs` panics). No module loader exists there |

On some hosts the default SDK hits a linker/TAPI mismatch (`unknown arm64e.x1`). Worker 3 worked around it with a
command-scoped `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`. Do not hardcode that path.

## License and attribution

Upstream Lightpanda is AGPL-3.0-only (see its `LICENSE` and `LICENSING.md`). This rewrite is a derivative work.
It keeps AGPL-3.0-only (`LICENSE` copied verbatim) and credits upstream in `NOTICE`. Upstream's Rust html5ever glue
(`src/rust/html5ever`) is AGPL code: reuse it only with attribution.
