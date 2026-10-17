//! The per-thread JavaScriptCore engine (one `JSContextGroup` = one VM/heap).

use crate::class::{self, ClassRegistry};
use crate::error::{JsError, JsException};
use crate::realm::{Realm, RealmId, RealmInner, RealmOptions};
use crate::value::Value;
use lp_jsc_sys as sys;
use std::cell::{Cell, RefCell};
use std::os::raw::c_void;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Optional engine features, detected at runtime. Missing private SPI never
/// crashes: the dependent operation reports `JsError::Unsupported`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// `JSWeak*`: wrapper identity maps (required for DOM bindings).
    pub weak_refs: bool,
    /// `JSScript` module SPI: ES modules and dynamic `import()`.
    pub modules: bool,
    /// `JSGlobalContextSetUnhandledRejectionCallback`.
    pub unhandled_rejections: bool,
    /// `JSContextGroupSetExecutionTimeLimit`: termination of running scripts.
    pub termination: bool,
    /// `JSObject{Get,Set}PrivateProperty`: GC-traced hidden slots on wrappers.
    pub hidden_properties: bool,
    /// No-trap inspection of native proxy targets, including global proxies.
    pub proxy_targets: bool,
    /// Native global-object detection, including globals retained after realm disposal.
    pub object_global_context: bool,
}

impl Capabilities {
    pub fn detect() -> Capabilities {
        let spi = sys::spi::get();
        Capabilities {
            weak_refs: spi.weak().is_some(),
            // SAFETY: plain capability probe without arguments.
            modules: unsafe { sys::modules::lp_jsc_modules_available() } == 1,
            unhandled_rejections: spi.set_unhandled_rejection_callback.is_some(),
            termination: spi.set_execution_time_limit.is_some() && spi.clear_execution_time_limit.is_some(),
            hidden_properties: spi.get_private_property.is_some() && spi.set_private_property.is_some(),
            proxy_targets: spi.get_proxy_target.is_some(),
            object_global_context: spi.get_object_global_context.is_some(),
        }
    }
}

/// Console message severity (maps to CDP `Runtime.consoleAPICalled.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Log,
    Debug,
    Info,
    Warning,
    Error,
    Dir,
    Table,
    Trace,
    Assert,
    Clear,
    Count,
    TimeEnd,
    StartGroup,
    StartGroupCollapsed,
    EndGroup,
}

impl ConsoleLevel {
    /// The CDP `type` string.
    pub fn cdp_type(self) -> &'static str {
        match self {
            ConsoleLevel::Log => "log",
            ConsoleLevel::Debug => "debug",
            ConsoleLevel::Info => "info",
            ConsoleLevel::Warning => "warning",
            ConsoleLevel::Error => "error",
            ConsoleLevel::Dir => "dir",
            ConsoleLevel::Table => "table",
            ConsoleLevel::Trace => "trace",
            ConsoleLevel::Assert => "assert",
            ConsoleLevel::Clear => "clear",
            ConsoleLevel::Count => "count",
            ConsoleLevel::TimeEnd => "timeEnd",
            ConsoleLevel::StartGroup => "startGroup",
            ConsoleLevel::StartGroupCollapsed => "startGroupCollapsed",
            ConsoleLevel::EndGroup => "endGroup",
        }
    }
}

