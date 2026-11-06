//! The per-thread JavaScriptCore engine (one `JSContextGroup` = one VM/heap).

use crate::class::{self, ClassRegistry};
use crate::error::{JsError, JsException};
use crate::realm::{Realm, RealmId, RealmInner, RealmOptions, Scope};
use crate::value::{Object, Value};
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
    /// Import maps, bare specifiers and `import.meta.resolve` (patched JavaScriptCore).
    pub import_maps: bool,
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
    /// fork() support (patched JavaScriptCore): [`Engine::prepare_for_fork`].
    pub fork: bool,
    /// Constant attributes of a realm's global (patched JavaScriptCore):
    /// [`crate::Scope::define_global_constant`] and
    /// [`crate::Scope::define_lazy_global_constant`].
    pub global_constants: bool,
}

impl Capabilities {
    pub fn detect() -> Capabilities {
        let spi = sys::spi::get();
        Capabilities {
            weak_refs: spi.weak().is_some(),
            // SAFETY: plain capability probe without arguments.
            modules: unsafe { sys::modules::lp_jsc_modules_available() } == 1,
            // SAFETY: plain capability probe without arguments.
            import_maps: unsafe { sys::modules::lp_jsc_import_maps_available() } == 1,
            unhandled_rejections: spi.set_unhandled_rejection_callback.is_some(),
            termination: spi.set_execution_time_limit.is_some() && spi.clear_execution_time_limit.is_some(),
            hidden_properties: spi.get_private_property.is_some() && spi.set_private_property.is_some(),
            proxy_targets: spi.get_proxy_target.is_some(),
            object_global_context: spi.get_object_global_context.is_some(),
            fork: spi.fork().is_some(),
            global_constants: spi.set_constant_accessor.is_some()
                && spi.set_lazy_constant_accessor.is_some()
                && spi.set_constant_accessor_initializer.is_some()
                && spi.clear_constant_accessors.is_some()
                && spi.get_proxy_target.is_some(),
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
    /// Page JavaScript called a binding [`Scope::install_binding`] defined
    /// (CDP `Runtime.bindingCalled`).
    fn binding_called(&self, _realm: &Realm, _name: &str, _payload: &str) {}
}

pub(crate) struct EngineInner {
    group: sys::JSContextGroupRef,
    /// Context kept alive for the engine's lifetime and used for engine-level
    /// operations (protect/unprotect) that must not depend on a page realm.
    root: sys::JSGlobalContextRef,
    pub(crate) caps: Capabilities,
    pub(crate) classes: RefCell<ClassRegistry>,
    /// The live realms by their global context (the address of the context,
    /// which a callback receives): a callback that cannot tell its realm from
    /// its function or receiver finds it here with one hash lookup. A realm
    /// enters at creation and leaves when it is disposed.
    pub(crate) realms: RefCell<rustc_hash::FxHashMap<usize, Weak<RealmInner>>>,
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
    /// When [`Engine::release_closed`] actually collects.
    release_gate: ReleaseGate,
    /// Payloads of the member functions made with the patched JSC's
    /// `JSObjectMakeFunctionWithCallbackAndData`, which have no finalizer:
    /// they live as long as the engine, which outlives every realm.
    pub(crate) member_payloads: RefCell<Vec<Box<class::MemberPayload>>>,
    /// Garbage collections seen, for sweeping the realms' interned strings.
    pub(crate) collections: crate::interned::Collections,
}

/// Decides when a closed page's garbage is worth a full collection: once the
/// process's footprint is past `RELEASE_GROWTH_MIN` or `RELEASE_GROWTH_PERCENT`
/// above its lowest reading. The footprint covers the engine's heap and the
/// native memory of closed pages alike, and costs one system call.
///
/// The reference is never raised, not even by a collection: JavaScriptCore
/// returns freed pages some time later, so a reading right after one is close
/// to the peak, and measuring from it let an app's process grow from 272 MB to
/// 726 MB over 25 navigations. A process that holds more than the threshold
/// alive collects on every close, which is what it would do without the gate.
///
/// Serving connections on several engines in one process, each sees the
/// others' growth too: that collects more often, never less.
struct ReleaseGate {
    low: Cell<u64>,
    /// A closed page has not been collected yet.
    pending: Cell<bool>,
    /// (minimum growth in bytes, percent of the footprint).
    growth: (u64, u64),
}

/// A loop of small pages levels off about 20 MB above its first reading under
/// JavaScriptCore's own collections. A lower threshold sits under that level
/// and collects on nearly every close: at 16 MB `page.navigate.simple` took
/// 4.5 ms of CPU per navigation, at 32 MB 2.6 ms, and collecting always 4.9 ms.
const RELEASE_GROWTH_MIN: u64 = 32 << 20;
const RELEASE_GROWTH_PERCENT: u64 = 25;

impl ReleaseGate {
    fn new(growth: (u64, u64)) -> ReleaseGate {
        ReleaseGate { low: Cell::new(u64::MAX), pending: Cell::new(false), growth }
    }

    /// `LIGHTNINGPANDA_RELEASE_GROWTH=<MB>[:<percent>]` overrides the
    /// thresholds (benchmark comparison); `0` collects on every close.
    fn growth_from_env() -> (u64, u64) {
        let default = (RELEASE_GROWTH_MIN, RELEASE_GROWTH_PERCENT);
        let Ok(value) = std::env::var("LIGHTNINGPANDA_RELEASE_GROWTH") else { return default };
        let (mb, percent) = value.split_once(':').unwrap_or((&value, "0"));
        match (mb.parse::<u64>(), percent.parse::<u64>()) {
            (Ok(mb), Ok(percent)) => (mb << 20, percent),
            _ => default,
        }
    }

    /// Whether a page that was just closed should be collected now.
    fn should_collect(&self, footprint: Option<u64>) -> bool {
        let Some(now) = footprint else { return true };
        let low = self.observe(now);
        let (min, percent) = self.growth;
        let due = now - low >= min.max(low / 100 * percent);
        self.pending.set(!due);
        due
    }

    /// Records a reading without deciding anything; the lowest so far.
    fn observe(&self, now: u64) -> u64 {
        let low = self.low.get().min(now);
        self.low.set(low);
        low
    }

    /// The process's pages are another's now: the next reading is the first.
    fn forked(&self) {
        self.low.set(u64::MAX);
        self.pending.set(false);
    }
}

/// The process's physical footprint in bytes: what the system charges it for,
/// without the reclaimable pages that resident size still counts.
fn process_footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        /// `struct rusage_info_v0` from `<sys/resource.h>`: a 16-byte UUID,
        /// then `ri_user_time` .. `ri_proc_exit_abstime`.
        #[repr(C)]
        struct RusageInfoV0 {
            uuid: [u8; 16],
            fields: [u64; 10],
        }
        const RI_PHYS_FOOTPRINT: usize = 7;
        const RUSAGE_INFO_V0: i32 = 0;
        unsafe extern "C" {
            fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut RusageInfoV0) -> i32;
        }
        let mut info = std::mem::MaybeUninit::<RusageInfoV0>::uninit();
        // SAFETY: the buffer has the layout the flavor writes, for our own pid.
        let done = unsafe { proc_pid_rusage(std::process::id() as i32, RUSAGE_INFO_V0, info.as_mut_ptr()) };
        // SAFETY: filled in when the call succeeds.
        (done == 0).then(|| unsafe { info.assume_init() }.fields[RI_PHYS_FOOTPRINT])
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
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
    /// Whether callbacks may still use this engine (what `current_inner`
    /// checks before handing it out).
    pub(crate) fn is_live(&self) -> bool {
        !self.is_shut_down() && !self.owner_dropped.get()
    }

