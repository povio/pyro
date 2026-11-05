//! Realms (JavaScript global environments) and the `Scope` handle used to work
//! inside one.
//!
//! A realm is one `JSGlobalContext` in the engine's context group. A page
//! frame has a main-world realm plus optional isolated worlds (e.g. the
//! Playwright utility world); they share the DOM but have distinct globals and
//! wrapper objects (per-realm identity maps).

use crate::class::{self, ClassId, Header, JsClass, Kind};
use crate::engine::{ConsoleLevel, EngineInner};
use crate::error::{JsError, JsException, JsResult};
use crate::persistent::Persistent;
use crate::string::JsString;
use crate::value::{check, Args, Object, Value};
use lp_jsc_sys as sys;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::ptr;
use std::rc::{Rc, Weak};

/// Stable identifier of a realm within its engine (CDP `executionContextId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RealmId(pub u32);

/// Key identifying the native object behind a wrapper, so the same object is
/// always exposed as the same JS object within a realm (e.g. document pointer
/// and node id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdentityKey(pub u64, pub u64);

impl IdentityKey {
    /// Not a key: asks [`Scope::wrap`] for a wrapper its owner keeps track of
    /// itself ([`WrapperRef`]), outside the realm's identity map. The owner
    /// holds the reference where it keeps the thing wrapped (a DOM node's
    /// slot), so finding the wrapper again is a load, and decides how long
    /// the wrapper lives with [`WrapperRef::set_kept`] instead of a table of
    /// protected values. Needs the patched JavaScriptCore
    /// ([`Scope::owned_wrappers`]).
    pub const OWNED: IdentityKey = IdentityKey(u64::MAX, 0);
    /// [`IdentityKey::OWNED`], kept alive from the start.
    pub const OWNED_KEPT: IdentityKey = IdentityKey(u64::MAX, 1);

    fn is_owned(self) -> bool {
        self.0 == u64::MAX && self.1 <= 1
    }
}

/// A wrapper made with [`IdentityKey::OWNED`], as its owner holds it. Valid
/// until the wrapper's payload (its `Data`) is dropped: the owner forgets the
/// reference in that `Drop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapperRef(usize);

impl WrapperRef {
    pub fn to_raw(self) -> usize {
        self.0
    }

    /// # Safety
    /// `raw` came from [`WrapperRef::to_raw`] (zero is no reference).
    pub unsafe fn from_raw(raw: usize) -> Option<WrapperRef> {
        (raw != 0).then_some(WrapperRef(raw))
    }

    /// Keeps the wrapper alive (or lets the collector decide again). The
    /// collector reads this while marking; a wrapper whose owner stopped
    /// keeping it dies at a later collection if nothing else reaches it.
    ///
    /// # Safety
    /// The wrapper's payload has not been dropped.
    pub unsafe fn set_kept(self, keep: bool) {
        let h = self.0 as *mut Header;
        unsafe { (*h).keep_alive.store(u8::from(keep), std::sync::atomic::Ordering::Relaxed) };
    }

    /// # Safety
    /// The wrapper's payload has not been dropped.
    pub unsafe fn is_kept(self) -> bool {
        let h = self.0 as *mut Header;
        unsafe { (*h).keep_alive.load(std::sync::atomic::Ordering::Relaxed) != 0 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RealmOptions {
    /// Debug/world name (`""` for the main world).
    pub name: String,
    /// Host class whose instance is the global object (e.g. `Window`). The
    /// global's payload is set with [`Realm::set_global_data`].
    pub global_class: Option<ClassId>,
    /// Origin used to resolve root-relative module specifiers, which JSC maps
    /// to `file:///…` (e.g. `http://127.0.0.1:8080`).
    pub origin: Option<String>,
}

/// Receives ES module fetch requests from a realm (`import`, `import()`).
pub trait ModuleFetcher {
    /// Fetch `url` (absolute) and complete `request`, now or later on the loop.
    fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest);

    /// The document base URL, which specifiers resolve against when their
    /// script has no URL of its own (CDP evaluate, init scripts).
    fn document_base_url(&self, _realm: &Realm) -> Option<String> {
        None
    }
}

/// A pending module fetch; complete exactly once (drop = reject).
pub struct ModuleRequest {
    url: String,
    /// The identifier JSC requested (module registry key); differs from `url`
    /// for root-relative specifiers remapped from `file://`.
    key: String,
    realm: std::rc::Weak<RealmInner>,
    resolve: Persistent,
    reject: Persistent,
    done: bool,
}

// JavaScriptCore's module SPI creates an empty import.meta object. Initialize
// the canonical module key before evaluating source so user code sees its URL.
fn module_source_with_url(source: &str, url: &str) -> String {
    let url = serde_json::to_string(url)
        .expect("a URL string is serializable")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    format!("import.meta.url = {url};\n{source}")
}

impl ModuleRequest {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Provides module source text.
    pub fn resolve(mut self, scope: &Scope<'_>, source: &str) -> JsResult<()> {
        self.done = true;
        let mut err: sys::JSStringRef = ptr::null();
        let source = module_source_with_url(source, &self.key);
        let script = unsafe {
            sys::modules::lp_jsc_module_script_create(
                scope.realm.global_ctx()?,
                source.as_ptr().cast(),
                source.len(),
                self.key.as_ptr().cast(),
                self.key.len(),
                &mut err,
            )
        };
        if script.is_null() {
            let msg = if err.is_null() {
                "module creation failed".to_string()
            } else {
                unsafe { JsString::from_raw_owned(err) }.to_string()
            };
            let e = JsError::Type(msg).to_value(scope);
            return self.reject.get(scope).as_object().expect("reject fn").call(None, &[e]).map(|_| ());
        }
        let script = unsafe { Value::from_raw(scope.ctx, script) };
        self.resolve.get(scope).as_object().expect("resolve fn").call(None, &[script]).map(|_| ())
    }

    /// Fails the import with `error` (e.g. network error, MIME mismatch).
    pub fn reject(mut self, scope: &Scope<'_>, error: &JsError) -> JsResult<()> {
        self.done = true;
        let e = error.to_value(scope);
        self.reject.get(scope).as_object().expect("reject fn").call(None, &[e]).map(|_| ())
    }
}

impl Drop for ModuleRequest {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // Dropped without completion: reject so the import promise cannot hang.
        let Some(realm) = self.realm.upgrade() else { return };
        let Ok(ctx) = realm.global_ctx() else { return };
        let scope = Scope::new(ctx as sys::JSContextRef, &realm);
        let e = JsError::Type(format!("Failed to fetch dynamically imported module: {}", self.url)).to_value(&scope);
        if let Some(f) = self.reject.get(&scope).as_object() {
            let _ = f.call(None, &[e]);
        }
    }
}

/// Owner-thread scheduling callback run after a native binding returns.
/// See [`Realm::set_native_callback_hook`] for borrowing and reentrancy rules.
pub type NativeCallbackHook = Rc<dyn for<'s> Fn(&Scope<'s>)>;

/// Browser notification for a rejected promise; true cancels default reporting.
pub type UnhandledRejectionHandler = for<'s> fn(&Scope<'s>, Value<'s>, Value<'s>) -> JsResult<bool>;

/// Embedder step of [`Scope::report_exception`] (HTML "report an exception"
/// fires an `error` event at the Window); true means the exception was
/// handled and the engine hooks are not told.
pub type ExceptionReporter = for<'s> fn(&Scope<'s>, &JsException) -> bool;

pub(crate) struct RealmInner {
    pub(crate) id: RealmId,
    name: String,
    origin: RefCell<Option<String>>,
    pub(crate) engine: Rc<EngineInner>,
    pub(crate) ctx: Cell<sys::JSGlobalContextRef>,
    protos: RefCell<Vec<Option<Persistent>>>,
    /// Interface objects as seen by script (proxies over `targets`).
    ctors: RefCell<Vec<Option<Persistent>>>,
    /// The constructor functions behind the interface proxies.
    targets: RefCell<Vec<Option<Persistent>>>,
    /// Whether each class's members are installed in this realm.
    populated: RefCell<Vec<bool>>,
    identity: RefCell<rustc_hash::FxHashMap<IdentityKey, sys::JSWeakRef>>,
    /// One JavaScript string per shared host string (by pointer identity),
    /// protected for the realm's life: `tagName` and friends return the same
    /// few dozen values tens of thousands of times per selector query, and a
    /// fresh JSString each time was most of their cost. The `Arc` keeps the
    /// key's address from being reused.
    pub(crate) shared_strings: RefCell<rustc_hash::FxHashMap<usize, (std::sync::Arc<str>, sys::JSValueRef)>>,
    /// The realm's owned wrappers (`IdentityKey::OWNED`), linked through
    /// their payload headers: `dispose` releases their weak handles, which
    /// must not outlive the VM.
    pub(crate) owned_head: Cell<*mut Header>,
    helpers: RefCell<Option<Persistent>>,
    helper_cache: RefCell<rustc_hash::FxHashMap<&'static str, Persistent>>,
    /// The realm's `Object.prototype`, for the plain-object fast path of `is_plain_object`.
    object_proto: RefCell<Option<Persistent>>,
    /// The realm's `Function.prototype`: the prototype of the native
    /// functions it makes (`Scope::function`, interface members).
    function_proto: RefCell<Option<Persistent>>,
    /// Functions compiled by `call_function*`, keyed by source: CDP clients
    /// send the same (large) `functionDeclaration` on every call.
    fn_cache: RefCell<HashMap<u64, (String, Persistent)>>,
    dom_exception: RefCell<Option<Persistent>>,
    module_loader: Cell<*mut c_void>,
    module_fetcher: RefCell<Option<Rc<dyn ModuleFetcher>>>,
    native_callback_hook: RefCell<Option<NativeCallbackHook>>,
    unhandled_rejection_handler: Cell<Option<UnhandledRejectionHandler>>,
    exception_reporter: Cell<Option<ExceptionReporter>>,
    /// Optional cheap test of whether the hook has anything to do; when it
    /// says no, the hook (and the rooting of the callback result) is skipped.
    native_callback_pending: RefCell<Option<Rc<dyn Fn() -> bool>>>,
    native_callback_hook_active: Cell<bool>,
    /// Sources of top-level modules passed to `eval_module`; JSC requests the
    /// entry module itself through the fetch hook.
    inline_modules: RefCell<HashMap<String, String>>,
    embedder: RefCell<Option<Rc<dyn Any>>>,
    named_properties: RefCell<Option<Rc<dyn NamedProperties>>>,
    global_class: Option<ClassId>,
    /// Payload of the global object (set once; freed when the realm is dropped,
    /// never while a callback can still borrow it).
    global_payload: Cell<*mut Header>,
    /// `JSContextGetGlobalObject` of the realm's context, cached: the C API
    /// call takes the JSC lock and binding calls ask for it constantly.
    global_object: Cell<sys::JSObjectRef>,
    /// The global object behind that proxy (null when unknown). A custom
    /// accessor read as a global variable receives it as its receiver.
    global_target: Cell<sys::JSObjectRef>,
}

impl RealmInner {
    pub(crate) fn named_properties(&self) -> Option<Rc<dyn NamedProperties>> {
        self.named_properties.borrow().clone()
    }

    pub(crate) fn native_callback_hook(&self) -> Option<NativeCallbackHook> {
        if self.ctx.get().is_null() || self.native_callback_hook_active.get() {
            return None;
        }
        // Runs after every native callback: ask whether there is anything to
        // do before cloning the hook (the usual answer is no).
        if let Some(pending) = self.native_callback_pending.borrow().as_ref() {
            if !pending() {
                return None;
            }
        }
        self.native_callback_hook.borrow().clone()
    }