/// Observer for engine events; installed by the embedder (CLI or CDP session).
/// Called synchronously on the engine thread, possibly while JS is on the
/// stack: implementations must not re-enter the event loop.
pub trait EngineHooks {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, _args: &[Value<'_>]) {}
    fn exception_thrown(&self, _realm: &Realm, _exception: &JsException) {}
    fn unhandled_rejection(&self, _realm: &Realm, _exception: &JsException) {}
    fn realm_created(&self, _realm: &Realm) {}
    fn realm_destroyed(&self, _id: RealmId) {}
}

pub(crate) struct EngineInner {
    group: sys::JSContextGroupRef,
    /// Context kept alive for the engine's lifetime and used for engine-level
    /// operations (protect/unprotect) that must not depend on a page realm.
    root: sys::JSGlobalContextRef,
    pub(crate) caps: Capabilities,
    pub(crate) classes: RefCell<ClassRegistry>,
    pub(crate) realms: RefCell<Vec<(sys::JSGlobalContextRef, Weak<RealmInner>)>>,
    pub(crate) hooks: RefCell<Option<Rc<dyn EngineHooks>>>,
    pub(crate) next_realm_id: Cell<u32>,
    terminate: Arc<TerminateState>,
    /// Payload headers queued by GC finalizers, which may run on any thread.
    pub(crate) finalized: Arc<class::FinalizeQueue>,
    /// Set once the VM has been torn down; `Persistent`s then skip unprotect.
    shut_down: Cell<bool>,
    /// The owning `Engine` handle was dropped: callbacks no longer resolve to
    /// this engine and a new engine may be created on the thread.
    owner_dropped: Cell<bool>,
}

impl EngineInner {
    pub(crate) fn group(&self) -> sys::JSContextGroupRef {
        self.group
    }
    pub(crate) fn root_ctx(&self) -> sys::JSContextRef {
        self.root as sys::JSContextRef
    }
    pub(crate) fn hooks(&self) -> Option<Rc<dyn EngineHooks>> {
        self.hooks.borrow().clone()
    }
    pub(crate) fn is_shut_down(&self) -> bool {
        self.shut_down.get()
    }

    /// Finds the realm owning global context `global`.
    pub(crate) fn realm_for_global(&self, global: sys::JSGlobalContextRef) -> Option<Rc<RealmInner>> {
        self.realms.borrow().iter().find(|(g, _)| *g == global).and_then(|(_, r)| r.upgrade())
    }

    /// Disposes every realm, destroys the VM (running all remaining finalizers)
    /// and drops their payloads. Payload-owned `Persistent`s may keep this
    /// struct alive, but never the VM: after shutdown they are inert.
    ///
    /// Refused while any `Realm` handle is alive: every `Scope`, `Value`,
    /// `Object` and `Unwrapped` borrows one, so none can outlive the VM. A
    /// refused shutdown runs again when the last realm (which holds this
    /// struct) is dropped.
    fn shutdown(&self) {
        if self.shut_down.get() {
            return;
        }
        let realm_alive = self.realms.borrow().iter().any(|(_, r)| r.strong_count() > 0);
        if realm_alive || (self.is_current() && DEPTH.with(Cell::get) > 0) {
            return;
        }
        let realms: Vec<Rc<RealmInner>> = self.realms.borrow().iter().filter_map(|(_, r)| r.upgrade()).collect();
        for r in &realms {
            r.dispose();
        }
        drop(realms);
        class::drain_finalized(self);
        self.shut_down.set(true);
        // SAFETY: no realm, scope or callback remains; the root context and
        // group are owned here. Releasing the group destroys the VM, which
        // finalizes every remaining object (including protected ones).
        unsafe {
            if self.caps.termination {
                if let Some(clear) = sys::spi::get().clear_execution_time_limit {
                    clear(self.group);
                }
            }
            sys::JSGlobalContextRelease(self.root);
            sys::JSContextGroupRelease(self.group);
        }
        let was_current = self.is_current();
        class::drain_finalized(self);
        self.classes.borrow_mut().release_all();
        // Payload destructors above may have created a new engine on this
        // thread: only clear the thread state if it still refers to us.
        if was_current && self.is_current() {
            let _ = CURRENT.try_with(|c| *c.borrow_mut() = Weak::new());
            let _ = TERMINATING.try_with(|t| t.set(false));
        }
    }