    pub(crate) fn is_shut_down(&self) -> bool {
        self.shut_down.get()
    }

    /// Finds the realm owning global context `global`.
    pub(crate) fn realm_for_global(&self, global: sys::JSGlobalContextRef) -> Option<Rc<RealmInner>> {
        self.realms.borrow().get(&(global as usize)).and_then(Weak::upgrade)
    }

    /// Finds a live realm by id (`Realm::by_id` only: this walks the map).
    pub(crate) fn realm_by_id(&self, id: crate::RealmId) -> Option<Rc<RealmInner>> {
        self.realms.borrow().values().filter_map(Weak::upgrade).find(|r| r.id == id)
    }

    /// The live realms, in creation order.
    pub(crate) fn live_realms(&self) -> Vec<Rc<RealmInner>> {
        let mut realms: Vec<Rc<RealmInner>> = self.realms.borrow().values().filter_map(Weak::upgrade).collect();
        realms.sort_by_key(|r| r.id);
        realms
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
        let realm_alive = self.realms.borrow().values().any(|r| r.strong_count() > 0);
        if realm_alive || (self.is_current() && callback_tls().depth.get() > 0) {
            return;
        }
        let realms = self.live_realms();
        for r in &realms {
            r.dispose();
        }
        drop(realms);
        class::drain_finalized(self);
        self.collections.release(self.group);
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
            callback_tls().terminating.set(false);
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

/// Per-thread callback state, one thread-local (each access is a runtime
/// lookup on macOS, and a native callback reads these several times).
pub(crate) struct CallbackTls {
    /// Native callback nesting depth on this thread (0 = called from Rust).
    pub depth: Cell<u32>,
    /// A termination is propagating through the VM on this thread.
    pub terminating: Cell<bool>,
}

thread_local! {
    static CURRENT: RefCell<Weak<EngineInner>> = const { RefCell::new(Weak::new()) };
    static CALLBACK: CallbackTls = const { CallbackTls { depth: Cell::new(0), terminating: Cell::new(false) } };
    /// Asked at each watchdog poll while JavaScript runs on this thread;
    /// returning true terminates it (see [`Engine::set_interrupt_check`]).
    static INTERRUPT_CHECK: RefCell<Option<Rc<dyn Fn() -> bool>>> = const { RefCell::new(None) };
}

/// The live engine whose VM `ctx` belongs to, even if its owner was dropped
/// (for rooting values of a still-referenced realm).
pub(crate) fn engine_for_ctx(ctx: sys::JSContextRef) -> Option<Rc<EngineInner>> {
    let e = CURRENT.try_with(|c| c.borrow().upgrade()).ok().flatten()?;
    // SAFETY: ctx is a live context passed by the caller.
    let group = unsafe { sys::JSContextGetGroup(ctx) };
    (!e.is_shut_down() && e.group == group).then_some(e)
}

/// The thread's callback state, as a reference that outlives the call. The
/// cells have no destructor and live as long as the thread, which the caller
/// is on.
pub(crate) fn callback_tls() -> &'static CallbackTls {
    // SAFETY: a const-initialized thread-local without a destructor is never
    // deallocated while the thread runs; the reference is used only on it.
    CALLBACK.with(|c| unsafe { &*(c as *const CallbackTls) })
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
        configure_process();
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
            crate::value::verify_immediates(root);
            (group, root)
        };
        let terminate = Arc::new(TerminateState { requested: AtomicBool::new(false) });
        let inner = Rc::new(EngineInner {
            group,
            root,
            caps,
            classes: RefCell::new(ClassRegistry::new()),
            realms: RefCell::new(rustc_hash::FxHashMap::default()),
            hooks: RefCell::new(None),
            next_realm_id: Cell::new(1),
            terminate: terminate.clone(),
            finalized: Arc::new(class::FinalizeQueue::default()),
            shut_down: Cell::new(false),
            owner_dropped: Cell::new(false),
            release_gate: ReleaseGate::new(ReleaseGate::growth_from_env()),
            member_payloads: RefCell::new(Vec::new()),
            collections: crate::interned::Collections::new(),
        });
        if caps.termination {
            // Poll the termination flag every 50 ms of continuous JS execution.
            arm_watchdog(group, &terminate);
        }
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&inner));
        callback_tls().terminating.set(false);
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

    /// Creates a realm in this thread's live engine, including from an
    /// embedder callback whose owner already borrows the `Engine` handle.
    /// No new engine or class registration occurs. The caller must release
    /// native state borrows before calling: creation can run engine hooks and
    /// allocate/collect JavaScript objects. An absent or dropped owner is an
    /// error, even if old realm handles still keep its heap alive.
    pub fn create_realm_in_current_thread(options: RealmOptions) -> Result<Realm, JsError> {
        let inner = current_inner().ok_or(JsError::ContextDestroyed)?;
        crate::realm::create(&inner, options)
    }

    /// Installs a check this thread runs at every watchdog poll (about every
    /// 50 ms) while JavaScript executes; when it returns true the running
    /// script is terminated as by [`TerminateHandle::terminate`]. Lets the
    /// owner of the thread notice external events (a client disconnecting)
    /// without a second thread. It must be cheap and must not call JSC.
    pub fn set_interrupt_check(&self, check: Option<Rc<dyn Fn() -> bool>>) {
        INTERRUPT_CHECK.with(|c| *c.borrow_mut() = check);
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
        let n = class::drain_finalized(&self.inner);
        crate::interned::sweep_realms(&self.inner, false);
        n
    }

    /// Frees what a closed page (or a navigated-away document) left behind,
    /// once there is enough of it ([`ReleaseGate`]): a collection of the
    /// engine's heap, then the allocator's retained free pages are returned
    /// to the OS (`release_memory`). A closed page's wrappers and DOM are
    /// otherwise kept until the next collection, which an idle test process
    /// may never run: a caller with an idle moment checks
    /// [`Engine::release_pending`].
    ///
    /// Measured on 20 open/boot/close cycles of a 2 MB app: the process
    /// plateaus at 270 MB instead of 339 MB. `LIGHTNINGPANDA_RELEASE_ON_CLOSE=0`
    /// turns it off (benchmark comparison).
    pub fn release_closed(&self) {
        if std::env::var("LIGHTNINGPANDA_RELEASE_ON_CLOSE").is_ok_and(|v| v == "0") {
            return;
        }
        if self.inner.release_gate.should_collect(process_footprint()) {
            self.settle();
            // This collection runs inside the command that closed the page,
            // with the closing code on the stack: JavaScriptCore scans that
            // stack conservatively, and a compilation still running keeps its
            // realm too, so the closed page's heap can survive it (measured:
            // a React app's 30 MB stayed until the next close). A second
            // collection from the idle loop, where neither holds, frees it.
            self.inner.release_gate.pending.set(true);
        }
    }

    /// [`Engine::release_closed`] for a document whose replacement leaves
    /// nothing worth a full collection (a page's first document, its initial
    /// about:blank one): no collection, but the footprint is still a reading
    /// for the gate. Those moments are a process's low points, a test's
    /// first navigation for one, and the gate measures growth from the
    /// lowest reading: without them, the first reading would be a close
    /// with an app loaded, and later closes would rarely collect.
    pub fn note_release_point(&self) {
        if let Some(now) = process_footprint() {
            self.inner.release_gate.observe(now);
        }
    }

    /// Whether a closed page is still waiting for the collection
    /// [`Engine::release_closed`] put off. [`Engine::settle`] runs it.
    pub fn release_pending(&self) -> bool {
        self.inner.release_gate.pending.get()
    }

    /// A full synchronous collection, finalized wrappers released, free
    /// pages returned to the OS: a page that just finished booting, or a
    /// closed one, costs its live size instead of the heap's growth peak.
    pub fn settle(&self) {
        // Finished compilations only: a page close must not wait for an FTL
        // compile. One still running holds its realm until the next settle.
        self.complete_jit_plans(false);
        self.synchronous_gc();
        class::drain_finalized(&self.inner);
        crate::interned::sweep_realms(&self.inner, true);
        release_memory();
        // The collection freed the closed page's blocks to JSC's allocator,
        // whose scavenger returns them to the system only later, if at all
        // while the process stays busy: do it now (patched JSC).
        if let Some(f) = sys::spi::get().release_free_memory {
            unsafe { f(self.inner.group()) };
        }
        self.inner.release_gate.pending.set(false);
    }

    /// [`Engine::settle`] for a process that has gone idle: first drops every
    /// compiled and unlinked code (the bytecode cache included; patched JSC),
    /// which a closed page's scripts otherwise keep for a reload that may
    /// never come. The next page re-parses what it loads. Measured after
    /// closing a React app: 69 MB footprint with `settle`, 39 MB with this.
    pub fn settle_idle(&self) {
        if let Some(f) = sys::spi::get().delete_all_code {
            unsafe { f(self.inner.group()) };
        }
        self.settle();
    }

    /// Requests a full garbage collection (asynchronous in JSC).
    pub fn collect_garbage(&self) {
        unsafe { sys::JSGarbageCollect(self.inner.root_ctx()) }
    }

    /// Synchronous full collection when the SPI is available (tests/benchmarks),
    /// after every JIT compilation has finished, so that what it finds depends
    /// on reachability alone.
    pub fn collect_garbage_now(&self) -> bool {
        self.complete_jit_plans(true);
        self.synchronous_gc()
    }

    /// Installs finished JIT compilations (also running ones when `wait`). The
    /// worklist holds a finished plan until the next tier-up check in the VM, and
    /// the GC marks its CodeBlock, which reaches that code's global object: a
    /// helper that tiered up shortly before its page closed keeps the closed realm
    /// alive through every collection in between. No-op on the system framework.
    fn complete_jit_plans(&self, wait: bool) {
        if let Some(f) = sys::spi::get().complete_jit_plans {
            // SAFETY: the group is alive and used by this thread only.
            unsafe { f(self.inner.group, wait) };
        }
    }

    fn synchronous_gc(&self) -> bool {
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

    /// Makes a `fork()` of this process safe for JavaScriptCore: finishes a
    /// collection under way (it starts none: [`Engine::settle`] first for a
    /// compact parent) and JIT work, and parks every JSC helper thread.
    /// Call on the engine's thread, with no other thread running JavaScript,
    /// right before `fork()`, then [`Engine::did_fork`] in both processes.
    /// The child keeps this engine with all of its state, shared
    /// copy-on-write with the parent.
    pub fn prepare_for_fork(&self) -> Result<(), JsError> {
        let Some((prepare, _, _)) = sys::spi::get().fork() else {
            return Err(JsError::Unsupported("fork (patched JavaScriptCore)"));
        };
        self.drain_finalized();
        // SAFETY: the group is alive and used by this thread only.
        if unsafe { prepare(self.inner.group, FORK_PREPARE_TIMEOUT_SECS) } {
            Ok(())
        } else {
            unsafe { (sys::spi::get().fork().expect("checked").1)(false) };
            Err(JsError::Type("JavaScriptCore helper threads did not stop".into()))
        }
    }

    /// Call right after `fork()` in the parent (`child: false`) and in the
    /// child (`child: true`), after a successful [`Engine::prepare_for_fork`].
    /// In the child every realm also gets a new `Math.random()` seed.
    pub fn did_fork(&self, child: bool) {
        let Some((_, did_fork, reseed)) = sys::spi::get().fork() else { return };
        // SAFETY: called on the engine's thread after prepare_for_fork; the
        // contexts are alive (the root always, realms while listed and live).
        unsafe {
            did_fork(child);
            if child {
                self.inner.release_gate.forked();
                reseed(self.inner.root);
                for (global, realm) in self.inner.realms.borrow().iter() {
                    if realm.strong_count() > 0 {
                        reseed(*global as sys::JSGlobalContextRef);
                    }
                }
            }
        }
    }

    /// Calls `hook` after script adds a property to a wrapper on this thread, by assignment or
    /// `defineProperty`. It must not run JavaScript. Does nothing on unpatched JavaScriptCore.
    pub fn set_wrapper_property_hook(&self, hook: WrapperPropertyHook) {
        WRAPPER_PROPERTY_HOOK.set(Some(hook));
        if let Some(set) = sys::spi::get().set_wrapper_property_added {
            // SAFETY: a process-wide function pointer, to a function that looks up this thread's hook.
            unsafe { set(Some(wrapper_property_added)) };
        }
    }

    /// Sets the time zone of `Date`, `Intl` and `Temporal` in this engine's VM,
    /// which covers all of its realms and no other engine. `None` returns to
    /// the host's zone. `Ok(false)`, changing nothing, for an ID `Intl` does not
    /// know. Needs the patched JavaScriptCore.
    pub fn set_time_zone(&self, id: Option<&str>) -> Result<bool, JsError> {
        let Some(set) = sys::spi::get().set_time_zone else {
            return if id.is_none() {
                Ok(true)
            } else {
                Err(JsError::Unsupported("time zone override (patched JavaScriptCore)"))
            };
        };
        let id = match id.map(std::ffi::CString::new) {
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => return Ok(false),
            None => None,
        };
        // SAFETY: the group is alive and used by this thread only; the string
        // is NUL-terminated and read during the call.
        Ok(unsafe { set(self.inner.group, id.as_ref().map_or(std::ptr::null(), |id| id.as_ptr())) })
    }

    /// Live realms of this engine.
    pub fn realms(&self) -> Vec<Realm> {
        self.inner.live_realms().into_iter().map(|inner| Realm { inner }).collect()
    }
}