    pub(crate) fn run_native_callback_hook(&self, scope: &Scope<'_>, hook: NativeCallbackHook) {
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        self.native_callback_hook_active.set(true);
        let _reset = Reset(&self.native_callback_hook_active);
        hook(scope);
    }

    pub(crate) fn global_ctx(&self) -> JsResult<sys::JSGlobalContextRef> {
        let c = self.ctx.get();
        if c.is_null() {
            Err(JsError::ContextDestroyed)
        } else {
            Ok(c)
        }
    }

    pub(crate) fn dom_exception_ctor<'s>(&self, scope: &Scope<'s>) -> Option<Object<'s>> {
        self.dom_exception.borrow().as_ref().and_then(|p| p.get(scope).as_object())
    }

    /// `helper` with the function handle kept per realm: a property read on
    /// the helpers object costs more than the call for the hot helpers.
    fn cached_helper<'s>(&self, scope: &Scope<'s>, name: &'static str) -> JsResult<Object<'s>> {
        if let Some(p) = self.helper_cache.borrow().get(name) {
            return p.get(scope).as_object().ok_or(JsError::Type("missing helper".into()));
        }
        let f = self.helper(scope, name)?;
        self.helper_cache.borrow_mut().insert(name, Persistent::new(scope, f.value()));
        Ok(f)
    }
    fn helper<'s>(&self, scope: &Scope<'s>, name: &str) -> JsResult<Object<'s>> {
        let h = self.helpers.borrow();
        let h = h.as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
        h.as_object().expect("helpers object").get(name)?.as_object().ok_or(JsError::Type("missing helper".into()))
    }

    pub(crate) fn prune_identity(&self, key: IdentityKey) {
        let mut map = self.identity.borrow_mut();
        if let Some(weak) = map.get(&key).copied() {
            let (_, release, get) = sys::spi::get().weak().expect("weak caps checked");
            // SAFETY: weak handles are owned by this map.
            unsafe {
                if get(weak).is_null() {
                    release(self.engine.group(), weak);
                    map.remove(&key);
                }
            }
        }
    }

    pub(crate) fn dispose(&self) {
        let ctx = self.ctx.replace(ptr::null_mut());
        if ctx.is_null() {
            return;
        }
        self.global_object.set(ptr::null_mut());
        self.engine.realms.borrow_mut().remove(&(ctx as usize));
        let loader = self.module_loader.replace(ptr::null_mut());
        if !loader.is_null() {
            unsafe { sys::modules::lp_jsc_module_loader_release(loader) };
        }
        self.module_fetcher.borrow_mut().take();
        // Release captured embedder state without holding the hook cell borrow.
        drop(self.native_callback_hook.take());
        self.unhandled_rejection_handler.set(None);
        self.exception_reporter.set(None);
        drop(self.native_callback_pending.take());
        self.protos.borrow_mut().clear();
        self.ctors.borrow_mut().clear();
        self.targets.borrow_mut().clear();
        self.populated.borrow_mut().clear();
        self.helpers.borrow_mut().take();
        self.helper_cache.borrow_mut().clear();
        self.object_proto.borrow_mut().take();
        self.function_proto.borrow_mut().take();
        self.fn_cache.borrow_mut().clear();
        self.dom_exception.borrow_mut().take();
        if let Some((_, release, _)) = sys::spi::get().weak() {
            for (_, w) in self.identity.borrow_mut().drain() {
                unsafe { release(self.engine.group(), w) };
            }
            // Owned wrappers: nothing keeps them any more, and their handles
            // go now (the payloads stay until their objects die).
            let mut h = self.owned_head.replace(ptr::null_mut());
            while !h.is_null() {
                // SAFETY: a payload leaves the list before it is freed
                // (`class::release_owned`), so every entry is live.
                unsafe {
                    let next = (*h).owned_next.replace(ptr::null_mut());
                    (*h).owned_prev.set(ptr::null_mut());
                    (*h).keep_alive.store(0, std::sync::atomic::Ordering::Relaxed);
                    let weak = (*h).weak.replace(ptr::null_mut());
                    if !weak.is_null() {
                        release(self.engine.group(), weak);
                    }
                    h = next;
                }
            }
        }
        for (_, (_, v)) in self.shared_strings.borrow_mut().drain() {
            // SAFETY: protected by `shared_string` on this still-live context.
            unsafe { sys::JSValueUnprotect(ctx, v) };
        }
        self.embedder.borrow_mut().take();
        self.named_properties.borrow_mut().take();
        // SAFETY: we owned one retain on the global context.
        unsafe { sys::JSGlobalContextRelease(ctx) };
        if let Some(h) = self.engine.hooks() {
            h.realm_destroyed(self.id);
        }
    }
}

impl Drop for RealmInner {
    fn drop(&mut self) {
        self.dispose();
        let h = self.global_payload.replace(ptr::null_mut());
        if !h.is_null() {
            // SAFETY: owned payload; no Scope of this realm exists any more
            // (scopes borrow the Rc that is being dropped).
            unsafe { ((*h).drop_fn)(h) };
        }
    }
}

/// Owning handle to a realm. Cloning shares it; the realm is disposed by
/// [`Realm::dispose`] or when the last handle (and any in-flight callback) drops.
#[derive(Clone)]
pub struct Realm {
    pub(crate) inner: Rc<RealmInner>,
}

impl Realm {
    /// The live realm `id` of this thread's engine.
    pub fn by_id(id: RealmId) -> Option<Realm> {
        let inner = crate::engine::current_inner()?.realm_by_id(id)?;
        (!inner.ctx.get().is_null()).then_some(Realm { inner })
    }

    pub fn set_unhandled_rejection_handler(&self, handler: UnhandledRejectionHandler) {
        self.inner.unhandled_rejection_handler.set(Some(handler));
    }

    /// Installs the embedder's "report an exception" step, run by
    /// [`Scope::report_exception`] before the engine hooks (`None` removes it).
    pub fn set_exception_reporter(&self, reporter: Option<ExceptionReporter>) {
        self.inner.exception_reporter.set(reporter);
    }

    pub fn id(&self) -> RealmId {
        self.inner.id
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn origin(&self) -> Option<String> {
        self.inner.origin.borrow().clone()
    }

    /// Sets the origin used for root-relative module specifiers. For realms
    /// created ahead of their frame (see lp-browser's spare realm); call
    /// before any script runs in the realm.
    pub fn set_origin(&self, origin: Option<String>) {
        *self.inner.origin.borrow_mut() = origin;
    }

    pub fn is_disposed(&self) -> bool {
        self.inner.ctx.get().is_null()
    }

    /// Enters the realm. Fails after disposal or once the engine owner was
    /// dropped. Everything derived from the scope borrows this handle, which
    /// keeps the VM alive.
    pub fn scope(&self) -> JsResult<Scope<'_>> {
        let ctx = self.inner.global_ctx()?;
        if self.inner.engine.is_shut_down() {
            return Err(JsError::ContextDestroyed);
        }
        Ok(Scope { ctx: ctx as sys::JSContextRef, realm: &self.inner })
    }

    /// Tears the realm down now: callbacks into its functions afterwards throw
    /// "Execution context was destroyed.", persistent prototypes are released
    /// and `EngineHooks::realm_destroyed` fires. Idempotent.
    pub fn dispose(&self) {
        self.inner.dispose();
    }

    /// Sets the payload of the global object (realm created with `global_class`).
    /// May be called once; the payload lives until the realm is dropped.
    pub fn set_global_data<C: JsClass>(&self, data: C::Data) -> JsResult<()> {
        self.inner.global_ctx()?;
        let id = self
            .inner
            .engine
            .classes
            .borrow()
            .id_of::<C>()
            .ok_or_else(|| JsError::Type(format!("{} not registered", C::NAME)))?;
        if self.inner.global_class != Some(id) {
            return Err(JsError::Type(format!("realm global is not a {}", C::NAME)));
        }
        if !self.inner.global_payload.get().is_null() {
            return Err(JsError::Type("global data already set".into()));
        }
        let h = class::new_payload(&self.inner.engine, id, Kind::Instance, &self.inner, None, data);
        self.inner.global_payload.set(h);
        Ok(())
    }

    /// Attaches embedder state reachable from callbacks via [`Scope::embedder`].
    pub fn set_embedder<T: Any>(&self, data: Rc<T>) {
        *self.inner.embedder.borrow_mut() = Some(data);
    }

    /// Answers the names the global lacks, through the named properties object
    /// in a window realm's prototype chain. Needs the patched JavaScriptCore.
    pub fn set_named_properties(&self, named: Rc<dyn NamedProperties>) {
        *self.inner.named_properties.borrow_mut() = Some(named);
    }

    /// Installs a scheduling hook after native callback bodies return and before
    /// JavaScript resumes, including normal error returns. Returned values stay
    /// rooted while it runs. Termination and destroyed realms skip the hook.
    ///
    /// Use this to inspect pending work and enqueue a microtask, not to deliver
    /// observer callbacks or dispose the realm. The callback body's local borrows
    /// have ended; an outer reentrant native callback may still hold borrows, so
    /// embedders must check those safely. Hook-induced callbacks in this realm do
    /// not recursively invoke the hook. Ordinary nested callbacks do invoke it.
    /// Capture embedder state weakly to avoid cycles. Disposal clears the hook;
    /// passing `None` uninstalls it. Panics become native-callback JS errors.
    pub fn set_native_callback_hook(&self, hook: Option<NativeCallbackHook>) -> JsResult<()> {
        self.scope()?;
        drop(self.inner.native_callback_hook.replace(hook));
        Ok(())
    }

    /// Lets the native callback hook be skipped while `pending` returns false.
    /// It must return true whenever the hook would do anything.
    pub fn set_native_callback_pending(&self, pending: Option<Rc<dyn Fn() -> bool>>) -> JsResult<()> {
        self.scope()?;
        drop(self.inner.native_callback_pending.replace(pending));
        Ok(())
    }

    /// Runs `f` with the native callback hook enabled again for the native
    /// callbacks `f` causes in this realm, when called from inside the hook
    /// (elsewhere it only runs `f`). For a hook that itself runs page script
    /// (HTML post-connection steps execute an inserted `<script>` before the
    /// inserting call returns): that script's own DOM calls need the hook.
    pub fn reenter_native_callback_hook<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Restore<'a>(&'a Cell<bool>, bool);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let active = &self.inner.native_callback_hook_active;
        let _restore = Restore(active, active.replace(false));
        f()
    }

    pub fn set_module_fetcher(&self, fetcher: Option<Rc<dyn ModuleFetcher>>) {
        *self.inner.module_fetcher.borrow_mut() = fetcher;
    }

    pub fn ptr_eq(&self, other: &Realm) -> bool {
        Rc::ptr_eq(&self.inner, &other.inner)
    }

    /// A non-owning handle (does not keep the realm alive).
    pub fn downgrade(&self) -> WeakRealm {
        WeakRealm(Rc::downgrade(&self.inner))
    }
}

/// Non-owning realm handle; see [`Realm::downgrade`].
#[derive(Clone)]
pub struct WeakRealm(std::rc::Weak<RealmInner>);

impl WeakRealm {
    /// The realm, if still alive and not disposed.
    pub fn upgrade(&self) -> Option<Realm> {
        self.0.upgrade().map(|inner| Realm { inner }).filter(|r| !r.is_disposed())
    }
}