    /// Whether this thread's current-engine slot refers to this engine.
    fn is_current(&self) -> bool {
        CURRENT.try_with(|c| std::ptr::eq(c.borrow().as_ptr(), self)).unwrap_or(false)
    }
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

thread_local! {
    static CURRENT: RefCell<Weak<EngineInner>> = const { RefCell::new(Weak::new()) };
    /// Native callback nesting depth on this thread (0 = called from Rust).
    pub(crate) static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// A termination is propagating through the VM on this thread.
    pub(crate) static TERMINATING: Cell<bool> = const { Cell::new(false) };
}

/// The live engine whose VM `ctx` belongs to, even if its owner was dropped
/// (for rooting values of a still-referenced realm).
pub(crate) fn engine_for_ctx(ctx: sys::JSContextRef) -> Option<Rc<EngineInner>> {
    let e = CURRENT.try_with(|c| c.borrow().upgrade()).ok().flatten()?;
    // SAFETY: ctx is a live context passed by the caller.
    let group = unsafe { sys::JSContextGetGroup(ctx) };
    (!e.is_shut_down() && e.group == group).then_some(e)
}

pub(crate) fn current_inner() -> Option<Rc<EngineInner>> {
    CURRENT.try_with(|c| c.borrow().upgrade()).ok().flatten().filter(|e| !e.is_shut_down() && !e.owner_dropped.get())
}

/// The JavaScript engine of the current thread.
///
/// At most one engine may be alive per thread: values, wrappers and callbacks
/// locate their engine through thread-local state. `Engine` is `!Send`/`!Sync`
/// and the unique owner: dropping it destroys the VM once no `Realm` handle
/// remains (immediately, if the embedder dropped its realms first).
pub struct Engine {
    pub(crate) inner: Rc<EngineInner>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // If realms are still referenced, teardown is deferred until the last
        // one goes away; meanwhile a new engine may be created on the thread.
        self.inner.owner_dropped.set(true);
        self.inner.shutdown();
    }
}

impl Engine {
    /// Creates the engine for this thread.
    pub fn new() -> Result<Engine, JsError> {
        if current_inner().is_some() {
            return Err(JsError::Type("a JavaScript engine already exists on this thread".into()));
        }
        // A previous engine whose owner was dropped but whose realms are still
        // referenced keeps running until they go; it is no longer "current".
        let caps = Capabilities::detect();
        if !caps.weak_refs {
            return Err(JsError::Unsupported("JSWeak (wrapper identity maps)"));
        }
        // SAFETY: plain object creation; ownership tracked by EngineInner.
        let (group, root) = unsafe {
            let group = sys::JSContextGroupCreate();
            let root = sys::JSGlobalContextCreateInGroup(group, std::ptr::null());
            (group, root)
        };
        let terminate = Arc::new(TerminateState { requested: AtomicBool::new(false) });
        let inner = Rc::new(EngineInner {
            group,
            root,
            caps,
            classes: RefCell::new(ClassRegistry::new()),
            realms: RefCell::new(Vec::new()),
            hooks: RefCell::new(None),
            next_realm_id: Cell::new(1),
            terminate: terminate.clone(),
            finalized: Arc::new(class::FinalizeQueue::default()),
            shut_down: Cell::new(false),
            owner_dropped: Cell::new(false),
        });
        if caps.termination {
            // Poll the termination flag every 50 ms of continuous JS execution.
            arm_watchdog(group, &terminate);
        }
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&inner));
        TERMINATING.with(|t| t.set(false));
        Ok(Engine { inner })
    }

    pub fn capabilities(&self) -> Capabilities {
        self.inner.caps
    }

    /// Installs the embedder's event observer (replaces any previous one).
    pub fn set_hooks(&self, hooks: Option<Rc<dyn EngineHooks>>) {
        *self.inner.hooks.borrow_mut() = hooks;
    }

    /// Registers host class `C` (and its ancestors). Must happen before the
    /// realms that should expose it are created.
    pub fn register<C: class::JsClass>(&self) -> class::ClassId {
        class::register::<C>(&self.inner)
    }

    /// Creates a realm (global object + bound host classes).
    pub fn create_realm(&self, options: RealmOptions) -> Result<Realm, JsError> {
        crate::realm::create(&self.inner, options)
    }

    /// Thread-safe handle to request termination of running JavaScript.
    pub fn terminate_handle(&self) -> Result<TerminateHandle, JsError> {
        if !self.inner.caps.termination {
            return Err(JsError::Unsupported("JSContextGroupSetExecutionTimeLimit"));
        }
        Ok(TerminateHandle(self.inner.terminate.clone()))
    }

    /// Clears a pending termination request (call before running new work).
    pub fn clear_termination(&self) {
        self.inner.terminate.requested.store(false, Ordering::SeqCst);
    }

    /// Releases payloads of wrappers finalized by the GC. Call at safe points
    /// (the event loop does this after each task).
    pub fn drain_finalized(&self) -> usize {
        class::drain_finalized(&self.inner)
    }

    /// Requests a full garbage collection (asynchronous in JSC).
    pub fn collect_garbage(&self) {
        unsafe { sys::JSGarbageCollect(self.inner.root_ctx()) }
    }

    /// Synchronous full collection when the SPI is available (tests/benchmarks).
    pub fn collect_garbage_now(&self) -> bool {
        match sys::spi::get().synchronous_gc_for_debugging {
            Some(f) => {
                unsafe { f(self.inner.root_ctx()) };
                true
            }
            None => {
                self.collect_garbage();
                false
            }
        }
    }

    /// Informs the GC about native memory owned by JS-reachable objects.
    pub fn report_extra_memory(&self, bytes: usize) {
        if let Some(f) = sys::spi::get().report_extra_memory_cost {
            unsafe { f(self.inner.root_ctx(), bytes) }
        }
    }

    /// Live realms of this engine.
    pub fn realms(&self) -> Vec<Realm> {
        self.inner.realms.borrow().iter().filter_map(|(_, r)| r.upgrade()).map(|inner| Realm { inner }).collect()
    }
}