/// How long [`Engine::prepare_for_fork`] waits for JSC's helper threads.
const FORK_PREPARE_TIMEOUT_SECS: f64 = 5.0;

/// How often running JavaScript polls the termination flag.
const WATCHDOG_INTERVAL_SECS: f64 = 0.05;

/// Sets process-wide JavaScriptCore options that must be in place before JSC
/// initializes (it reads `JSC_*` environment variables once, at the first VM
/// creation). `Engine::new` calls this; executables should call it at the
/// start of `main`, before spawning threads, since it may set an environment
/// variable. Has no effect if JSC was already initialized in this process.
///
/// `JSC_usePollingTraps=true` (unless the variable is already set): with the
/// JIT enabled (the `com.apple.security.cs.allow-jit` entitlement), JSC's
/// default signal-based traps never deliver the watchdog to JIT-compiled
/// loops, so termination and `JSContextGroupSetExecutionTimeLimit` would not
/// fire; polling traps check in generated code instead.
///
/// `JSC_heapGrowthMaxIncrease=1` (unless the variable is already set): on a
/// machine with at least 16 GB, JSC otherwise lets the heap reach about 4x its
/// live size before the next collection. 1 makes that about 2x, which lowers
/// the peak on pages with a large heap for a few percent more CPU on them. See
/// docs/ARCHITECTURE.md for the measured trade-off.
///
/// `JSC_smallHeapSize=4194304`: the C API makes its VM with JSC's small-heap
/// setting, a collection after every megabyte allocated. A script that
/// allocates steadily (Playwright's selector engine builds arrays of every
/// element it visits) spent 20% of its time collecting. 4 MB between
/// collections takes 19% off such a query (8 MB only 2% more), which is the
/// difference between trailing Chrome on a locator and matching it; it costs
/// memory while script runs, about 14 MB of `ps` RSS on a loaded mid-size SPA.
/// `JSC_smallHeapSize=1048576` in the environment restores the old trade.
pub fn configure_process() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (name, value) in
            [("JSC_usePollingTraps", "true"), ("JSC_heapGrowthMaxIncrease", "1"), ("JSC_smallHeapSize", "4194304")]
        {
            if std::env::var_os(name).is_none() {
                std::env::set_var(name, value);
            }
        }
        // Freed pages leave the resident set (patched JSC): macOS keeps
        // `madvise`d pages resident until it needs them, so a process that
        // closed a page kept reporting its memory as in use.
        // `LIGHTNINGPANDA_DECOMMIT=madvise` keeps the stock behaviour.
        if std::env::var("LIGHTNINGPANDA_DECOMMIT").map_or(true, |v| v != "madvise") {
            if let Some(f) = sys::spi::get().set_decommit_replaces_pages {
                unsafe { f(true) };
            }
        }
    });
}