impl std::fmt::Debug for Realm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Realm({}, {:?})", self.inner.id.0, self.inner.name)
    }
}

/// Working handle inside a realm, valid for `'s`. Created by
/// [`Realm::scope`] or passed to native callbacks.
pub struct Scope<'s> {
    pub(crate) ctx: sys::JSContextRef,
    pub(crate) realm: &'s Rc<RealmInner>,
}

/// A property name as JavaScriptCore holds it.
#[derive(Clone, Copy)]
pub enum NameUnits<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

/// The names of a window's named properties object (see
/// [`Realm::set_named_properties`]). Consulted only for names the global lacks;
/// members of `Object.prototype` win over it.
pub trait NamedProperties {
    /// False if `resolve` would find nothing. Runs on every global miss, outside
    /// the native callback machinery, so it must be cheap and call no JavaScript API.
    fn may_resolve(&self, name: NameUnits<'_>) -> bool;
    fn resolve<'s>(&self, scope: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>>;
}

/// The realm of the current engine whose context is `ctx`: the realm a
/// native callback runs in. Callbacks that know a candidate (a member
/// function's or closure's own realm, a receiver's) try it first with
/// [`realm_if_ctx`]; this is for the rest.
pub(crate) fn realm_for_ctx(ctx: sys::JSContextRef) -> Option<Rc<RealmInner>> {
    let engine = crate::engine::current_inner()?;
    // Callbacks receive the global context itself; only other contexts need
    // the (locking) lookup of their global context.
    match engine.realm_for_global(ctx as sys::JSGlobalContextRef) {
        Some(realm) => Some(realm),
        None => {
            let global = unsafe { sys::JSContextGetGlobalContext(ctx) };
            engine.realm_for_global(global)
        }
    }
}

/// `candidate`, if it is what [`realm_for_ctx`] would find for `ctx`: a live
/// realm of a live engine whose global context is `ctx` (only the thread's
/// current engine is live, and a realm whose context is set is in its
/// engine's map). Costs a weak upgrade and two compares, no lookup.
#[inline]
pub(crate) fn realm_if_ctx(candidate: &Weak<RealmInner>, ctx: sys::JSContextRef) -> Option<Rc<RealmInner>> {
    let realm = candidate.upgrade()?;
    (realm.ctx.get() as sys::JSContextRef == ctx && !ctx.is_null() && realm.engine.is_live()).then_some(realm)
}

/// Whether `realm` is the realm `scope` works in.
#[inline]
pub(crate) fn is_scope_realm(realm: &Weak<RealmInner>, scope: &Scope<'_>) -> bool {
    std::ptr::eq(realm.as_ptr(), Rc::as_ptr(scope.realm))
}

impl<'s> Scope<'s> {
    /// Scope for a native callback (JSC is on the stack; not counted).
    pub(crate) fn new(ctx: sys::JSContextRef, realm: &'s Rc<RealmInner>) -> Scope<'s> {
        Scope { ctx, realm }
    }

    pub fn ctx(&self) -> sys::JSContextRef {
        self.ctx
    }

    pub fn realm(&self) -> Realm {
        Realm { inner: self.realm.clone() }
    }

    pub fn realm_id(&self) -> RealmId {
        self.realm.id
    }

    pub(crate) fn realm_inner(&self) -> &RealmInner {
        self.realm
    }

    pub(crate) fn engine_inner(&self) -> &Rc<EngineInner> {
        &self.realm.engine
    }

    /// Embedder state set with [`Realm::set_embedder`].
    pub fn embedder<T: Any>(&self) -> Option<Rc<T>> {
        self.realm.embedder.borrow().clone().and_then(|d| d.downcast::<T>().ok())
    }

    /// Evaluates a classic script.
    pub fn eval(&self, source: &str, url: Option<&str>, line: i32) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let src = JsString::new(source);
        let url = url.map(JsString::new);
        let mut exc = ptr::null();
        let v = unsafe {
            sys::JSEvaluateScript(
                self.ctx,
                src.as_raw(),
                ptr::null_mut(),
                url.as_ref().map_or(ptr::null(), |u| u.as_raw()),
                line.max(1),
                &mut exc,
            )
        };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    /// Parses without running; returns the SyntaxError on failure.
    pub fn check_syntax(&self, source: &str, url: Option<&str>) -> JsResult<()> {
        let src = JsString::new(source);
        let url = url.map(JsString::new);
        let mut exc = ptr::null();
        unsafe {
            sys::JSCheckScriptSyntax(
                self.ctx,
                src.as_raw(),
                url.as_ref().map_or(ptr::null(), |u| u.as_raw()),
                1,
                &mut exc,
            )
        };
        check(self.ctx, exc)
    }

    /// Merges an import map (`<script type="importmap">` text) into the realm's
    /// map; `base` is the document base URL. Returns the warnings about ignored
    /// entries, or an error for a map that is invalid as a whole. Realms that
    /// never add one resolve specifiers without consulting a map.
    pub fn add_import_map(&self, source: &str, base: &str) -> JsResult<Vec<String>> {
        if !self.realm.engine.caps.import_maps {
            return Err(JsError::Unsupported("import maps (patched JavaScriptCore)"));
        }
        let mut messages: sys::JSStringRef = ptr::null();
        // SAFETY: the slices outlive the call, which copies them; `messages` comes back owned.
        let merged = unsafe {
            sys::modules::lp_jsc_import_map_add(
                self.realm.global_ctx()?,
                source.as_ptr().cast(),
                source.len(),
                base.as_ptr().cast(),
                base.len(),
                &mut messages,
            )
        } == 1;
        let messages =
            if messages.is_null() { String::new() } else { unsafe { JsString::from_raw_owned(messages) }.to_string() };
        if !merged {
            return Err(JsError::Type(format!("Invalid import map: {messages}")));
        }
        Ok(messages.lines().map(str::to_owned).collect())
    }

    /// Reports `message` as `console.warn` would, without running page code.
    pub fn console_warn(&self, message: &str) {
        if let Some(h) = self.realm.engine.hooks() {
            h.console_api(&self.realm(), ConsoleLevel::Warning, &[self.string(message)]);
        }
    }

    /// Evaluates a module script; returns its evaluation promise. Imports go
    /// through the realm's [`ModuleFetcher`].
    ///
    /// JSC's module registry is keyed by URL: a URL evaluated once is not
    /// evaluated again, so inline module scripts need distinct URLs (the
    /// browser layer appends a fragment, which leaves relative resolution intact).
    pub fn eval_module(&self, source: &str, url: &str) -> JsResult<Value<'s>> {
        if !self.realm.engine.caps.modules {
            return Err(JsError::Unsupported("ES modules (JSScript module SPI)"));
        }
        self.realm.inline_modules.borrow_mut().insert(url.to_string(), source.to_string());
        let mut err: sys::JSStringRef = ptr::null();
        let source_with_url = module_source_with_url(source, url);
        let v = unsafe {
            sys::modules::lp_jsc_module_evaluate(
                self.realm.global_ctx()?,
                source_with_url.as_ptr().cast(),
                source_with_url.len(),
                url.as_ptr().cast(),
                url.len(),
                &mut err,
            )
        };
        if v.is_null() {
            let msg = if err.is_null() {
                "module evaluation failed".to_string()
            } else {
                unsafe { JsString::from_raw_owned(err) }.to_string()
            };
            return Err(JsError::Type(msg));
        }
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    /// Calls `function_source` (a function expression) with `this` and `args`;
    /// the shape of CDP `Runtime.callFunctionOn`.
    pub fn call_function(
        &self,
        function_source: &str,
        this: Option<Value<'_>>,
        args: &[Value<'_>],
    ) -> JsResult<Value<'s>> {
        let f = self.function_for_source(function_source)?;
        match this {
            Some(t) if !t.is_object() => {
                // Primitive receivers: route through Function.prototype.call.
                let call = self.realm.helper(self, "callWith")?;
                let mut all = Vec::with_capacity(args.len() + 2);
                all.push(f.value());
                all.push(t);
                all.extend_from_slice(args);
                call.call(None, &all)
            }
            Some(t) => f.call(t.as_object(), args),
            None => f.call(None, args),
        }
    }

    /// [`Scope::call_function`] with arguments in a [`crate::RootedVec`]
    /// (CDP `callFunctionOn` converts arguments one by one; each conversion can
    /// trigger GC, so they must be rooted, not kept in a `Vec<Value>`).
    /// The function object for `function_source`, compiled once per realm
    /// and source text. The same object is returned on later calls: its
    /// identity and expando properties persist, which inspector clients do
    /// not observe. Bounded; a full cache is emptied.
    fn function_for_source(&self, function_source: &str) -> JsResult<Object<'s>> {
        use std::hash::{Hash, Hasher};
        const MAX_CACHED: usize = 512;
        // Fx: hashing a 38 KB declaration with SipHash costs ~40 us.
        let mut hasher = rustc_hash::FxHasher::default();
        function_source.hash(&mut hasher);
        let key = hasher.finish();
        if let Some((source, f)) = self.realm.fn_cache.borrow().get(&key) {
            if source == function_source {
                if let Some(o) = f.get(self).as_object() {
                    return Ok(o);
                }
            }
        }
        let f = self.eval(&format!("({function_source}\n)"), None, 1)?;
        let f = f
            .as_object()
            .filter(|o| o.is_function())
            .ok_or_else(|| JsError::Type("Given expression does not evaluate to a function".into()))?;
        let mut cache = self.realm.fn_cache.borrow_mut();
        if cache.len() >= MAX_CACHED {
            cache.clear();
        }
        cache.insert(key, (function_source.to_owned(), Persistent::new(self, f.value())));
        Ok(f)
    }

    pub fn call_function_rooted(
        &self,
        function_source: &str,
        this: Option<&Persistent>,
        args: &crate::RootedVec,
    ) -> JsResult<Value<'s>> {
        let f = self.function_for_source(function_source)?;
        let this_v = this.map(|p| p.get(self));
        match this_v {
            Some(t) if !t.is_object() => {
                let call = self.realm.helper(self, "callWith")?;
                let mut all = crate::RootedVec::new();
                all.push(self, f.value());
                all.push(self, t);
                for v in args.iter(self) {
                    all.push(self, v);
                }
                call.call_rooted(None, &all)
            }
            Some(t) => f.call_rooted(t.as_object(), args),
            None => f.call_rooted(None, args),
        }
    }

