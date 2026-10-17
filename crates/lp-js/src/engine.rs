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

    /// Finds the realm owning global context `global`.
    pub(crate) fn realm_for_global(&self, global: sys::JSGlobalContextRef) -> Option<Rc<RealmInner>> {
        self.realms.borrow().iter().find(|(g, _)| *g == global).and_then(|(_, r)| r.upgrade())
    }
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        // SAFETY: last reference; no realm (they hold Rc<EngineInner>) or
        // Persistent remains. Releasing the group runs remaining finalizers,
        // whose payloads are then dropped by drain_finalized.
        unsafe {
            if self.caps.termination {
                if let Some(clear) = sys::spi::get().clear_execution_time_limit {
                    clear(self.group);
                }
            }
            sys::JSGlobalContextRelease(self.root);
            self.classes.get_mut().release_all();
            sys::JSContextGroupRelease(self.group);
        }
        class::drain_finalized(None);
    }
}

thread_local! {
    static CURRENT: RefCell<Weak<EngineInner>> = const { RefCell::new(Weak::new()) };
}

pub(crate) fn current_inner() -> Option<Rc<EngineInner>> {
    CURRENT.try_with(|c| c.borrow().upgrade()).ok().flatten()
}

/// The JavaScript engine of the current thread.
///
/// At most one engine may be alive per thread: values, wrappers and callbacks
/// locate their engine through thread-local state. `Engine` is `!Send`/`!Sync`.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Rc<EngineInner>,
}

impl Engine {
    /// Creates the engine for this thread.
    pub fn new() -> Result<Engine, JsError> {
        if current_inner().is_some() {
            return Err(JsError::Type("a JavaScript engine already exists on this thread".into()));
        }
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
        });
        if caps.termination {
            // Poll the termination flag every 50 ms of continuous JS execution.
            // Returning false from the callback re-arms the limit.
            let set = sys::spi::get().set_execution_time_limit.expect("checked by caps");
            // SAFETY: the Arc pointer stays valid: EngineInner owns a clone and
            // clears the limit before it is dropped.
            unsafe {
                set(group, WATCHDOG_INTERVAL_SECS, Some(should_terminate), Arc::as_ptr(&terminate) as *mut c_void)
            };
        }
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&inner));
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
        class::drain_finalized(Some(&self.inner))
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

struct TerminateState {
    requested: AtomicBool,
}

/// `Send + Sync` handle that makes the engine abort running JavaScript at its
/// next watchdog poll (≤ ~50 ms). The aborted call fails with an uncatchable
/// termination exception.
#[derive(Clone)]
pub struct TerminateHandle(Arc<TerminateState>);

impl TerminateHandle {
    pub fn terminate(&self) {
        self.0.requested.store(true, Ordering::SeqCst);
    }
}

unsafe extern "C" fn should_terminate(ctx: sys::JSContextRef, data: *mut c_void) -> bool {
    // SAFETY: data is the TerminateState kept alive by EngineInner.
    let state = unsafe { &*(data as *const TerminateState) };
    if state.requested.swap(false, Ordering::SeqCst) {
        return true;
    }
    // JSC does not re-arm the limit after the callback declines; re-arming
    // from inside the callback is the pattern WebKit's own API tests use.
    if let Some(set) = sys::spi::get().set_execution_time_limit {
        unsafe { set(sys::JSContextGetGroup(ctx), WATCHDOG_INTERVAL_SECS, Some(should_terminate), data) };
    }
    false
}