/// Complete Foundation number initialization in the parent before its first
/// fork, preferably on the main thread before starting VM/network workers.
/// macOS DNS can use these classes lazily through Network/Security entitlement
/// checks; initializing them for the first time in a fork child aborts. Derived
/// templates inherit the completed initialization, including this once guard.
pub fn prepare_process_for_fork() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: the ObjC shim owns its autorelease pool, returns no pointers,
        // and only touches Foundation's thread-safe immutable number classes.
        let valid = unsafe { sys::foundation::lp_foundation_initialize_numbers() };
        assert_eq!(valid, 1, "Foundation number initialization failed before fork");
        // Mach ports do not survive a fork, so in a child JSC's exception port is
        // gone, and a thread entering a VM there (a dedicated worker) crashes
        // registering with it. Signal handlers are inherited. JSC reads this when
        // it initializes, at the first VM, which this function must precede.
        if std::env::var_os("JSC_useMachForExceptions").is_none() {
            std::env::set_var("JSC_useMachForExceptions", "false");
        }
    });
}

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

/// Suspends the execution time limit while the engine runs its own setup
/// code (no page script), restoring it on drop. With the limit armed, every
/// VM entry reads the thread's CPU time (a system call); realm creation
/// makes hundreds of entries. Only at depth 0: with page JavaScript on the
/// stack the watchdog keeps running. A termination requested meanwhile is
/// seen at the first poll after the limit is restored.
pub(crate) struct WatchdogPause<'a>(Option<&'a EngineInner>);