    /// Creates a function backed by a Rust closure (may capture state).
    pub fn function(
        &self,
        name: &str,
        f: impl for<'a> Fn(&Scope<'a>, Value<'a>, Args<'a>) -> JsResult<Value<'a>> + 'static,
    ) -> JsResult<Object<'s>> {
        let data: ClosureData = Box::new(f);
        let h = class::new_payload(&self.realm.engine, u32::MAX, Kind::Closure, self.realm, None, data);
        let closure_class = self.realm.engine.classes.borrow().closure;
        let obj = unsafe { sys::JSObjectMake(self.ctx, closure_class, h as *mut c_void) };
        let obj = unsafe { Object::from_raw(self.ctx, obj) };
        if let Some(p) = self.function_prototype()? {
            obj.set_prototype(p);
        }
        let nm = self.string(name);
        let _ = self.realm.cached_helper(self, "setName").and_then(|h| h.call(None, &[obj.value(), nm]));
        Ok(obj)
    }

    /// The realm's own `Function.prototype` (%Function.prototype%), kept
    /// from realm creation until disposal: what native functions get,
    /// whatever page script did to the global `Function` since. A disposed
    /// realm reads the global's.
    fn function_prototype(&self) -> JsResult<Option<Value<'s>>> {
        if let Some(p) = self.realm.function_proto.borrow().as_ref() {
            return Ok(Some(p.get(self)));
        }
        Ok(self.global().get_atom("Function")?.as_object().and_then(|f| f.get_atom("prototype").ok()))
    }

    /// Defines `name` on the global (not enumerable) as a function taking one
    /// string, reported to [`EngineHooks::binding_called`] (CDP
    /// `Runtime.addBinding`).
    pub fn install_binding(&self, name: &str) -> JsResult<()> {
        let binding = name.to_owned();
        let f = self.function(name, move |scope, _this, args| {
            let payload = match (args.len(), args.get(0).as_string()) {
                (1, Some(payload)) => payload,
                _ => return Err(JsError::Type("Invalid arguments: should be exactly one string.".into())),
            };
            if let Some(hooks) = scope.realm.engine.hooks() {
                hooks.binding_called(&scope.realm(), &binding, &payload);
            }
            Ok(scope.undefined())
        })?;
        self.global().set_with(&JsString::new(name), f.value(), sys::kJSPropertyAttributeDontEnum)
    }

    /// `new Promise` with externally controlled settlement.
    pub fn deferred(&self) -> JsResult<Deferred> {
        let mut resolve: sys::JSObjectRef = ptr::null_mut();
        let mut reject: sys::JSObjectRef = ptr::null_mut();
        let mut exc = ptr::null();
        let p = unsafe { sys::JSObjectMakeDeferredPromise(self.ctx, &mut resolve, &mut reject, &mut exc) };
        check(self.ctx, exc)?;
        unsafe {
            Ok(Deferred {
                promise: Persistent::new(self, Value::from_raw(self.ctx, p as sys::JSValueRef)),
                resolve: Persistent::new(self, Value::from_raw(self.ctx, resolve as sys::JSValueRef)),
                reject: Persistent::new(self, Value::from_raw(self.ctx, reject as sys::JSValueRef)),
            })
        }
    }

    /// `Promise.resolve(value).then(on_fulfilled, on_rejected)` with native
    /// reactions (run as microtasks). Uses the realm's original `Promise`.
    /// An undefined value uses a private settled promise whose constructor
    /// cannot be replaced, so native scheduling never invokes author species.
    pub fn then(
        &self,
        value: Value<'_>,
        on_fulfilled: impl for<'a> FnOnce(&Scope<'a>, Value<'a>) + 'static,
        on_rejected: impl for<'a> FnOnce(&Scope<'a>, Value<'a>) + 'static,
    ) -> JsResult<()> {
        let ok = RefCell::new(Some(on_fulfilled));
        let err = RefCell::new(Some(on_rejected));
        let fok = self.function("", move |s, _this, a| {
            if let Some(f) = ok.borrow_mut().take() {
                f(s, a.get(0));
            }
            Ok(s.undefined())
        })?;
        let ferr = self.function("", move |s, _this, a| {
            if let Some(f) = err.borrow_mut().take() {
                f(s, a.get(0));
            }
            Ok(s.undefined())
        })?;
        self.realm.cached_helper(self, "then")?.call(None, &[value, fok.value(), ferr.value()])?;
        Ok(())
    }

    // -- conversions ------------------------------------------------------

    /// WebIDL `sequence<T>` conversion of `value`: iterates it with its own
    /// iterator protocol (arrays included, so overridden `@@iterator`s and
    /// holes are honoured) and returns the yielded values as a fresh array
    /// of this realm. `what` names the operation in the TypeError for
    /// non-iterable values (https://webidl.spec.whatwg.org/#es-sequence).
    pub fn sequence(&self, value: Value<'_>, what: &str) -> JsResult<Object<'s>> {
        let seq = self.realm.cached_helper(self, "sequence")?.call(None, &[value, self.string(what)])?;
        seq.as_object().ok_or_else(|| JsError::Type("sequence conversion produced no array".into()))
    }

    // -- host objects ------------------------------------------------------

    /// Returns the wrapper for `data`, reusing the existing one for `key` in this
    /// realm (in which case `data` is dropped).
    pub fn wrap<C: JsClass>(&self, data: C::Data, key: Option<IdentityKey>) -> JsResult<Object<'s>> {
        if let Some(k) = key.filter(|k| !k.is_owned()) {
            if let Some(o) = self.lookup_identity(k) {
                return Ok(o);
            }
        }
        let id = self.class_id::<C>()?;
        populate_class(self, id)?;
        let proto = self.proto(id)?;
        self.make_wrapper::<C>(id, data, key, proto)
    }

    /// Creates a wrapper whose prototype comes from `new_target` (constructors,
    /// subclassing). Optionally registers it under `key`.
    pub fn wrap_new<C: JsClass>(
        &self,
        data: C::Data,
        new_target: Object<'_>,
        key: Option<IdentityKey>,
    ) -> JsResult<Object<'s>> {
        let id = self.class_id::<C>()?;
        populate_class(self, id)?;
        let proto = match new_target.get("prototype")?.as_object() {
            Some(p) => p.value(),
            None => self.proto(id)?,
        };
        let proto = unsafe { Value::from_raw(self.ctx, proto.raw) };
        self.make_wrapper::<C>(id, data, key, proto)
    }

    /// Whether [`IdentityKey::OWNED`] wrappers can be made (patched
    /// JavaScriptCore with `JSWeakCreateKeptWhile`).
    pub fn owned_wrappers(&self) -> bool {
        let spi = sys::spi::get();
        spi.weak_create_kept_while.is_some() && spi.weak().is_some()
    }

    /// The reference of an owned wrapper of this realm (`None` for any other
    /// object).
    pub fn wrapper_ref(&self, object: Object<'_>) -> Option<WrapperRef> {
        // SAFETY: a live object of this VM.
        let h = unsafe { self.header_of_object(object.raw) }?;
        // SAFETY: the object's live payload.
        (!unsafe { (*h).weak.get() }.is_null()).then_some(WrapperRef(h as usize))
    }

    /// The wrapper `r` refers to, if it is still alive.
    ///
    /// # Safety
    /// The wrapper's payload has not been dropped (see [`WrapperRef`]).
    pub unsafe fn wrapper(&self, r: WrapperRef) -> Option<Object<'s>> {
        let h = r.0 as *mut Header;
        unsafe {
            let weak = (*h).weak.get();
            if weak.is_null() {
                return None;
            }
            let (_, _, get) = sys::spi::get().weak()?;
            let o = get(weak);
            (!o.is_null()).then(|| Object::from_raw(self.ctx, o))
        }
    }

    /// Existing wrapper for `key` in this realm, if alive.
    pub fn lookup_identity(&self, key: IdentityKey) -> Option<Object<'s>> {
        let map = self.realm.identity.borrow();
        let weak = *map.get(&key)?;
        let (_, _, get) = sys::spi::get().weak()?;
        let o = unsafe { get(weak) };
        (!o.is_null()).then(|| unsafe { Object::from_raw(self.ctx, o) })
    }

    fn make_wrapper<C: JsClass>(
        &self,
        id: ClassId,
        data: C::Data,
        key: Option<IdentityKey>,
        proto: Value<'_>,
    ) -> JsResult<Object<'s>> {
        let class = self.realm.engine.classes.borrow().classes[id as usize].instance_class;
        let owned = key.filter(|k| k.is_owned());
        let key = key.filter(|k| !k.is_owned());
        let kept_while = match owned {
            Some(_) => {
                Some(sys::spi::get().weak_create_kept_while.ok_or(JsError::Unsupported("JSWeakCreateKeptWhile"))?)
            }
            None => None,
        };
        let h = class::new_payload(&self.realm.engine, id, Kind::Instance, self.realm, key, data);
        let obj = unsafe { sys::JSObjectMake(self.ctx, class, h as *mut c_void) };
        let obj = unsafe { Object::from_raw(self.ctx, obj) };
        obj.set_prototype(proto);
        if let (Some(mode), Some(kept_while)) = (owned, kept_while) {
            // SAFETY: h is the live payload of obj; its flag byte lives as
            // long as the payload, which outlives the handle (released in
            // `class::release_owned` or `RealmInner::dispose`).
            unsafe {
                (*h).keep_alive.store(u8::from(mode == IdentityKey::OWNED_KEPT), std::sync::atomic::Ordering::Relaxed);
                let weak = kept_while(self.realm.engine.group(), obj.raw, (*h).keep_alive.as_ptr());
                (*h).weak.set(weak);
                let head = self.realm.owned_head.get();
                (*h).owned_next.set(head);
                if !head.is_null() {
                    (*head).owned_prev.set(h);
                }
                self.realm.owned_head.set(h);
            }
        }
        if let Some(k) = key {
            let (create, release, _) = sys::spi::get().weak().ok_or(JsError::Unsupported("JSWeak"))?;
            let weak = unsafe { create(self.realm.engine.group(), obj.raw) };
            if let Some(old) = self.realm.identity.borrow_mut().insert(k, weak) {
                unsafe { release(self.realm.engine.group(), old) };
            }
        }
        Ok(obj)
    }

    /// Borrows the payload of a `C` (or subclass) wrapper. The returned guard
    /// keeps the wrapper (and so the payload) alive while it exists.
    pub fn unwrap<C: JsClass>(&self, value: Value<'_>) -> Option<Unwrapped<'s, C::Data>> {
        let (data, obj) = self.unwrap_raw::<C>(value)?;
        if !obj.is_null() {
            // SAFETY: live object; balanced by Unwrapped::drop.
            unsafe { sys::JSValueProtect(self.realm.engine.root_ctx(), obj as sys::JSValueRef) };
        }
        Some(Unwrapped { data, obj, root: self.realm.engine.root_ctx() })
    }

    /// Borrows the payload of argument `i` of a native callback. Arguments are
    /// rooted by the caller's frame for the callback's duration (JSC's
    /// argument array belongs to the calling frame, which outlives the
    /// callback), so no guard is needed.
    pub fn unwrap_arg<C: JsClass>(&self, args: &Args<'s>, i: usize) -> Option<&'s C::Data> {
        self.unwrap_raw::<C>(args.opt(i)?).map(|(d, _)| d)
    }

    /// [`Scope::unwrap`] without the guard's protect/unprotect pair, for a
    /// value something else keeps alive for `'s`: a callback's receiver or
    /// argument (rooted by the calling frame), or a value reachable from one.
    ///
    /// # Safety
    /// The wrapper must stay alive for `'s`. A value merely held in a local
    /// does not qualify: the conservative scan only sees it while the
    /// compiled code still keeps it, and a collection followed by
    /// [`crate::Engine::drain_finalized`] would free the payload.
    pub unsafe fn unwrap_rooted<C: JsClass>(&self, value: Value<'_>) -> Option<&'s C::Data> {
        self.unwrap_raw::<C>(value).map(|(d, _)| d)
    }

    /// Payload of the realm's global object, if it is (a subclass of) `C`.
    pub fn global_data<C: JsClass>(&self) -> Option<&'s C::Data> {
        let h = self.realm.global_payload.get();
        if h.is_null() {
            return None;
        }
        // SAFETY: the global payload lives as long as the realm, which this
        // scope borrows for 's.
        unsafe { class::data_of_header::<C>(h) }
    }

    /// Data and object of a `C` wrapper (object is null for the global). The
    /// brand check walks the payload's class chain (`data_of_header`): no
    /// registry borrow, no class-id lookup.
    fn unwrap_raw<C: JsClass>(&self, value: Value<'_>) -> Option<(&'s C::Data, sys::JSObjectRef)> {
        let o = value.as_object()?;
        // SAFETY: a live object of this VM; its payload lives while it does
        // (the callers root it for 's), the global's while the realm does.
        unsafe {
            let h = self.header_of_object(o.raw)?;
            let data = class::data_of_header::<C>(h)?;
            Some((data, if h == self.realm.global_payload.get() { ptr::null_mut() } else { o.raw }))
        }
    }

    /// Interface name of a native wrapper instance of this engine (not a
    /// global object), without running JavaScript. `None` for other values.
    pub fn host_class_name(&self, value: Value<'_>) -> Option<&'static str> {
        let o = value.as_object()?;
        if unsafe { sys::JSContextGetGroup(value.ctx) } != self.realm.engine.group() {
            return None;
        }
        let h = unsafe { self.header_of_object(o.raw) }?;
        if h == self.realm.global_payload.get() {
            return None;
        }
        unsafe {
            if (*h).kind != Kind::Instance {
                return None;
            }
            self.realm.engine.classes.borrow().classes.get((*h).class as usize).map(|c| c.name)
        }
    }

    /// An ordinary object whose prototype is this realm's `Object.prototype`:
    /// not callable, not an array or date, not a native wrapper. Decided
    /// natively, without invoking getters or proxy traps.
    pub fn is_plain_object(&self, value: Value<'_>) -> bool {
        let Some(o) = value.as_object() else { return false };
        if unsafe { sys::JSContextGetGroup(value.ctx) } != self.realm.engine.group() {
            return false;
        }
        if value.is_function() || value.is_array() || value.is_date() {
            return false;
        }
        if unsafe { self.header_of_object(o.raw) }.is_some() {
            return false;
        }
        let proto = self.realm.object_proto.borrow();
        let Some(proto) = proto.as_ref() else { return false };
        unsafe { sys::JSObjectGetPrototype(self.ctx, o.raw) == proto.get(self).raw }
    }

    /// The realm `object` was created in (the "associated realm" of a Web IDL
    /// callback), when it is a live realm of this engine: a function's own
    /// realm rather than its caller's, and a Proxy's rather than its target's.
    /// `None` without the global-context SPI, for an object of another engine
    /// or of a disposed realm. Runs no script.
    pub fn object_realm(&self, object: Object<'_>) -> Option<Realm> {
        let get_context = sys::spi::get().get_object_global_context?;
        if unsafe { sys::JSContextGetGroup(object.ctx) } != self.realm.engine.group() {
            return None;
        }
        // SAFETY: object is live in this VM; reading its global runs no script.
        let context = unsafe { get_context(object.raw) };
        if context.is_null() {
            return None;
        }
        if context == self.realm.ctx.get() {
            return Some(self.realm());
        }
        self.realm.engine.realm_for_global(context).map(|inner| Realm { inner })
    }

    /// Tests for one of this engine's native wrappers/closures or a same-VM
    /// global object, without invoking getters, proxy traps, or other JS.
    /// Includes globals retained by another realm after their owner is disposed.
    /// This does not classify arbitrary built-in/exotic JS objects or unwrap
    /// proxies. Non-wrapper objects require the optional global-context SPI.
    pub fn is_host_object(&self, value: Value<'_>) -> JsResult<bool> {
        // A value from another still-live engine must not reach this VM's class
        // inspection API. Keeping its source context in Value makes this check
        // possible even when the original Engine owner has been dropped.
        if unsafe { sys::JSContextGetGroup(value.ctx) } != self.realm.engine.group() {
            return Err(JsError::Type("host-object inspection across engines".into()));
        }
        let Some(object) = value.as_object() else { return Ok(false) };
        let base = self.realm.engine.classes.borrow().base;
        // The registry borrow ends before entering JSC; no Rust payload is read.
        if unsafe { sys::JSValueIsObjectOfClass(self.ctx, value.raw, base) } {
            return Ok(true);
        }
        let get_context = sys::spi::get()
            .get_object_global_context
            .ok_or(JsError::Unsupported("object global-context inspection"))?;
        // SAFETY: object is live in this VM. Its native realm remains reachable
        // through the object, independently of the Rust Realm handle. Reading
        // the global identity does not execute script or retain another realm.
        let context = unsafe { get_context(object.raw) };
        Ok(!context.is_null() && unsafe { sys::JSContextGetGlobalObject(context) } == object.raw)
    }

    /// Payload header of an object: wrappers, or this realm's global.
    unsafe fn header_of_object(&self, obj: sys::JSObjectRef) -> Option<*mut Header> {
        unsafe {
            // Every private pointer in the VM is a wrapper header or a member
            // function payload, told apart by their first word. Reading it does
            // not take the JSC lock, unlike the class check.
            let h = sys::JSObjectGetPrivate(obj) as *mut Header;
            if !h.is_null() {
                return ((*h).tag == class::PayloadTag::Wrapper).then_some(h);
            }
            if self.is_global_object(obj) {
                let h = self.realm.global_payload.get();
                return (!h.is_null()).then_some(h);
            }
        }
        None
    }

    /// Whether `obj` is the realm's global: its proxy (what script sees) or
    /// the object behind it (what a global variable's getter receives).
    pub(crate) fn is_global_object(&self, obj: sys::JSObjectRef) -> bool {
        obj == self.global_object_raw() || (!obj.is_null() && obj == self.realm.global_target.get())
    }

    /// The realm's global object (`JSContextGetGlobalObject`, cached).
    pub(crate) fn global_object_raw(&self) -> sys::JSObjectRef {
        let g = self.realm.global_object.get();
        if g.is_null() {
            unsafe { sys::JSContextGetGlobalObject(self.ctx) }
        } else {
            g
        }
    }

    /// Payload header of a callback receiver, mapping `null`/global to the
    /// realm's global object.
    ///
    /// # Safety
    /// `this` must be the receiver JSC passed to the current callback.
    pub(crate) unsafe fn header_of_receiver(&self, this: sys::JSObjectRef) -> Option<*mut Header> {
        unsafe {
            let obj = if this.is_null() { self.global_object_raw() } else { this };
            self.header_of_object(obj)
        }
    }

    /// The interface object (constructor) of `C` in this realm.
    pub fn interface<C: JsClass>(&self) -> JsResult<Object<'s>> {
        let id = self.class_id::<C>()?;
        let ctors = self.realm.ctors.borrow();
        let p = ctors.get(id as usize).and_then(|p| p.as_ref()).ok_or(JsError::ContextDestroyed)?;
        Ok(p.get(self).as_object().expect("constructor object"))
    }

    fn class_id<C: JsClass>(&self) -> JsResult<ClassId> {
        self.realm
            .engine
            .classes
            .borrow()
            .id_of::<C>()
            .ok_or_else(|| JsError::Type(format!("class {} is not registered", C::NAME)))
    }

    fn proto(&self, id: ClassId) -> JsResult<Value<'s>> {
        let protos = self.realm.protos.borrow();
        let p = protos
            .get(id as usize)
            .and_then(|p| p.as_ref())
            .ok_or_else(|| JsError::Type("class registered after this realm was created".into()))?;
        Ok(p.get(self))
    }

    /// Stores a GC-traced hidden value on a host object (wrappers and the
    /// global only). Use for per-wrapper JS state such as event listeners so
    /// cycles through it stay collectable. Keys are constants: the thread's
    /// string for each ([`crate::atom`]) is made once.
    pub fn set_hidden(&self, object: Object<'_>, key: &'static str, value: Value<'_>) -> JsResult<()> {
        self.set_hidden_raw(object, crate::string::atom_raw(key), value)
    }

    pub fn get_hidden(&self, object: Object<'_>, key: &'static str) -> JsResult<Option<Value<'s>>> {
        self.get_hidden_raw(object, crate::string::atom_raw(key))
    }

    /// [`Scope::set_hidden`] for a key computed at run time (a string is
    /// made for each call).
    pub fn set_hidden_str(&self, object: Object<'_>, key: &str, value: Value<'_>) -> JsResult<()> {
        self.set_hidden_raw(object, JsString::new(key).as_raw(), value)
    }

    /// [`Scope::get_hidden`] for a key computed at run time.
    pub fn get_hidden_str(&self, object: Object<'_>, key: &str) -> JsResult<Option<Value<'s>>> {
        self.get_hidden_raw(object, JsString::new(key).as_raw())
    }

    fn set_hidden_raw(&self, object: Object<'_>, key: sys::JSStringRef, value: Value<'_>) -> JsResult<()> {
        let f = sys::spi::get().set_private_property.ok_or(JsError::Unsupported("JSObjectSetPrivateProperty"))?;
        let ok = unsafe { f(self.ctx, object.raw, key, value.raw) };
        if ok {
            Ok(())
        } else {
            Err(JsError::Type("hidden properties require a host object".into()))
        }
    }

    fn get_hidden_raw(&self, object: Object<'_>, key: sys::JSStringRef) -> JsResult<Option<Value<'s>>> {
        let f = sys::spi::get().get_private_property.ok_or(JsError::Unsupported("JSObjectGetPrivateProperty"))?;
        let v = unsafe { f(self.ctx, object.raw, key) };
        Ok((!v.is_null()).then(|| unsafe { Value::from_raw(self.ctx, v) }))
    }

    /// Reports an uncaught exception: first to the realm's
    /// [`ExceptionReporter`] (a Window fires its `error` event, which may
    /// run script), then, unless that handled it, to the engine hooks
    /// (console/CDP). Termination is not an exception and is not reported.
    pub fn report_exception(&self, error: &JsError) {
        if matches!(error, JsError::Terminated) {
            return;
        }
        let hooks = self.realm.engine.hooks();
        let reporter = self.realm.exception_reporter.get();
        if hooks.is_none() && reporter.is_none() {
            return;
        }
        let details = error.details(self);
        if let Some(reporter) = reporter {
            if reporter(self, &details) {
                return;
            }
        }
        if let Some(h) = hooks {
            h.exception_thrown(&self.realm(), &details);
        }
    }

    /// Describes a value for remote-object protocols: `(type, subtype, className)`
    /// following CDP `Runtime.RemoteObject`.
    pub fn describe(&self, value: Value<'_>) -> JsResult<ValueDescription> {
        let packed = self.realm.cached_helper(self, "describePacked")?.call(None, &[value])?.to_string()?;
        let mut parts = packed.splitn(4, '\0');
        let mut next = || parts.next().filter(|s| !s.is_empty()).map(str::to_owned);
        Ok(ValueDescription {
            ty: next().unwrap_or_default(),
            subtype: next(),
            class_name: next(),
            description: next(),
        })
    }

    /// Own property names including non-enumerable and symbols, as an array of
    /// `{name, symbol?, value?, get?, set?, writable, enumerable, configurable, isOwn}`.
    pub fn own_properties(&self, object: Object<'_>, own_only: bool) -> JsResult<Object<'s>> {
        let r = self.realm.helper(self, "properties")?.call(None, &[object.value(), self.boolean(own_only)])?;
        r.as_object().ok_or_else(|| JsError::Type("properties helper failed".into()))
    }

    /// CDP `Runtime.ObjectPreview` of an object as JSON text. `description`
    /// replaces the computed one; `promise` is a `[state, result]` array
    /// read through [`Scope::promise_probe`].
    pub fn object_preview(
        &self,
        value: Value<'_>,
        description: Option<&str>,
        promise: Option<Value<'_>>,
    ) -> JsResult<String> {
        let description = description.map_or_else(|| self.undefined(), |d| self.string(d));
        let promise = promise.unwrap_or_else(|| self.undefined());
        self.realm.cached_helper(self, "preview")?.call(None, &[value, description, promise])?.to_string()
    }

    /// CDP `internalProperties` of an object as an array of `[name, value]`
    /// pairs: `[[Prototype]]` (when asked), `[[PrimitiveValue]]` of
    /// primitive wrappers and `[[Entries]]` of a Map or Set.
    pub fn internal_properties(&self, object: Object<'_>, with_prototype: bool) -> JsResult<Object<'s>> {
        let r = self.realm.helper(self, "internals")?.call(None, &[object.value(), self.boolean(with_prototype)])?;
        r.as_object().ok_or_else(|| JsError::Type("internals helper failed".into()))
    }

    /// For a promise, attaches reactions recording its settlement and
    /// returns a function reading `[state, result]`. A settled promise's
    /// reaction runs once the outermost native call into the engine returns
    /// (JavaScriptCore drains microtasks then), so read it after that.
    pub fn promise_probe(&self, value: Value<'_>) -> JsResult<Option<Object<'s>>> {
        let r = self.realm.helper(self, "promiseProbe")?.call(None, &[value])?;
        Ok(r.as_object())
    }

    /// The JavaScript stack at this point, in JavaScriptCore's format (one
    /// `function@url:line:column` per line, innermost first, the helper's
    /// own frame included).
    pub fn current_stack(&self) -> JsResult<String> {
        self.realm.helper(self, "stack")?.call(None, &[])?.to_string()
    }
}

/// Borrow of a wrapper payload that keeps the wrapper GC-protected.
pub struct Unwrapped<'s, T> {
    data: &'s T,
    obj: sys::JSObjectRef,
    root: sys::JSContextRef,
}

impl<T> std::ops::Deref for Unwrapped<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> Drop for Unwrapped<'_, T> {
    fn drop(&mut self) {
        if !self.obj.is_null() {
            // SAFETY: protected in Scope::unwrap; the engine outlives 's.
            unsafe { sys::JSValueUnprotect(self.root, self.obj as sys::JSValueRef) };
        }
    }
}

/// CDP-style classification of a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDescription {
    pub ty: String,
    pub subtype: Option<String>,
    pub class_name: Option<String>,
    pub description: Option<String>,
}

/// A promise plus its resolving functions, all rooted.
pub struct Deferred {
    pub promise: Persistent,
    resolve: Persistent,
    reject: Persistent,
}

impl Deferred {
    pub fn resolve(&self, scope: &Scope<'_>, value: Value<'_>) -> JsResult<()> {
        self.resolve.get(scope).as_object().expect("resolve").call(None, &[value]).map(|_| ())
    }
    pub fn reject(&self, scope: &Scope<'_>, value: Value<'_>) -> JsResult<()> {
        self.reject.get(scope).as_object().expect("reject").call(None, &[value]).map(|_| ())
    }
}

type ClosureData = Box<dyn for<'a> Fn(&Scope<'a>, Value<'a>, Args<'a>) -> JsResult<Value<'a>>>;