/// How often running JavaScript polls the termination flag.
const WATCHDOG_INTERVAL_SECS: f64 = 0.05;

pub(crate) struct TerminateState {
    requested: AtomicBool,
}

/// `Send + Sync` handle that makes the engine abort running JavaScript at its
/// next watchdog poll (≤ ~50 ms). The aborted call returns
/// `JsError::Terminated`.
///
/// The C API cannot propagate JSC's uncatchable termination through native
/// frames: when a native callback sees termination it rethrows and re-arms the
/// watchdog, so page code that catches it is terminated again within one
/// polling interval.
#[derive(Clone)]
pub struct TerminateHandle(Arc<TerminateState>);

impl TerminateHandle {
    pub fn terminate(&self) {
        self.0.requested.store(true, Ordering::SeqCst);
    }
}

fn arm_watchdog(group: sys::JSContextGroupRef, state: &Arc<TerminateState>) {
    if let Some(set) = sys::spi::get().set_execution_time_limit {
        // SAFETY: the Arc pointer stays valid: EngineInner owns a clone and
        // clears the limit before releasing the group.
        unsafe { set(group, WATCHDOG_INTERVAL_SECS, Some(should_terminate), Arc::as_ptr(state) as *mut c_void) };
    }
}

/// Called when native code observed a termination while JS is still on the
/// stack: request it again and restart the watchdog timer for this entry.
pub(crate) fn rearm_termination(ctx: sys::JSContextRef) {
    if let Some(engine) = current_inner() {
        engine.terminate.requested.store(true, Ordering::SeqCst);
        let _ = ctx;
        arm_watchdog(engine.group, &engine.terminate);
    }
}

/// Resets termination state once control is back in Rust (depth 0): clears
/// the request and flushes JSC's pending termination, which otherwise aborts
/// the next entry when page code caught the termination exception.
pub(crate) fn finish_termination(ctx: sys::JSContextRef) {
    TERMINATING.with(|t| t.set(false));
    if let Some(engine) = current_inner() {
        engine.terminate.requested.store(false, Ordering::SeqCst);
    }
    let src = crate::string::JsString::new("0");
    let mut exc = std::ptr::null();
    // SAFETY: live context of this thread's engine.
    unsafe { sys::JSEvaluateScript(ctx, src.as_raw(), std::ptr::null_mut(), std::ptr::null(), 1, &mut exc) };
}

unsafe extern "C" fn should_terminate(ctx: sys::JSContextRef, data: *mut c_void) -> bool {
    // SAFETY: data is the TerminateState kept alive by EngineInner. This runs
    // on the thread executing JavaScript (the engine thread).
    let state = unsafe { &*(data as *const TerminateState) };
    if state.requested.swap(false, Ordering::SeqCst) {
        let _ = TERMINATING.try_with(|t| t.set(true));
        return true;
    }
    // JSC does not re-arm the limit after the callback declines; re-arming
    // from inside the callback is the pattern WebKit's own API tests use.
    if let Some(set) = sys::spi::get().set_execution_time_limit {
        unsafe { set(sys::JSContextGetGroup(ctx), WATCHDOG_INTERVAL_SECS, Some(should_terminate), data) };
    }
    false
}