impl<'a> WatchdogPause<'a> {
    pub(crate) fn new(engine: &'a EngineInner) -> Self {
        let spi = sys::spi::get();
        if !engine.caps.termination || callback_tls().depth.get() > 0 || spi.set_execution_time_limit.is_none() {
            return WatchdogPause(None);
        }
        let Some(clear) = spi.clear_execution_time_limit else { return WatchdogPause(None) };
        // SAFETY: valid group owned by the engine.
        unsafe { clear(engine.group) };
        WatchdogPause(Some(engine))
    }
}

impl Drop for WatchdogPause<'_> {
    fn drop(&mut self) {
        if let Some(engine) = self.0 {
            arm_watchdog(engine.group, &engine.terminate);
        }
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
    callback_tls().terminating.set(false);
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
    let check = INTERRUPT_CHECK.try_with(|c| c.borrow().clone()).ok().flatten();
    if check.is_some_and(|check| check()) {
        state.requested.store(true, Ordering::SeqCst);
    }
    if state.requested.swap(false, Ordering::SeqCst) {
        callback_tls().terminating.set(true);
        return true;
    }
    // JSC does not re-arm the limit after the callback declines; re-arming
    // from inside the callback is the pattern WebKit's own API tests use.
    if let Some(set) = sys::spi::get().set_execution_time_limit {
        unsafe { set(sys::JSContextGetGroup(ctx), WATCHDOG_INTERVAL_SECS, Some(should_terminate), data) };
    }
    false
}