pub(crate) unsafe extern "C" fn closure_call(
    ctx: sys::JSContextRef,
    function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        // The closure's own realm (its payload's), which JSC calls it in.
        class::guard_in(ctx, class::wrapper_realm(function), exc, ptr::null(), |scope| {
            let h = sys::JSObjectGetPrivate(function) as *mut Header;
            let f =
                if h.is_null() || (*h).kind != Kind::Closure { None } else { class::payload_data::<ClosureData>(h) };
            let f = f.ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
            let this_v = if this.is_null() { scope.undefined() } else { Value::from_raw(ctx, this as sys::JSValueRef) };
            let args = Args::from_raw(ctx, argc, argv);
            f(scope, this_v, args).map(|v| v.raw)
        })
    }
}

// ---------------------------------------------------------------------------
// Realm creation

const HELPERS_JS: &str = include_str!("helpers.js");

pub(crate) fn create(engine: &Rc<EngineInner>, options: RealmOptions) -> JsResult<Realm> {
    if engine.is_shut_down() {
        return Err(JsError::ContextDestroyed);
    }
    if let Some(id) = options.global_class {
        if id as usize >= engine.classes.borrow().classes.len() {
            return Err(JsError::Type("unknown global class".into()));
        }
    }
    // Always the default global implementation (JSAPIGlobalObject): only it
    // supports the module loader. Its [[Prototype]] is immutable, so a host
    // global class is emulated by copying its members onto the global
    // (WebIDL puts [Global] interface members on the instance anyway).
    let ctx = unsafe { sys::JSGlobalContextCreateInGroup(engine.group(), ptr::null()) };
    // A window's chain gets a named properties object, as in the standard: own
    // members and `var`s are found (and cached) first, and only a name the global
    // lacks reaches it. The public API cannot change the global's prototype.
    if options.global_class.is_some() {
        if let Some(install) = sys::spi::get().install_named_properties {
            // SAFETY: fresh context; nothing holds its global proxy yet.
            unsafe { install(ctx, crate::class::named_properties_filter, Some(crate::class::named_properties_get)) };
        }
    }
    let id = RealmId(engine.next_realm_id.get());
    engine.next_realm_id.set(id.0 + 1);
    let inner = Rc::new(RealmInner {
        id,
        name: options.name,
        origin: RefCell::new(options.origin),
        engine: engine.clone(),
        ctx: Cell::new(ctx),
        protos: RefCell::new(Vec::new()),
        ctors: RefCell::new(Vec::new()),
        targets: RefCell::new(Vec::new()),
        populated: RefCell::new(Vec::new()),
        identity: RefCell::new(rustc_hash::FxHashMap::default()),
        shared_strings: RefCell::new(rustc_hash::FxHashMap::default()),
        owned_head: Cell::new(ptr::null_mut()),
        helpers: RefCell::new(None),
        helper_cache: RefCell::new(rustc_hash::FxHashMap::default()),
        object_proto: RefCell::new(None),
        function_proto: RefCell::new(None),
        fn_cache: RefCell::new(HashMap::new()),
        dom_exception: RefCell::new(None),
        module_loader: Cell::new(ptr::null_mut()),
        module_fetcher: RefCell::new(None),
        native_callback_hook: RefCell::new(None),
        unhandled_rejection_handler: Cell::new(None),
        exception_reporter: Cell::new(None),
        native_callback_pending: RefCell::new(None),
        native_callback_hook_active: Cell::new(false),
        inline_modules: RefCell::new(HashMap::new()),
        embedder: RefCell::new(None),
        named_properties: RefCell::new(None),
        global_class: options.global_class,
        global_payload: Cell::new(ptr::null_mut()),
        // SAFETY: fresh, retained global context.
        global_object: Cell::new(unsafe { sys::JSContextGetGlobalObject(ctx) }),
        global_target: Cell::new(ptr::null_mut()),
    });
    if let Some(target) = sys::spi::get().get_proxy_target {
        // SAFETY: the cached global proxy of a live context.
        let t = unsafe { target(inner.global_object.get()) };
        inner.global_target.set(t as sys::JSObjectRef);
    }
    engine.realms.borrow_mut().insert(ctx as usize, Rc::downgrade(&inner));
    let realm = Realm { inner };
    {
        let _watchdog = crate::engine::WatchdogPause::new(engine);
        let scope = realm.scope()?;
        let helpers = scope.eval(HELPERS_JS, Some("lightpanda://internal/helpers.js"), 1)?;
        *realm.inner.helpers.borrow_mut() = Some(Persistent::new(&scope, helpers));
        let dom_exc = helpers.as_object().expect("helpers").get("DOMException")?;
        *realm.inner.dom_exception.borrow_mut() = Some(Persistent::new(&scope, dom_exc));
        // Before any native function is made (interface members included).
        let function = scope.global().get_atom("Function")?.as_object().expect("Function");
        *realm.inner.function_proto.borrow_mut() = Some(Persistent::new(&scope, function.get_atom("prototype")?));
        install_classes(&scope)?;
        let object_proto = scope.global().get("Object")?.as_object().expect("Object").get("prototype")?;
        *realm.inner.object_proto.borrow_mut() = Some(Persistent::new(&scope, object_proto));
        install_console(&scope)?;
        install_rejection_tracking(&scope)?;
        if engine.caps.modules {
            let h = unsafe {
                sys::modules::lp_jsc_module_loader_install(ctx, module_fetch, module_base_url, ptr::null_mut())
            };
            realm.inner.module_loader.set(h);
        }
    }
    if let Some(h) = engine.hooks() {
        h.realm_created(&realm);
    }
    Ok(realm)
}

/// Creates every interface object and prototype of the realm, with their
/// inheritance, eagerly. Prototype/static members are installed on first use
/// of the class: a wrap of it or a subclass, or any property operation on its
/// interface object (a Proxy over the constructor whose traps install the
/// members and then forward). Interface objects stay data properties of the
/// global, so descriptors and identities match eager installation. The global
/// class chain is populated eagerly (its members are copied onto the global).
fn install_classes(scope: &Scope<'_>) -> JsResult<()> {
    let realm = scope.realm;
    let reg = realm.engine.classes.borrow();
    let global = scope.global();
    let object_proto = global.get("Object")?.as_object().expect("Object").get("prototype")?;
    let install = realm.helper(scope, "installInterfaces")?;
    let populate = make_native_fn(scope, "populate", populate_tramp);
    let keys = MemberKeys::new(Some(realm));
    let n = reg.classes.len();
    // One call into JavaScript for all classes: per-class calls cost a VM
    // entry each, which dominated realm creation.
    let mut spec = String::from("[");
    let mut natives = crate::RootedVec::new();
    for (i, info) in reg.classes.iter().enumerate() {
        if i > 0 {
            spec.push(',');
        }
        let name = serde_json::to_string(info.name).expect("class name serializes");
        let parent = info.parent.map_or(-1, i64::from);
        spec.push_str(&format!(
            "[{name},{parent},{},{},{}]",
            info.def.ctor_length, info.def.exposed, info.def.callback_interface
        ));
        let native = match info.def.constructor {
            Some(c) => {
                make_member_fn(scope.ctx, &realm.engine, reg.member, c, false, info.name, info.def.ctor_length, &keys)?
                    .value()
            }
            None => scope.null(),
        };
        natives.push(scope, native);
    }
    spec.push(']');
    let spec = scope.parse_json(&spec).expect("class spec is JSON");
    let natives: Vec<Value<'_>> = natives.iter(scope).collect();
    let natives = scope.array(&natives)?;
    let result = install.call(None, &[spec, natives.value(), object_proto, global.value(), populate.value()])?;
    let result = result.as_object().expect("installInterfaces result");
    let lists: Vec<Object<'_>> = (0..3)
        .map(|k| result.get_index(k).map(|v| v.as_object().expect("installInterfaces list")))
        .collect::<JsResult<_>>()?;
    let column = |list: &Object<'_>| -> JsResult<Vec<Option<Persistent>>> {
        (0..n as u32).map(|i| Ok(Some(Persistent::new(scope, list.get_index(i)?)))).collect()
    };
    let protos = column(&lists[0])?;
    let targets = column(&lists[1])?;
    let ctors = column(&lists[2])?;
    // [LegacyFactoryFunction]s of exposed classes: global constructors over
    // the class's prototype (`Image`), lazily populated like the interface.
    let factories: Vec<(ClassId, &'static str, u32, class::MemberCall)> = reg
        .classes
        .iter()
        .enumerate()
        .filter(|(_, info)| info.def.exposed)
        .flat_map(|(i, info)| info.def.legacy_factories.iter().map(move |f| (i as ClassId, f.name, f.length, f.call)))
        .collect();
    if !factories.is_empty() {
        let legacy_factory = realm.helper(scope, "legacyFactory")?;
        for (id, name, length, call) in factories {
            let native = make_member_fn(scope.ctx, &realm.engine, reg.member, call, false, name, length, &keys)?;
            let proto = lists[0].get_index(id)?;
            legacy_factory.call(
                None,
                &[
                    global.value(),
                    scope.string(name),
                    native.value(),
                    proto,
                    scope.number(length as f64),
                    scope.number(id as f64),
                    populate.value(),
                ],
            )?;
        }
    }
    let global_chain = realm.global_class.map(|gid| {
        let mut chain = vec![gid];
        while let Some(p) = reg.classes[*chain.last().expect("non-empty") as usize].parent {
            chain.push(p);
        }
        chain.reverse();
        chain
    });
    let global_name = realm.global_class.map(|gid| reg.classes[gid as usize].name);
    drop(reg);
    *realm.protos.borrow_mut() = protos;
    *realm.ctors.borrow_mut() = ctors;
    *realm.targets.borrow_mut() = targets;
    *realm.populated.borrow_mut() = vec![false; n];
    if let (Some(chain), Some(name)) = (global_chain, global_name) {
        // Chain root-first: copy members onto the global, brand instanceof.
        populate_class(scope, *chain.last().expect("non-empty"))?;
        let install_global = realm.helper(scope, "installGlobal")?;
        let mut ps = crate::RootedVec::new();
        let mut cs = crate::RootedVec::new();
        for c in &chain {
            ps.push(scope, realm.protos.borrow()[*c as usize].as_ref().expect("proto").get(scope));
            cs.push(scope, realm.ctors.borrow()[*c as usize].as_ref().expect("ctor").get(scope));
        }
        let ps = rooted_array(scope, &ps)?;
        let cs = rooted_array(scope, &cs)?;
        install_global.call(None, &[global.value(), ps.value(), cs.value(), scope.string(name)])?;
    }
    Ok(())
}