/// What returns the process allocator's free pages when it is not the
/// system's ([`set_allocator_release`]).
static ALLOCATOR_RELEASE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Registers how the executable's global allocator returns its free pages
/// to the operating system: [`release_memory`] calls it besides the system
/// allocator's own release. An executable that replaces the allocator sets
/// it at the start of `main`.
pub fn set_allocator_release(release: fn()) {
    let _ = ALLOCATOR_RELEASE.set(release);
}

/// Returns the allocator's free pages to the operating system (what
/// `malloc_trim(0)` does on glibc; `malloc_zone_pressure_relief` on macOS).
pub fn release_memory() {
    if let Some(release) = ALLOCATOR_RELEASE.get() {
        release();
    }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means every zone; goal 0 means as much as possible.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: glibc's malloc_trim has no preconditions.
        unsafe {
            malloc_trim(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    #[test]
    fn release_gate_collects_on_growth_only() {
        let gate = ReleaseGate::new((16 * MB, 25));
        assert!(!gate.should_collect(Some(40 * MB)), "the first reading is the reference");
        assert!(!gate.should_collect(Some(50 * MB)));
        assert!(gate.pending.get());
        assert!(gate.should_collect(Some(56 * MB)));
        assert!(!gate.pending.get());
    }

    #[test]
    fn release_gate_scales_with_the_footprint() {
        let gate = ReleaseGate::new((16 * MB, 25));
        assert!(!gate.should_collect(Some(400 * MB)));
        assert!(!gate.should_collect(Some(480 * MB)), "under 25%");
        assert!(gate.should_collect(Some(500 * MB)));
    }

    #[test]
    fn release_gate_measures_from_the_low_point() {
        let gate = ReleaseGate::new((16 * MB, 25));
        assert!(!gate.should_collect(Some(100 * MB)));
        assert!(!gate.should_collect(Some(60 * MB)));
        assert!(gate.should_collect(Some(76 * MB)));
    }

    #[test]
    fn release_gate_is_not_raised_by_a_collection_that_returned_nothing_yet() {
        let gate = ReleaseGate::new((16 * MB, 25));
        assert!(!gate.should_collect(Some(100 * MB)));
        assert!(gate.should_collect(Some(130 * MB)));
        // Still 130 MB: the freed pages have not gone back to the system.
        assert!(gate.should_collect(Some(130 * MB)));
    }

    #[test]
    fn release_gate_without_a_threshold_or_a_footprint_always_collects() {
        assert!(ReleaseGate::new((0, 0)).should_collect(Some(100 * MB)));
        assert!(ReleaseGate::new((16 * MB, 25)).should_collect(None));
    }

    #[test]
    fn release_gate_measures_from_readings_that_collected_nothing() {
        let gate = ReleaseGate::new((16 * MB, 25));
        gate.observe(40 * MB);
        assert!(!gate.pending.get(), "a reading alone leaves nothing pending");
        assert!(gate.should_collect(Some(60 * MB)), "20 MB above the observed low point");
    }

    #[test]
    fn release_gate_after_a_fork_starts_from_the_next_reading() {
        let gate = ReleaseGate::new((16 * MB, 25));
        assert!(!gate.should_collect(Some(400 * MB)));
        gate.forked();
        assert!(!gate.should_collect(Some(500 * MB)));
        assert!(!gate.should_collect(Some(5 * MB)));
        assert!(gate.should_collect(Some(21 * MB)));
    }

    #[test]
    fn process_footprint_reads_something_on_macos() {
        if cfg!(target_os = "macos") {
            assert!(process_footprint().is_some_and(|f| f > MB));
        }
    }
}

pub type WrapperPropertyHook = for<'a, 'b> fn(&'a Scope<'b>, Object<'b>);

thread_local! {
    static WRAPPER_PROPERTY_HOOK: Cell<Option<WrapperPropertyHook>> = const { Cell::new(None) };
}

unsafe extern "C" fn wrapper_property_added(ctx: sys::JSContextRef, object: sys::JSObjectRef) {
    let Some(hook) = WRAPPER_PROPERTY_HOOK.get() else { return };
    // SAFETY: called by JSC on the VM's thread, with live arguments and the lock held.
    unsafe {
        crate::class::guard_in(ctx, crate::class::wrapper_realm(object), std::ptr::null_mut(), (), |scope| {
            hook(scope, Object::from_raw(ctx, object));
            Ok(())
        })
    }
}

/// Announces names that wrappers of a class made with
/// `ClassDef::named_properties_are_announced` may now answer (a document gained
/// an element named `name`): JavaScriptCore drops the inline caches and
/// compiled code that assumed no such property. Cheap when nothing watches the
/// name; safe to call with the DOM borrowed (it runs no JavaScript).
#[derive(Clone)]
pub struct ImpureNames(std::rc::Weak<EngineInner>);

impl ImpureNames {
    pub fn announce(&self, name: &str) {
        let Some(engine) = self.0.upgrade() else { return };
        if engine.is_shut_down() {
            return;
        }
        if let Some(add) = sys::spi::get().add_impure_property {
            let name = crate::JsString::new(name);
            // SAFETY: the engine's root context is alive while the engine is.
            unsafe { add(engine.root_ctx(), name.as_raw()) };
        }
    }
}

impl Engine {
    pub fn impure_names(&self) -> ImpureNames {
        ImpureNames(Rc::downgrade(&self.inner))
    }
}

impl<'s> crate::Scope<'s> {
    pub fn impure_names(&self) -> ImpureNames {
        ImpureNames(Rc::downgrade(self.engine_inner()))
    }
}