fn rooted_array<'s>(scope: &Scope<'s>, items: &crate::RootedVec) -> JsResult<Object<'s>> {
    let arr = scope.array(&[])?;
    for (i, v) in items.iter(scope).enumerate() {
        arr.set_index(i as u32, v)?;
    }
    Ok(arr)
}

/// `populate(id)` for the interface proxies. Installing into a disposed realm
/// is impossible: throw the same error as every other use of it.
unsafe extern "C" fn populate_tramp(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    let undefined = unsafe { sys::JSValueMakeUndefined(ctx) };
    if realm_for_ctx(ctx).is_none() {
        // Disposed realm: install from the state its helper object keeps.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _depth = class::DepthGuard::enter();
            let args = unsafe { Args::from_raw(ctx, argc, argv) };
            let id = args.get(0).to_number()? as ClassId;
            let h = args.get(1).as_object().ok_or(JsError::ContextDestroyed)?;
            populate_disposed(ctx, id, h)
        }));
        return match outcome {
            Ok(Ok(())) => undefined,
            Ok(Err(JsError::Thrown(p))) => {
                if !exc.is_null() {
                    unsafe { *exc = p.as_raw() };
                }
                ptr::null()
            }
            Ok(Err(e)) => {
                unsafe { crate::error::throw_message(ctx, &e.to_string(), exc) };
                ptr::null()
            }
            Err(_) => {
                unsafe { crate::error::throw_message(ctx, "Internal error: panic in populate", exc) };
                ptr::null()
            }
        };
    }
    unsafe {
        class::guard(ctx, exc, ptr::null(), |s| {
            let id = Args::from_raw(ctx, argc, argv).get(0).to_number()?;
            populate_class(s, id as ClassId)?;
            Ok(undefined)
        })
    }
}

/// Installs the prototype/static members of `id` and its ancestors in this
/// realm, once. No RefCell borrow is held across calls into JavaScript.
pub(crate) fn populate_class(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let realm = scope.realm;
    // Root-first chain of classes still to populate.
    let chain = {
        let reg = realm.engine.classes.borrow();
        let populated = realm.populated.borrow();
        if populated.get(id as usize).copied().unwrap_or(true) {
            return Ok(()); // done, or unknown/disposed (callers report those)
        }
        let mut chain = Vec::new();
        let mut c = Some(id);
        while let Some(cur) = c {
            if populated.get(cur as usize).copied().unwrap_or(true) {
                break;
            }
            chain.push(cur);
            c = reg.classes[cur as usize].parent;
        }
        chain.reverse();
        chain
    };
    for c in chain {
        // Marked first: member installation does not re-enter population
        // (it defines on the prototype and the proxy target directly).
        realm.populated.borrow_mut()[c as usize] = true;
        if let Err(e) = install_members(scope, c).and_then(|()| mirror_populated(scope, c)) {
            realm.populated.borrow_mut()[c as usize] = false;
            return Err(e);
        }
    }
    Ok(())
}

/// Mirrors the populated flag into the helper object, which is what
/// population after disposal consults (`populate_disposed`).
fn mirror_populated(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let helpers = scope.realm.helpers.borrow().as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let flags = helpers.as_object().expect("helpers").get("populated")?.as_object().expect("populated array");
    flags.set_index(id, scope.boolean(true))
}

fn install_members(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let realm = scope.realm;
    let proto = realm.protos.borrow()[id as usize].as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let target = realm.targets.borrow()[id as usize].as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let helpers = realm.helpers.borrow().as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let (Some(proto), Some(target), Some(helpers)) = (proto.as_object(), target.as_object(), helpers.as_object())
    else {
        return Err(JsError::Type("invalid interface state".into()));
    };
    install_members_raw(scope.ctx, &realm.engine, Some(realm), id, proto, target, helpers)
}

/// Installs `id`'s members onto `proto`/`target` using only `ctx` and the
/// realm's helper object, so it also works for a disposed realm (whose
/// interface objects are still referenced from another realm).
/// A callable object for a member: `member_call` reads the boxed
/// `MemberCall` from its private data; `member_finalize` frees it. `length`
/// and `name` are defined like a built-in function's (non-writable,
/// non-enumerable, configurable), in that order.
fn make_member_fn(
    ctx: sys::JSContextRef,
    engine: &EngineInner,
    class: sys::JSClassRef,
    call: class::MemberCall,
    pure: bool,
    name: &str,
    length: u32,
    keys: &MemberKeys,
) -> JsResult<Object<'static>> {
    // The patched JSC makes an internal function that call inline caches link
    // to directly; a C API callback object is called through JSC's host-call
    // slow path every time. The payload has no finalizer there: the registry
    // keeps it for the engine's lifetime.
    if let Some(make) = sys::spi::get().make_function_with_data {
        let realm = keys.realm.clone();
        let mut payload = Box::new(class::MemberPayload { tag: class::PayloadTag::Member, call, pure, realm });
        let data = &mut *payload as *mut class::MemberPayload as *mut c_void;
        engine.member_payloads.borrow_mut().push(payload);
        let n = JsString::new(name);
        let f = unsafe { make(ctx, n.as_raw(), length, Some(class::member_call_data), data) };
        if f.is_null() {
            return Err(JsError::Type("cannot create function".into()));
        }
        return Ok(unsafe { Object::from_raw(ctx, f) });
    }
    // No realm: this payload is freed by `member_finalize` (see `MemberPayload::realm`).
    let payload = class::MemberPayload { tag: class::PayloadTag::Member, call, pure, realm: None };
    let payload = Box::into_raw(Box::new(payload)) as *mut c_void;
    let f = unsafe { Object::from_raw(ctx, sys::JSObjectMake(ctx, class, payload)) };
    let attrs = sys::kJSPropertyAttributeReadOnly | sys::kJSPropertyAttributeDontEnum;
    unsafe {
        let mut exc = ptr::null();
        let len = sys::JSValueMakeNumber(ctx, length as f64);
        sys::JSObjectSetProperty(ctx, f.raw, keys.length.as_raw(), len, attrs, &mut exc);
        check(ctx, exc)?;
        let v = JsString::new(name);
        let nm = sys::JSValueMakeString(ctx, v.as_raw());
        sys::JSObjectSetProperty(ctx, f.raw, keys.name.as_raw(), nm, attrs, &mut exc);
        check(ctx, exc)?;
    }
    Ok(f)
}

/// What every member function of one installation pass shares: the
/// `length` and `name` keys, and the realm the functions belong to (`None`
/// for a disposed realm).
struct MemberKeys {
    length: JsString,
    name: JsString,
    realm: Option<Weak<RealmInner>>,
}

impl MemberKeys {
    fn new(realm: Option<&Rc<RealmInner>>) -> MemberKeys {
        MemberKeys { length: JsString::new("length"), name: JsString::new("name"), realm: realm.map(Rc::downgrade) }
    }
}

fn install_members_raw(
    ctx: sys::JSContextRef,
    engine: &EngineInner,
    realm: Option<&Rc<RealmInner>>,
    id: ClassId,
    proto: Object<'_>,
    target: Object<'_>,
    helpers: Object<'_>,
) -> JsResult<()> {
    // Copied out so no registry borrow spans calls into JavaScript.
    let (methods, accessors, constants, statics, iterable) = {
        let reg = engine.classes.borrow();
        let d = &reg.classes.get(id as usize).ok_or(JsError::Type("unknown class".into()))?.def;
        (
            d.methods.iter().map(|m| (m.name, m.length, m.call, m.pure)).collect::<Vec<_>>(),
            d.accessors.iter().map(|a| (a.name, a.get, a.set, a.slot)).collect::<Vec<_>>(),
            d.constants.clone(),
            d.statics.iter().map(|m| (m.name, m.length, m.call)).collect::<Vec<_>>(),
            if d.indexed_iterable {
                Some("iterable")
            } else if d.indexed_iterator {
                Some("iterator")
            } else {
                None
            },
        )
    };
    // SAFETY (all raw constructors below): ctx is the live callback/scope context.
    let num = |n: f64| unsafe { Value::from_raw(ctx, sys::JSValueMakeNumber(ctx, n)) };
    let string = |t: &str| unsafe { Value::from_raw(ctx, sys::JSValueMakeString(ctx, JsString::new(t).as_raw())) };
    let undefined = unsafe { Value::from_raw(ctx, sys::JSValueMakeUndefined(ctx)) };
    let helper = |name: &str| -> JsResult<Object<'_>> {
        helpers.get(name)?.as_object().ok_or_else(|| JsError::Type("missing helper".into()))
    };
    // A live realm's own Function.prototype; a disposed one's global's.
    let cached = realm.and_then(|r| r.function_proto.borrow().as_ref().map(Persistent::as_raw));
    let function_proto = match cached {
        // SAFETY: protected by the realm until its disposal.
        Some(p) => Some(unsafe { Value::from_raw(ctx, p) }),
        None => unsafe {
            let global = Object::from_raw(ctx, sys::JSContextGetGlobalObject(ctx));
            global.get("Function")?.as_object().map(|f| f.get("prototype")).transpose()?
        },
    };
    let member_class = engine.classes.borrow().member;
    let keys = MemberKeys::new(realm);
    let native_pure = |name: &str, length: u32, call: class::MemberCall, pure: bool| -> JsResult<Object<'_>> {
        let f = make_member_fn(ctx, engine, member_class, call, pure, name, length, &keys)?;
        if let Some(p) = function_proto {
            f.set_prototype(p);
        }
        Ok(f)
    };
    let native = |name: &str, length: u32, call: class::MemberCall| native_pure(name, length, call, false);
    for (name, length, call, pure) in methods {
        let f = native_pure(name, length, call, pure)?;
        proto.set(name, f.value())?;
    }
    let accessor = helper("accessor")?;
    let custom = sys::spi::get().set_custom_accessor;
    for (name, get, set, slot) in accessors {
        if let (Some(install), Some(slot)) = (custom, slot) {
            // A custom accessor: inline caches call the getter directly
            // (`crate::accessors`); enumerable and configurable like the
            // ordinary form below.
            let n = JsString::new(name);
            let setter = set.map(|_| crate::accessors::SETTERS[usize::from(slot)]);
            let getter = Some(crate::accessors::GETTERS[usize::from(slot)]);
            // A pure getter is a DOM attribute to the JITs, which then call
            // it with the receiver proven a wrapper (`fast_getter_dispatch`).
            let ok = match (crate::accessors::pure_result(slot), sys::spi::get().set_fast_getter) {
                (Some(result), Some(fast)) => unsafe {
                    fast(
                        ctx,
                        proto.raw,
                        n.as_raw(),
                        getter,
                        setter,
                        sys::kJSPropertyAttributeNone,
                        Some(crate::accessors::fast_getter_dispatch),
                        usize::from(slot) as *mut c_void,
                        result,
                        true,
                    )
                },
                _ => unsafe { install(ctx, proto.raw, n.as_raw(), getter, setter, sys::kJSPropertyAttributeNone) },
            };
            if ok {
                continue;
            }
        }
        let g = native(name, 0, get)?;
        let st = match set {
            // Setters keep length 0, as JSC's callback functions had.
            Some(f) => native(name, 0, f)?.value(),
            None => undefined,
        };
        accessor.call(None, &[proto.value(), string(name), g.value(), st])?;
    }
    let constant = helper("constant")?;
    for (name, v) in constants {
        constant.call(None, &[target.value(), string(name), num(v)])?;
        constant.call(None, &[proto.value(), string(name), num(v)])?;
    }
    for (name, length, call) in statics {
        let f = native(name, length, call)?;
        target.set(name, f.value())?;
    }
    if let Some(kind) = iterable {
        helper(kind)?.call(None, &[proto.value()])?;
    }

    Ok(())
}

/// Population of a disposed realm's interface (reachable from another realm):
/// its state lives in the helper object `h` (`classProtos`, `classTargets`,
/// `populated`, the last recorded at disposal), so the result is the same API
/// eager installation exposed; calling native members then throws as before.
fn populate_disposed(ctx: sys::JSContextRef, id: ClassId, h: Object<'_>) -> JsResult<()> {
    let engine = crate::engine::engine_for_ctx(ctx).ok_or(JsError::ContextDestroyed)?;
    let protos = h.get("classProtos")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let targets = h.get("classTargets")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let populated = h.get("populated")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let chain = {
        let reg = engine.classes.borrow();
        let mut chain = Vec::new();
        let mut c = Some(id);
        while let Some(cur) = c {
            if cur as usize >= reg.classes.len() || populated.get_index(cur)?.to_bool() {
                break;
            }
            chain.push(cur);
            c = reg.classes[cur as usize].parent;
        }
        chain.reverse();
        chain
    };
    let yes = unsafe { Value::from_raw(ctx, sys::JSValueMakeBoolean(ctx, true)) };
    let no = unsafe { Value::from_raw(ctx, sys::JSValueMakeBoolean(ctx, false)) };
    for c in chain {
        let (Some(proto), Some(target)) = (protos.get_index(c)?.as_object(), targets.get_index(c)?.as_object()) else {
            return Err(JsError::ContextDestroyed);
        };
        populated.set_index(c, yes)?;
        if let Err(e) = install_members_raw(ctx, &engine, None, c, proto, target, h) {
            populated.set_index(c, no)?;
            return Err(e);
        }
    }
    Ok(())
}

fn make_native_fn<'s>(scope: &Scope<'s>, name: &str, call: sys::JSObjectCallAsFunctionCallbackRaw) -> Object<'s> {
    let n = JsString::new(name);
    unsafe { Object::from_raw(scope.ctx, sys::JSObjectMakeFunctionWithCallback(scope.ctx, n.as_raw(), Some(call))) }
}

macro_rules! console_methods {
    ($($name:literal => $level:expr),* $(,)?) => {
        fn install_console(scope: &Scope<'_>) -> JsResult<()> {
            let console = scope.object();
            $(
                {
                    unsafe extern "C" fn tramp(
                        ctx: sys::JSContextRef,
                        _f: sys::JSObjectRef,
                        _this: sys::JSObjectRef,
                        argc: usize,
                        argv: *const sys::JSValueRef,
                        _exc: *mut sys::JSValueRef,
                    ) -> sys::JSValueRef {
                        unsafe { console_call(ctx, $level, argc, argv) }
                    }
                    console.set($name, make_native_fn(scope, $name, tramp).value())?;
                }
            )*
            let attrs = sys::kJSPropertyAttributeDontEnum;
            scope.global().set_with(&JsString::new("console"), console.value(), attrs)
        }
    };
}

console_methods! {
    "log" => ConsoleLevel::Log,
    "debug" => ConsoleLevel::Debug,
    "info" => ConsoleLevel::Info,
    "warn" => ConsoleLevel::Warning,
    "error" => ConsoleLevel::Error,
    "dir" => ConsoleLevel::Dir,
    "dirxml" => ConsoleLevel::Dir,
    "table" => ConsoleLevel::Table,
    "trace" => ConsoleLevel::Trace,
    "assert" => ConsoleLevel::Assert,
    "clear" => ConsoleLevel::Clear,
    "count" => ConsoleLevel::Count,
    "timeEnd" => ConsoleLevel::TimeEnd,
    "group" => ConsoleLevel::StartGroup,
    "groupCollapsed" => ConsoleLevel::StartGroupCollapsed,
    "groupEnd" => ConsoleLevel::EndGroup,
}

unsafe fn console_call(
    ctx: sys::JSContextRef,
    level: ConsoleLevel,
    argc: usize,
    argv: *const sys::JSValueRef,
) -> sys::JSValueRef {
    let undefined = unsafe { sys::JSValueMakeUndefined(ctx) };
    unsafe {
        class::guard(ctx, ptr::null_mut(), undefined, |scope| {
            let Some(hooks) = scope.realm.engine.hooks() else { return Ok(undefined) };
            let args = Args::from_raw(ctx, argc, argv);
            let skip = if level == ConsoleLevel::Assert {
                if args.get(0).to_bool() {
                    return Ok(undefined);
                }
                1
            } else {
                0
            };
            // Arguments stay rooted by the caller's frame.
            let values: Vec<Value<'_>> = args.iter().skip(skip).collect();
            hooks.console_api(&scope.realm(), level, &values);
            Ok(undefined)
        })
    }
}

fn install_rejection_tracking(scope: &Scope<'_>) -> JsResult<()> {
    let Some(set) = sys::spi::get().set_unhandled_rejection_callback else {
        return Ok(());
    };
    let f = scope.function("unhandledRejection", |s, _this, args| {
        let canceled = if let Some(handler) = s.realm.unhandled_rejection_handler.get() {
            match handler(s, args.get(0), args.get(1)) {
                Ok(canceled) => canceled,
                Err(error) => {
                    s.report_exception(&error);
                    false
                }
            }
        } else {
            false
        };
        if !canceled {
            if let Some(h) = s.realm.engine.hooks() {
                let details = JsException::from_value(s, args.get(1));
                h.unhandled_rejection(&s.realm(), &details);
            }
        }
        Ok(s.undefined())
    })?;
    let mut exc = ptr::null();
    unsafe { set(scope.realm.global_ctx()?, f.raw, &mut exc) };
    check(scope.ctx, exc)
}

unsafe extern "C" fn module_fetch(
    _userdata: *mut c_void,
    ctx: sys::JSGlobalContextRef,
    identifier: sys::JSStringRef,
    resolve: sys::JSObjectRef,
    reject: sys::JSObjectRef,
) {
    unsafe {
        class::guard(ctx as sys::JSContextRef, ptr::null_mut(), (), |scope| {
            let realm = scope.realm;
            let key = crate::string::to_rust_string(identifier);
            let origin = realm.origin.borrow().clone();
            let url = remap_module_url(&key, origin.as_deref());
            let request = ModuleRequest {
                url: url.clone(),
                key,
                realm: Rc::downgrade(realm),
                resolve: Persistent::new(scope, Value::from_raw(scope.ctx, resolve as sys::JSValueRef)),
                reject: Persistent::new(scope, Value::from_raw(scope.ctx, reject as sys::JSValueRef)),
                done: false,
            };
            let inline = realm.inline_modules.borrow_mut().remove(&request.key);
            if let Some(src) = inline {
                return request.resolve(scope, &src);
            }
            let fetcher = realm.module_fetcher.borrow().clone();
            match fetcher {
                Some(f) => {
                    f.fetch(&scope.realm(), url, request);
                    Ok(())
                }
                None => request.reject(scope, &JsError::Type("module loading is not configured for this realm".into())),
            }
        })
    }
}

unsafe extern "C" fn module_base_url(ctx: sys::JSGlobalContextRef) -> sys::JSStringRef {
    let mut url = None;
    unsafe {
        class::guard(ctx as sys::JSContextRef, ptr::null_mut(), (), |scope| {
            let fetcher = scope.realm.module_fetcher.borrow().clone();
            url = fetcher.and_then(|f| f.document_base_url(&scope.realm()));
            Ok(())
        });
        // Retained for the caller; `JsString` releases its own reference.
        url.map_or(ptr::null_mut(), |url| sys::JSStringRetain(JsString::new(&url).as_raw()))
    }
}

/// JSC resolves root-relative specifiers ("/x.js") against `file:///`; map
/// exactly those (empty file authority) to the page origin. Other URLs,
/// including `file://host/...`, are returned unchanged.
pub(crate) fn remap_module_url(key: &str, origin: Option<&str>) -> String {
    match (origin, key.strip_prefix("file:///")) {
        (Some(origin), Some(rest)) if !origin.starts_with("file:") && origin != "null" => {
            format!("{}/{}", origin.trim_end_matches('/'), rest)
        }
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{module_source_with_url, remap_module_url};

    #[test]
    fn module_source_url_is_encoded_as_a_javascript_string() {
        let source = module_source_with_url("export {}", "https://site.test/a\"b\u{2028}.js");
        assert_eq!(source, "import.meta.url = \"https://site.test/a\\\"b\\u2028.js\";\nexport {}");
    }

    #[test]
    fn module_url_remapping_only_touches_root_relative_file_urls() {
        let o = Some("https://site.test");
        assert_eq!(remap_module_url("file:///c.js?x#y", o), "https://site.test/c.js?x#y");
        assert_eq!(remap_module_url("file://attacker.test/a.js", o), "file://attacker.test/a.js");
        assert_eq!(remap_module_url("https://cdn.test/a.js", o), "https://cdn.test/a.js");
        assert_eq!(remap_module_url("file:///c.js", Some("file://")), "file:///c.js");
        assert_eq!(remap_module_url("file:///c.js", Some("null")), "file:///c.js");
        assert_eq!(remap_module_url("file:///c.js", None), "file:///c.js");
    }
}
