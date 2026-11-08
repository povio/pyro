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

/// The `type` import attribute of a module request (HTML "module type").
/// Engines without import-attribute support report every request as
/// [`ModuleType::JavaScript`] ([`crate::Capabilities::typed_modules`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleType {
    /// No `type` attribute: a JavaScript module script.
    JavaScript,
    /// `with { type: "json" }`: resolve with [`ModuleRequest::resolve_json`].
    Json,
    /// `with { type: "css" }`: resolve with [`ModuleRequest::resolve_object`]
    /// (a `CSSStyleSheet`).
    Css,
    /// A type JavaScriptCore parses but HTML does not allow here
    /// (`"webassembly"`): reject it with a TypeError.
    Unsupported,
}

impl ModuleType {
    fn from_raw(raw: std::os::raw::c_int) -> ModuleType {
        match raw {
            sys::modules::MODULE_TYPE_JAVASCRIPT => ModuleType::JavaScript,
            sys::modules::MODULE_TYPE_JSON => ModuleType::Json,
            sys::modules::MODULE_TYPE_CSS => ModuleType::Css,
            _ => ModuleType::Unsupported,
        }
    }
}

/// A pending module fetch; complete exactly once (drop = reject).
pub struct ModuleRequest {
    url: String,
    ty: ModuleType,
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

    /// The request's `type` import attribute.
    pub fn module_type(&self) -> ModuleType {
        self.ty
    }

    /// Provides module source text (a [`ModuleType::JavaScript`] request).
    pub fn resolve(self, scope: &Scope<'_>, source: &str) -> JsResult<()> {
        let source = module_source_with_url(source, &self.key);
        self.resolve_source(scope, &source, sys::modules::MODULE_TYPE_JAVASCRIPT)
    }

    /// Provides the text of a [`ModuleType::Json`] request; the module's
    /// `default` export is `JSON.parse(text)`, and text that is not JSON
    /// fails the import with a SyntaxError.
    pub fn resolve_json(self, scope: &Scope<'_>, text: &str) -> JsResult<()> {
        self.resolve_source(scope, text, sys::modules::MODULE_TYPE_JSON)
    }

    /// Provides the `default` export of a [`ModuleType::Css`] request (the
    /// module's `CSSStyleSheet`).
    pub fn resolve_object(mut self, scope: &Scope<'_>, value: Object<'_>) -> JsResult<()> {
        self.done = true;
        self.resolve.get(scope).as_object().expect("resolve fn").call(None, &[value.value()]).map(|_| ())
    }

    fn resolve_source(mut self, scope: &Scope<'_>, source: &str, module_type: std::os::raw::c_int) -> JsResult<()> {
        self.done = true;
        let mut err: sys::JSStringRef = ptr::null();
        let script = unsafe {
            sys::modules::lp_jsc_module_script_create(
                scope.realm.global_ctx()?,
                source.as_ptr().cast(),
                source.len(),
                self.key.as_ptr().cast(),
                self.key.len(),
                module_type,
                &mut err,
            )
        };
        if script.is_null() {
            let msg = if err.is_null() {
                "module creation failed".to_string()
            } else {
                unsafe { JsString::from_raw_owned(err) }.to_string()
            };
            let e = JsError::Type(msg.into()).to_value(scope);
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
        let e = JsError::Type(format!("Failed to fetch dynamically imported module: {}", self.url).into()).to_value(&scope);
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

/// How a new wrapper gets its prototype (`Scope::make_wrapper`).
#[derive(Clone, Copy)]
enum WrapperProto<'a> {
    /// Its class's shape in the realm (the class's own prototype).
    Shape(sys::spi::JSWrapperShapeRef),
    /// Any prototype, set after the object is made.
    Value(Value<'a>),
}

/// The `RealmInner::shapes` entry of a class whose wrappers are made without
/// a shape (no patched JSC, or it made none): the shape is not asked for
/// again. No shape has this address.
const NO_SHAPE: sys::spi::JSWrapperShapeRef = ptr::dangling_mut::<c_void>();

/// Embedder step of [`Scope::report_exception`] (HTML "report an exception"
/// fires an `error` event at the Window); true means the exception was
/// handled and the engine hooks are not told.
pub type ExceptionReporter = for<'s> fn(&Scope<'s>, &JsException) -> bool;

/// The embedder's setup of a deferred realm ([`Engine::defer_realm_in_current_thread`]),
/// run right after the realm's context and interfaces are made, when it is
/// first entered.
pub type DeferredSetup = Box<dyn FnOnce(&Realm) -> JsResult<()>>;

/// What the lean member dispatch needs of a realm, behind one pointer that
/// every wrapper payload of the realm holds ([`Header::core`]): the context
/// (null once the realm is disposed) and the realm. It outlives the realm
/// (the payloads share it), so a payload whose realm is gone still reads a
/// null context here instead of a freed realm.
pub(crate) struct RealmCore {
    ctx: Cell<sys::JSContextRef>,
    /// The realm's own `Rc`, aliased without a count of its own (never
    /// dropped): handed out as `&Rc<RealmInner>` only while `ctx` is set,
    /// i.e. before `RealmInner::dispose`, which every drop of the realm runs
    /// first.
    realm: std::cell::OnceCell<std::mem::ManuallyDrop<Rc<RealmInner>>>,
}

impl RealmCore {
    fn new() -> RealmCore {
        RealmCore { ctx: Cell::new(ptr::null()), realm: std::cell::OnceCell::new() }
    }

    /// The live realm's context and the realm, with no reference count
    /// changed. The realm stays alive for as long as a lean callback uses it.
    ///
    /// Invariant: lean callbacks never run page script, so nothing can drop
    /// a realm handle while one holds this borrow. `pure_member_call` keeps it
    /// by sending calls with object arguments (whose conversion runs script)
    /// to the full dispatch; debug builds check it (`crate::class::LeanCall`).
    #[inline]
    pub(crate) fn live(&self) -> Option<(sys::JSContextRef, &Rc<RealmInner>)> {
        let ctx = self.ctx.get();
        if ctx.is_null() {
            return None;
        }
        Some((ctx, self.realm.get()?))
    }
}

pub(crate) struct RealmInner {
    pub(crate) id: RealmId,
    name: String,
    origin: RefCell<Option<String>>,
    pub(crate) engine: Rc<EngineInner>,
    /// Null while the realm is deferred (`deferred` holds its setup) and
    /// once it is disposed.
    pub(crate) ctx: Cell<sys::JSGlobalContextRef>,
    /// A realm made with its id, name and origin but without a context yet:
    /// the context, interfaces and this setup are made by the first
    /// [`Realm::scope`]. Playwright's utility world is created for every
    /// document and entered only by its actions, and a realm costs about
    /// 1.3 ms of CPU.
    deferred: RefCell<Option<DeferredSetup>>,
    /// The lean dispatch's view of this realm (see [`RealmCore`]).
    pub(crate) core: Rc<RealmCore>,
    /// Interface prototype objects by class id, null until the class is
    /// first populated (`materialize`) and past the classes this realm was
    /// created with. Kept alive by `interfaces[0]`, which only `materialize`
    /// writes, so a wrap reads its prototype without a lookup.
    protos: RefCell<Vec<sys::JSValueRef>>,
    /// The structure of each class's wrappers with its prototype of this
    /// realm (patched JSC, `JSWrapperShapeCreate`), by class id: made with
    /// the first wrapper, which every later one then starts with (null: not
    /// made yet; [`NO_SHAPE`]: none). Released by `dispose`.
    shapes: RefCell<Vec<sys::spi::JSWrapperShapeRef>>,
    /// The arrays `installInterfaces` returned, protected for the realm's
    /// life: prototypes, the constructor functions behind the interface
    /// proxies, and the interface objects as seen by script (the proxies).
    /// Three protections per realm instead of three per class.
    interfaces: RefCell<Option<[Persistent; 3]>>,
    /// Whether each class's members are installed in this realm.
    populated: RefCell<Vec<bool>>,
    identity: RefCell<rustc_hash::FxHashMap<IdentityKey, sys::JSWeakRef>>,
    /// One JavaScript string per shared host string (by pointer identity),
    /// protected for the realm's life: `tagName` and friends return the same
    /// few dozen values tens of thousands of times per selector query, and a
    /// fresh JSString each time was most of their cost. The `Arc` keeps the
    /// key's address from being reused.
    pub(crate) shared_strings: RefCell<rustc_hash::FxHashMap<usize, (std::sync::Arc<str>, sys::JSValueRef)>>,
    /// One weakly held JavaScript string per interned host string (atoms:
    /// ids, class values, local names), see `crate::interned`.
    pub(crate) interned: RefCell<crate::interned::InternedStrings>,
    /// The realm's owned wrappers (`IdentityKey::OWNED`), linked through
    /// their payload headers: `dispose` releases their weak handles, which
    /// must not outlive the VM.
    pub(crate) owned_head: Cell<*mut Header>,
    /// The payloads of this realm's member functions (patched JSC only):
    /// `dispose` clears their `realm`, so they do not keep this allocation
    /// alive, and hands them to the engine, which frees them once the
    /// realm's global object is dead (`EngineInner::retire_members`).
    member_fns: RefCell<Vec<Box<class::MemberPayload>>>,
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
    /// The global has constant attributes ([`Scope::define_global_constant`]),
    /// whose values `dispose` releases.
    global_constants: Cell<bool>,
    /// What makes each lazy constant attribute's value on its first read
    /// ([`Scope::define_lazy_global_constant`]).
    lazy_constants: RefCell<Vec<(&'static str, LazyGlobalConstant)>>,
    /// The global class's own members were defined on the global itself at
    /// creation and its interface prototype object is still empty: its first
    /// population copies them there (`populate_class`).
    global_members_on_instance: Cell<bool>,
    /// `console.count` counters and `console.time` timers by label (empty,
    /// and unallocated, until script uses them).
    console_counts: RefCell<rustc_hash::FxHashMap<String, u64>>,
    console_timers: RefCell<rustc_hash::FxHashMap<String, std::time::Instant>>,
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

    /// Entry `id` of interface list `list` (0 prototypes, 1 constructor
    /// functions, 2 interface objects); an error once disposed, and for a
    /// prototype not made yet (`materialize`).
    fn interface_object<'s>(&self, scope: &Scope<'s>, list: usize, id: ClassId) -> JsResult<Value<'s>> {
        let lists = self.interfaces.borrow();
        let lists = lists.as_ref().ok_or(JsError::ContextDestroyed)?;
        if id as usize >= self.populated.borrow().len() {
            return Err(JsError::Type("class registered after this realm was created".into()));
        }
        let v = lists[list].get(scope).as_object().expect("interface list").get_index(id)?;
        if v.is_undefined() {
            return Err(JsError::Type("interface prototype read before its class was populated".into()));
        }
        Ok(v)
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
        // A lean callback on the stack may hold this realm by `live()`'s
        // uncounted borrow: page script ran inside one (debug check).
        crate::class::assert_not_in_lean_call();
        // First: no lean callback may reach the realm from here on.
        self.core.ctx.set(ptr::null());
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
        let shapes = std::mem::take(&mut *self.shapes.borrow_mut());
        if let Some(release) = sys::spi::get().wrapper_shape_release {
            for shape in shapes.into_iter().filter(|&s| !s.is_null() && s != NO_SHAPE) {
                // SAFETY: made by `JSWrapperShapeCreate` in this realm's group.
                unsafe { release(self.engine.group(), shape) };
            }
        }
        self.interfaces.borrow_mut().take();
        self.populated.borrow_mut().clear();
        self.helpers.borrow_mut().take();
        self.helper_cache.borrow_mut().clear();
        self.object_proto.borrow_mut().take();
        self.function_proto.borrow_mut().take();
        self.fn_cache.borrow_mut().clear();
        self.dom_exception.borrow_mut().take();
        let member_fns = std::mem::take(&mut *self.member_fns.borrow_mut());
        for p in &member_fns {
            // No reference into the cell is held (`member_call_data`).
            drop(p.realm.take());
        }
        // The functions may outlive the realm (script elsewhere holds one),
        // their global with them: the payloads go when it does.
        let global = match self.global_target.get() {
            // SAFETY: the context is still retained.
            g if g.is_null() => unsafe { sys::JSContextGetGlobalObject(ctx) },
            g => g,
        };
        self.engine.retire_members(global, member_fns);
        #[cfg(debug_assertions)]
        {
            let me: *const RealmInner = self;
            let payloads = self.engine.member_payloads.borrow();
            debug_assert!(
                payloads.iter().all(|p| unsafe { (*p.realm.as_ptr()).as_ref() }.is_none_or(|w| w.as_ptr() != me)),
                "every member payload of a disposed realm has its realm cleared"
            );
        }
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
        self.interned.borrow_mut().clear(self.engine.group());
        self.embedder.borrow_mut().take();
        self.named_properties.borrow_mut().take();
        if self.global_constants.replace(false) {
            if let Some(clear) = sys::spi::get().clear_constant_accessors {
                // SAFETY: the live global behind this realm's proxy, on its
                // still-retained context. Its constants (a window's document)
                // would otherwise live as long as the global object, which
                // script elsewhere may hold (a stale `contentWindow`); a lazy
                // one not read yet is never made.
                unsafe { clear(ctx, self.global_target.get()) };
            }
        }
        drop(std::mem::take(&mut *self.lazy_constants.borrow_mut()));
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
    /// The live realm `id` of this thread's engine. A deferred realm is not
    /// found until it is made (its first [`Realm::scope`]): the lookup walks
    /// the engine's contexts, and it has none yet. Callers that must reach
    /// one keep its handle (a frame's world list) instead.
    pub fn by_id(id: RealmId) -> Option<Realm> {
        let inner = crate::engine::current_inner()?.realm_by_id(id)?;
        (!inner.ctx.get().is_null()).then_some(Realm { inner })
    }

    /// Holds the execution time limit off while the embedder sets up this
    /// realm with its own code (no page script), as realm creation does: an
    /// armed limit costs every VM entry a read of the thread's CPU time.
    /// Without effect while JavaScript is on the stack.
    pub fn setup_without_watchdog(&self) -> impl Drop + '_ {
        crate::engine::WatchdogPause::new(&self.inner.engine)
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
        self.inner.ctx.get().is_null() && !self.is_deferred()
    }

    /// Whether the realm still waits for its first [`Realm::scope`] to be
    /// made ([`Engine::defer_realm_in_current_thread`]): it has run no
    /// script and holds no object.
    pub fn is_deferred(&self) -> bool {
        self.inner.deferred.borrow().is_some()
    }

    /// Enters the realm, making it first if it was deferred. Fails after
    /// disposal or once the engine owner was dropped. Everything derived
    /// from the scope borrows this handle, which keeps the VM alive.
    pub fn scope(&self) -> JsResult<Scope<'_>> {
        if self.inner.ctx.get().is_null() && self.is_deferred() {
            self.materialize_deferred()?;
        }
        let ctx = self.inner.global_ctx()?;
        if self.inner.engine.is_shut_down() {
            return Err(JsError::ContextDestroyed);
        }
        Ok(Scope { ctx: ctx as sys::JSContextRef, realm: &self.inner })
    }

    /// Makes a deferred realm: its context and interfaces, then the
    /// embedder's setup. A failure disposes it.
    #[cold]
    #[inline(never)]
    fn materialize_deferred(&self) -> JsResult<()> {
        let setup = self.inner.deferred.borrow_mut().take().ok_or(JsError::ContextDestroyed)?;
        let result = make_context(self).and_then(|()| setup(self));
        if result.is_err() {
            self.inner.dispose();
        }
        result
    }

    /// Tears the realm down now: callbacks into its functions afterwards throw
    /// "Execution context was destroyed.", as reads of its global's constant
    /// attributes do (their values are released), persistent prototypes are
    /// released and `EngineHooks::realm_destroyed` fires. Idempotent. A
    /// deferred realm is never made.
    pub fn dispose(&self) {
        drop(self.inner.deferred.borrow_mut().take());
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
            .ok_or_else(|| JsError::Type(format!("{} not registered", C::NAME).into()))?;
        if self.inner.global_class != Some(id) {
            return Err(JsError::Type(format!("realm global is not a {}", C::NAME).into()));
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

    /// This realm's original `DOMException` constructor (unaffected by
    /// page reassignment of the global).
    pub fn dom_exception(&self) -> Option<Object<'s>> {
        self.realm.dom_exception_ctor(self)
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
        let _entry = crate::engine::EntryGuard::enter(self.ctx);
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
            return Err(JsError::Unsupported(&"import maps (patched JavaScriptCore)"));
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
            return Err(JsError::Type(format!("Invalid import map: {messages}").into()));
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
            return Err(JsError::Unsupported(&"ES modules (JSScript module SPI)"));
        }
        self.realm.inline_modules.borrow_mut().insert(url.to_string(), source.to_string());
        // Module top-level code runs with this realm as the entry realm (as
        // classic scripts do), e.g. for postMessage's incumbent window.
        let _entry = crate::engine::EntryGuard::enter(self.ctx);
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
            return Err(JsError::Type(msg.into()));
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
        // A shape is asked for once the class was populated in this realm.
        let shape = self.shape_entry(id);
        if !shape.is_null() && shape != NO_SHAPE {
            return self.make_wrapper::<C>(id, data, key, WrapperProto::Shape(shape));
        }
        populate_class(self, id)?;
        let proto = self.proto(id)?;
        let made = match shape.is_null().then(|| self.make_shape(id, proto)).flatten() {
            Some(shape) => WrapperProto::Shape(shape),
            None => WrapperProto::Value(proto),
        };
        self.make_wrapper::<C>(id, data, key, made)
    }

    /// Class `id`'s entry in this realm's wrapper shapes: null until one is
    /// asked for, [`NO_SHAPE`] when there is none.
    #[inline]
    fn shape_entry(&self, id: ClassId) -> sys::spi::JSWrapperShapeRef {
        self.realm.shapes.borrow().get(id as usize).copied().unwrap_or(ptr::null_mut())
    }

    /// Makes the wrapper shape of class `id` (populated), whose prototype in
    /// this realm is `proto`, and records it, or [`NO_SHAPE`] when there is
    /// none: once per class and realm.
    #[cold]
    fn make_shape(&self, id: ClassId, proto: Value<'_>) -> Option<sys::spi::JSWrapperShapeRef> {
        let shape = self.create_shape(id, proto).unwrap_or(NO_SHAPE);
        let mut shapes = self.realm.shapes.borrow_mut();
        if shapes.len() <= id as usize {
            shapes.resize(id as usize + 1, ptr::null_mut());
        }
        shapes[id as usize] = shape;
        (shape != NO_SHAPE).then_some(shape)
    }

    /// `JSWrapperShapeCreate` for class `id` with prototype `proto` (patched
    /// JSC); `None` without it, or when JSC makes none.
    fn create_shape(&self, id: ClassId, proto: Value<'_>) -> Option<sys::spi::JSWrapperShapeRef> {
        let spi = sys::spi::get();
        let create = spi.wrapper_shape_create?;
        spi.object_make_wrapper?;
        let proto = proto.as_object()?;
        let class = self.realm.engine.classes.borrow().classes[id as usize].instance_class;
        // SAFETY: a live class and an object of this context.
        let shape = unsafe { create(self.ctx, class, proto.raw) };
        (!shape.is_null()).then_some(shape)
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
        self.make_wrapper::<C>(id, data, key, WrapperProto::Value(proto))
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
        proto: WrapperProto<'_>,
    ) -> JsResult<Object<'s>> {
        let owned = key.filter(|k| k.is_owned());
        let key = key.filter(|k| !k.is_owned());
        let spi = sys::spi::get();
        let kept_while = match owned {
            Some(_) => Some(spi.weak_create_kept_while.ok_or(JsError::Unsupported(&"JSWeakCreateKeptWhile"))?),
            None => None,
        };
        let h = class::new_payload(&self.realm.engine, id, Kind::Instance, self.realm, key, data);
        if let Some(mode) = owned {
            // Before the handle exists: the collector reads it from then on.
            // SAFETY: h is the payload just made.
            unsafe {
                (*h).keep_alive.store(u8::from(mode == IdentityKey::OWNED_KEPT), std::sync::atomic::Ordering::Relaxed)
            };
        }
        let (obj, mut weak) = match proto {
            WrapperProto::Shape(shape) => {
                // Object, prototype and handle in one call (patched JSC).
                let make = spi.object_make_wrapper.expect("shapes are made with JSObjectMakeWrapper present");
                let mut weak: sys::JSWeakRef = ptr::null();
                // SAFETY: the flag byte lives as long as the payload, which
                // outlives the handle (see below).
                let keep = if owned.is_some() { unsafe { (*h).keep_alive.as_ptr() as *const u8 } } else { ptr::null() };
                // SAFETY: a shape made in this realm.
                let obj = unsafe { make(self.ctx, shape, h as *mut c_void, keep, &mut weak) };
                if !obj.is_null() {
                    (obj, weak)
                } else {
                    // JSC refused the shape: another global object made it
                    // (never so while a realm's scopes run in its context).
                    // The old way, then.
                    match self.proto(id) {
                        // SAFETY: no object holds the payload yet.
                        Ok(proto) => (unsafe { self.make_object(id, h, proto.raw) }, ptr::null()),
                        Err(e) => {
                            // SAFETY: as above; dropped once.
                            unsafe { ((*h).drop_fn)(h) };
                            return Err(e);
                        }
                    }
                }
            }
            // SAFETY: no object holds the payload yet.
            WrapperProto::Value(proto) => (unsafe { self.make_object(id, h, proto.raw) }, ptr::null()),
        };
        let obj = unsafe { Object::from_raw(self.ctx, obj) };
        if let (Some(_), Some(kept_while)) = (owned, kept_while) {
            // SAFETY: h is the live payload of obj; its flag byte lives as
            // long as the payload, which outlives the handle (released in
            // `class::release_owned` or `RealmInner::dispose`).
            unsafe {
                if weak.is_null() {
                    weak = kept_while(self.realm.engine.group(), obj.raw, (*h).keep_alive.as_ptr());
                }
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
            let (create, release, _) = sys::spi::get().weak().ok_or(JsError::Unsupported(&"JSWeak"))?;
            let weak = unsafe { create(self.realm.engine.group(), obj.raw) };
            if let Some(old) = self.realm.identity.borrow_mut().insert(k, weak) {
                unsafe { release(self.realm.engine.group(), old) };
            }
        }
        Ok(obj)
    }

    /// A class `id` wrapper with payload `h` by `JSObjectMake`, then given
    /// its prototype `proto`. A new object's prototype runs no script and
    /// makes no Objective-C temporaries, so no autorelease pool is pushed
    /// (`Object::set_prototype` pushes one).
    ///
    /// # Safety
    /// `h` is a payload of class `id` that no object holds yet, and `proto`
    /// an object of this context.
    unsafe fn make_object(&self, id: ClassId, h: *mut Header, proto: sys::JSValueRef) -> sys::JSObjectRef {
        let class = self.realm.engine.classes.borrow().classes[id as usize].instance_class;
        unsafe {
            let obj = sys::JSObjectMake(self.ctx, class, h as *mut c_void);
            sys::JSObjectSetPrototype(self.ctx, obj, proto);
            obj
        }
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
            .ok_or(JsError::Unsupported(&"object global-context inspection"))?;
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

    /// The global object's payload header (null before `set_global_data`).
    pub(crate) fn global_payload(&self) -> *mut Header {
        self.realm.global_payload.get()
    }

    /// Makes `name` a constant attribute of the realm's global: a
    /// non-configurable, read-only accessor that yields `value` (a WebIDL
    /// `[LegacyUnforgeable] readonly attribute` whose value never changes for
    /// the global, a window's `document`). Reads make no callback, and the
    /// optimizing JIT compiles a global variable read of it to the constant.
    /// Replaces a configurable property of that name (a copied interface
    /// member). The realm keeps `value` until it is disposed; reads after
    /// that throw, as its callbacks do. Set once: false, and the property
    /// unchanged, when it is already non-configurable or the JavaScriptCore
    /// lacks the entry points (system framework, see
    /// [`crate::Capabilities::global_constants`]).
    pub fn define_global_constant(&self, name: &str, value: Value<'_>) -> bool {
        let spi = sys::spi::get();
        let (Some(set), Some(_), target) =
            (spi.set_constant_accessor, spi.clear_constant_accessors, self.realm.global_target.get())
        else {
            return false;
        };
        if target.is_null() {
            return false;
        }
        let n = JsString::new(name);
        // SAFETY: the live global behind this realm's proxy; `value` is a live
        // value of this engine's VM (a scope's value), kept alive by the
        // property until `dispose` clears it.
        let set = unsafe { set(self.ctx, target, n.as_raw(), value.raw, None, sys::kJSPropertyAttributeNone) };
        if set {
            self.realm.global_constants.set(true);
        }
        set
    }

    /// [`Scope::define_global_constant`] with the value made on first use:
    /// the property is the same non-configurable, read-only accessor from
    /// the start, its `get` one function for its whole life (a descriptor
    /// taken before the first read still applies after it, and after
    /// `dispose`), and the first read of it (a bare `name`, `globalThis.name`,
    /// the reified getter) calls `make` in this realm and keeps its value as
    /// the constant from then on. A realm whose script never reads it never
    /// pays for the value (a window's `document`: its wrapper installs the
    /// Document interfaces). An error from `make` is the read's, and the next
    /// read calls it again. Disposed before the first read, the realm never
    /// calls it: reads throw. False, as for `define_global_constant`, when
    /// the property exists or the JavaScriptCore lacks the entry points.
    pub fn define_lazy_global_constant(&self, name: &'static str, make: LazyGlobalConstant) -> bool {
        let spi = sys::spi::get();
        let (Some(set), Some(initializer), Some(_), target) = (
            spi.set_lazy_constant_accessor,
            spi.set_constant_accessor_initializer,
            spi.clear_constant_accessors,
            self.realm.global_target.get(),
        ) else {
            return false;
        };
        if target.is_null() {
            return false;
        }
        static INITIALIZER: std::sync::Once = std::sync::Once::new();
        // SAFETY: a function for the process's life.
        INITIALIZER.call_once(|| unsafe { initializer(Some(make_lazy_global_constant)) });
        // Listed first: nothing reads the property before it exists, and a
        // refused one is unlisted again.
        self.realm.lazy_constants.borrow_mut().push((name, make));
        let n = JsString::new(name);
        // SAFETY: the live global behind this realm's proxy.
        let set = unsafe { set(self.ctx, target, n.as_raw(), None, sys::kJSPropertyAttributeNone) };
        if set {
            self.realm.global_constants.set(true);
        } else {
            self.realm.lazy_constants.borrow_mut().pop();
        }
        set
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
        self.realm.interface_object(self, 2, id)?.as_object().ok_or(JsError::ContextDestroyed)
    }

    /// Calls `hook(prototype, constructor)` once the members of `C` are
    /// installed in this realm, or now if they are: the embedder's additions
    /// to an interface (`@@iterator` aliases, unscopables) without installing
    /// the interface at realm creation. One hook per class; it runs where
    /// population happens, so it uses only what it captured when made.
    pub fn after_populate<C: JsClass>(&self, hook: Object<'_>) -> JsResult<()> {
        let id = self.class_id::<C>()?;
        if self.realm.populated.borrow().get(id as usize).copied().unwrap_or(false) {
            let proto = self.realm.interface_object(self, 0, id)?;
            let target = self.realm.interface_object(self, 1, id)?;
            return hook.call(None, &[proto, target]).map(|_| ());
        }
        let hooks = self.realm.helper(self, "hooks")?;
        debug_assert!(hooks.get_index(id)?.is_undefined(), "{}: one populate hook per class", C::NAME);
        hooks.set_index(id, hook.value())
    }

    fn class_id<C: JsClass>(&self) -> JsResult<ClassId> {
        self.realm
            .engine
            .classes
            .borrow()
            .id_of::<C>()
            .ok_or_else(|| JsError::Type(format!("class {} is not registered", C::NAME).into()))
    }

    fn proto(&self, id: ClassId) -> JsResult<Value<'s>> {
        let protos = self.realm.protos.borrow();
        let p = protos
            .get(id as usize)
            .copied()
            .filter(|p| !p.is_null())
            .ok_or_else(|| JsError::Type("class registered after this realm was created".into()))?;
        // SAFETY: kept alive by the realm's protected prototype list.
        Ok(unsafe { Value::from_raw(self.ctx, p) })
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
        let f = sys::spi::get().set_private_property.ok_or(JsError::Unsupported(&"JSObjectSetPrivateProperty"))?;
        let ok = unsafe { f(self.ctx, object.raw, key, value.raw) };
        if ok {
            Ok(())
        } else {
            Err(JsError::Type("hidden properties require a host object".into()))
        }
    }

    fn get_hidden_raw(&self, object: Object<'_>, key: sys::JSStringRef) -> JsResult<Option<Value<'s>>> {
        let f = sys::spi::get().get_private_property.ok_or(JsError::Unsupported(&"JSObjectGetPrivateProperty"))?;
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

/// What makes the value of a lazy global constant
/// ([`Scope::define_lazy_global_constant`]), in the realm it belongs to.
pub type LazyGlobalConstant = Rc<dyn for<'a> Fn(&Scope<'a>) -> JsResult<Value<'a>>>;

/// The process's initializer of lazy constant accessors
/// (`JSSetConstantAccessorInitializer`): the first read of one in `ctx`'s
/// realm. JavaScriptCore keeps the value as the constant.
unsafe extern "C" fn make_lazy_global_constant(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        class::guard(ctx, exc, ptr::null(), |scope| {
            // SAFETY: the property's name, alive during the call.
            let name = JsString::from_raw_borrowed(name);
            // Cloned out: `make` may define or read other lazy constants.
            let make = {
                let table = scope.realm.lazy_constants.borrow();
                table.iter().find(|(n, _)| name.eq_str(n)).map(|(_, m)| m.clone())
            };
            // Only this realm's global holds its lazy constants.
            match make {
                Some(make) if scope.is_global_object(object) => Ok(make(scope)?.raw),
                _ => Err(JsError::ContextDestroyed),
            }
        })
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
    let realm = allocate(engine, options)?;
    make_context(&realm)?;
    Ok(realm)
}

/// A realm with its id, name and origin, whose context, interfaces and
/// `setup` are made by its first [`Realm::scope`].
pub(crate) fn create_deferred(engine: &Rc<EngineInner>, options: RealmOptions, setup: DeferredSetup) -> JsResult<Realm> {
    let realm = allocate(engine, options)?;
    *realm.inner.deferred.borrow_mut() = Some(setup);
    Ok(realm)
}

/// The realm's state without a context: its id is taken now.
fn allocate(engine: &Rc<EngineInner>, options: RealmOptions) -> JsResult<Realm> {
    if engine.is_shut_down() {
        return Err(JsError::ContextDestroyed);
    }
    if let Some(id) = options.global_class {
        if id as usize >= engine.classes.borrow().classes.len() {
            return Err(JsError::Type("unknown global class".into()));
        }
    }
    let id = RealmId(engine.next_realm_id.get());
    engine.next_realm_id.set(id.0 + 1);
    let inner = Rc::new(RealmInner {
        id,
        name: options.name,
        origin: RefCell::new(options.origin),
        engine: engine.clone(),
        ctx: Cell::new(ptr::null_mut()),
        deferred: RefCell::new(None),
        core: Rc::new(RealmCore::new()),
        protos: RefCell::new(Vec::new()),
        shapes: RefCell::new(Vec::new()),
        interfaces: RefCell::new(None),
        populated: RefCell::new(Vec::new()),
        identity: RefCell::new(rustc_hash::FxHashMap::default()),
        shared_strings: RefCell::new(rustc_hash::FxHashMap::default()),
        interned: RefCell::new(crate::interned::InternedStrings::new()),
        owned_head: Cell::new(ptr::null_mut()),
        member_fns: RefCell::new(Vec::new()),
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
        global_object: Cell::new(ptr::null_mut()),
        global_target: Cell::new(ptr::null_mut()),
        global_constants: Cell::new(false),
        lazy_constants: RefCell::new(Vec::new()),
        global_members_on_instance: Cell::new(false),
        console_counts: RefCell::new(Default::default()),
        console_timers: RefCell::new(Default::default()),
    });
    Ok(Realm { inner })
}

/// Makes the context of an allocated realm and installs the interfaces,
/// console and module loader; `EngineHooks::realm_created` fires.
fn make_context(realm: &Realm) -> JsResult<()> {
    let inner = &realm.inner;
    let engine = &inner.engine;
    // Not once the owner was dropped either, as `Engine::create_realm_in_current_thread`
    // refuses then: a deferred realm first entered after that is not made.
    if !engine.is_live() {
        return Err(JsError::ContextDestroyed);
    }
    // Always the default global implementation (JSAPIGlobalObject): only it
    // supports the module loader. Its [[Prototype]] is immutable, so a host
    // global class is emulated by copying its members onto the global
    // (WebIDL puts [Global] interface members on the instance anyway).
    let ctx = unsafe { sys::JSGlobalContextCreateInGroup(engine.group(), ptr::null()) };
    // A window's chain gets a named properties object, as in the standard: own
    // members and `var`s are found (and cached) first, and only a name the global
    // lacks reaches it. The public API cannot change the global's prototype.
    if inner.global_class.is_some() {
        if let Some(install) = sys::spi::get().install_named_properties {
            // SAFETY: fresh context; nothing holds its global proxy yet.
            unsafe { install(ctx, crate::class::named_properties_filter, Some(crate::class::named_properties_get)) };
        }
    }
    inner.ctx.set(ctx);
    // SAFETY: fresh, retained global context.
    inner.global_object.set(unsafe { sys::JSContextGetGlobalObject(ctx) });
    if let Some(target) = sys::spi::get().get_proxy_target {
        // SAFETY: the cached global proxy of a live context.
        let t = unsafe { target(inner.global_object.get()) };
        inner.global_target.set(t as sys::JSObjectRef);
    }
    engine.realms.borrow_mut().insert(ctx as usize, Rc::downgrade(inner));
    // SAFETY: an alias of `inner` that is never dropped (no count of its
    // own); `RealmCore::live` hands it out only until `dispose`.
    let alias = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(Rc::as_ptr(inner)) });
    let _ = inner.core.realm.set(alias);
    inner.core.ctx.set(ctx as sys::JSContextRef);
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
        h.realm_created(realm);
    }
    Ok(())
}

/// Creates every interface object of the realm, with their inheritance.
/// Interface prototype objects and the prototype/static members are made on
/// first use of the class (`populate_class`): a wrap of it or a subclass, or
/// any property operation on its interface object (a Proxy over the
/// constructor whose traps populate and then forward). Interface objects stay
/// data properties of the global, so descriptors and identities match eager
/// installation. The global class chain is populated here (its members are
/// copied onto the global).
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
    // entry each, which dominated realm creation. The class table is the
    // registry's, kept as it registers classes: one string of names and
    // one Int32Array (`ClassRegistry::spec_names`).
    let (spec_names, spec_data) = reg.spec();
    let names = scope.string(spec_names);
    let data = scope.int32_array(spec_data)?;
    // Filled in place: the array (on this stack) keeps each new function alive.
    let natives = scope.array(&[])?;
    for (i, info) in reg.classes.iter().enumerate() {
        let native = match info.def.constructor {
            Some(c) => {
                make_member_fn(scope.ctx, &realm.engine, reg.member, c, false, info.name, info.def.ctor_length, &keys)?
                    .value()
            }
            None => scope.null(),
        };
        natives.set_index(i as u32, native)?;
    }
    let result =
        install.call(None, &[names, data.value(), natives.value(), object_proto, global.value(), populate.value()])?;
    let result = result.as_object().expect("installInterfaces result");
    let lists: Vec<Object<'_>> = (0..3)
        .map(|k| result.get_index(k).map(|v| v.as_object().expect("installInterfaces list")))
        .collect::<JsResult<_>>()?;
    // Prototype objects come with population (`materialize`).
    let protos = vec![ptr::null(); n];
    let interfaces = [0, 1, 2].map(|k| Persistent::new(scope, lists[k].value()));
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
            legacy_factory.call(
                None,
                &[
                    global.value(),
                    scope.string(name),
                    native.value(),
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
    // The pure getters of the global's classes, root-first (see below).
    let global_pure: Vec<(ClassId, &'static str, u16, bool)> = global_chain
        .iter()
        .flatten()
        .flat_map(|c| reg.classes[*c as usize].def.accessors.iter().map(move |a| (*c, a)))
        .filter_map(|(c, a)| Some((c, a.name, a.slot.filter(|_| a.pure)?, a.set.is_some())))
        .collect();
    drop(reg);
    *realm.protos.borrow_mut() = protos;
    *realm.interfaces.borrow_mut() = Some(interfaces);
    *realm.populated.borrow_mut() = vec![false; n];
    if let (Some(chain), Some(name)) = (global_chain, global_name) {
        let gid = *chain.last().expect("non-empty");
        let target = realm.global_target.get();
        // WebIDL puts a [Global] interface's members on the global itself.
        // They are defined there directly (custom accessors on the object
        // behind the proxy), and the interface prototype object stays empty
        // until a script reaches the interface, whose population then copies
        // them (`copy_global_members`). Populating the prototype and copying
        // every member from it cost each realm two definitions per member.
        // Without the proxy's target, the members are copied from the
        // populated prototype as before.
        let on_instance = !target.is_null();
        if on_instance {
            if let Some(&parent) = chain.iter().rev().nth(1) {
                populate_class(scope, parent)?;
            }
            materialize(scope, gid)?;
        } else {
            populate_class(scope, gid)?;
        }
        // Chain root-first: copy members onto the global, brand instanceof.
        let install_global = realm.helper(scope, "installGlobal")?;
        let mut ps = crate::RootedVec::new();
        let mut cs = crate::RootedVec::new();
        let mut ts = crate::RootedVec::new();
        for c in &chain {
            ps.push(scope, realm.interface_object(scope, 0, *c)?);
            cs.push(scope, realm.interface_object(scope, 2, *c)?);
            ts.push(scope, realm.interface_object(scope, 1, *c)?);
        }
        let ps = rooted_array(scope, &ps)?;
        let cs = rooted_array(scope, &cs)?;
        let ts = rooted_array(scope, &ts)?;
        install_global.call(
            None,
            &[global.value(), ps.value(), cs.value(), ts.value(), scope.string(name), scope.boolean(on_instance)],
        )?;
        // `installGlobal` copied each attribute as an accessor of functions,
        // which a global variable read (`document`) calls through a host
        // call frame. A pure getter is a custom accessor of the global
        // instead, as on its prototype, so an inline cache calls it directly
        // and its dispatch takes the lean path for the global's payload.
        if let (Some(install), false) = (sys::spi::get().set_custom_accessor, target.is_null()) {
            for (_, name, slot, has_set) in global_pure.into_iter().filter(|e| !on_instance || e.0 != gid) {
                let n = JsString::new(name);
                let setter = has_set.then(|| crate::accessors::SETTERS[usize::from(slot)]);
                let getter = Some(crate::accessors::GETTERS[usize::from(slot)]);
                // SAFETY: the live global behind this realm's proxy; the copy
                // is configurable.
                unsafe {
                    let mut exc = ptr::null();
                    sys::JSObjectDeleteProperty(scope.ctx, target, n.as_raw(), &mut exc);
                    install(scope.ctx, target, n.as_raw(), getter, setter, sys::kJSPropertyAttributeNone);
                }
            }
        }
        if on_instance {
            // After the parents' copies: a member of the same name replaces
            // one, as its copy did.
            let helpers = realm.helpers.borrow().as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
            let helpers = helpers.as_object().ok_or(JsError::ContextDestroyed)?;
            let interface = realm.interface_object(scope, 1, gid)?.as_object().ok_or(JsError::ContextDestroyed)?;
            let part = MemberPart::Instance(target);
            install_members_raw(scope.ctx, &realm.engine, Some(realm), gid, global, interface, helpers, part)?;
            realm.global_members_on_instance.set(true);
        }
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
        let done = materialize(scope, c)
            .and_then(|()| match realm.global_class == Some(c) && realm.global_members_on_instance.get() {
                true => copy_global_members(scope, c),
                false => install_members(scope, c, MemberPart::All),
            })
            .and_then(|()| mirror_populated(scope, c));
        if let Err(e) = done {
            realm.populated.borrow_mut()[c as usize] = false;
            return Err(e);
        }
    }
    Ok(())
}

/// Makes the interface prototype object of `id` (helpers.js `materialize`)
/// unless it exists: once per class and realm, when the class is first
/// populated, so a realm makes only the prototypes its page reaches.
fn materialize(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let realm = scope.realm;
    if realm.protos.borrow().get(id as usize).is_some_and(|p| !p.is_null()) {
        return Ok(());
    }
    let proto = realm.cached_helper(scope, "materialize")?.call(None, &[scope.number(f64::from(id))])?;
    // Kept alive by the protected prototype list, where `materialize` put it.
    if let Some(slot) = realm.protos.borrow_mut().get_mut(id as usize) {
        *slot = proto.raw;
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

/// The global class's own members, already on the global (see
/// `install_classes`): defined on its interface prototype object as the
/// global has them (the same functions), then the interface's other steps.
fn copy_global_members(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let realm = scope.realm;
    let names: Vec<&'static str> = {
        let reg = realm.engine.classes.borrow();
        let d = &reg.classes.get(id as usize).ok_or(JsError::Type("unknown class".into()))?.def;
        d.methods.iter().map(|m| m.name).chain(d.accessors.iter().map(|a| a.name)).collect()
    };
    let names_array = scope.array(&[])?;
    for (i, n) in names.iter().enumerate() {
        names_array.set_index(i as u32, scope.string(n))?;
    }
    let proto = realm.interface_object(scope, 0, id)?;
    realm.helper(scope, "copyMembers")?.call(None, &[scope.global().value(), proto, names_array.value()])?;
    install_members(scope, id, MemberPart::Interface)
}

fn install_members(scope: &Scope<'_>, id: ClassId, part: MemberPart) -> JsResult<()> {
    let realm = scope.realm;
    let proto = realm.interface_object(scope, 0, id)?;
    let target = realm.interface_object(scope, 1, id)?;
    let helpers = realm.helpers.borrow().as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let (Some(proto), Some(target), Some(helpers)) = (proto.as_object(), target.as_object(), helpers.as_object())
    else {
        return Err(JsError::Type("invalid interface state".into()));
    };
    install_members_raw(scope.ctx, &realm.engine, Some(realm), id, proto, target, helpers, part)
}

/// Which members of a class one installation defines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberPart {
    All,
    /// Methods and attributes only, onto the global (`proto` is its proxy);
    /// custom accessors go on the object behind the proxy, given here.
    Instance(sys::JSObjectRef),
    /// Everything but those: constants, statics, iteration, hooks.
    Interface,
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
    // slow path every time. The payload has no finalizer there: its realm
    // keeps it, then the engine until the realm's global is dead.
    if let Some(make) = sys::spi::get().make_function_with_data {
        // A disposed realm would never clear it again (see `member_fns`).
        let realm = keys.realm.filter(|r| !r.ctx.get().is_null());
        let cell = Cell::new(realm.map(Rc::downgrade));
        let mut payload = Box::new(class::MemberPayload { tag: class::PayloadTag::Member, call, pure, realm: cell });
        let data = &mut *payload as *mut class::MemberPayload as *mut c_void;
        // The realm owns it: the function is made in its context, so its
        // global is the realm's. A disposed realm's goes to the engine.
        match realm {
            Some(r) => {
                debug_assert_eq!(r.ctx.get() as usize, ctx as usize, "member functions are made in their realm");
                r.member_fns.borrow_mut().push(payload)
            }
            None => engine.member_payloads.borrow_mut().push(payload),
        }
        let n = JsString::new(name);
        let f = unsafe { make(ctx, n.as_raw(), length, Some(class::member_call_data), data) };
        if f.is_null() {
            return Err(JsError::Type("cannot create function".into()));
        }
        return Ok(unsafe { Object::from_raw(ctx, f) });
    }
    // No realm: this payload is freed by `member_finalize` (see `MemberPayload::realm`).
    let payload = class::MemberPayload { tag: class::PayloadTag::Member, call, pure, realm: Cell::new(None) };
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
struct MemberKeys<'a> {
    length: JsString,
    name: JsString,
    realm: Option<&'a Rc<RealmInner>>,
}

impl<'a> MemberKeys<'a> {
    fn new(realm: Option<&'a Rc<RealmInner>>) -> MemberKeys<'a> {
        MemberKeys { length: JsString::new("length"), name: JsString::new("name"), realm }
    }
}

#[allow(clippy::too_many_arguments)]
fn install_members_raw(
    ctx: sys::JSContextRef,
    engine: &EngineInner,
    realm: Option<&Rc<RealmInner>>,
    id: ClassId,
    proto: Object<'_>,
    target: Object<'_>,
    helpers: Object<'_>,
    part: MemberPart,
) -> JsResult<()> {
    // Copied out so no registry borrow spans calls into JavaScript.
    let (methods, accessors, constants, statics, iterable, on_global) = {
        let reg = engine.classes.borrow();
        let d = &reg.classes.get(id as usize).ok_or(JsError::Type("unknown class".into()))?.def;
        // Members of the global's class chain run on the global, which is not
        // a wrapper: JSC's own check of a DOM attribute's receiver refuses it.
        let mut on_global = false;
        let mut class = realm.and_then(|r| r.global_class);
        while let Some(c) = class {
            if c == id {
                on_global = true;
                break;
            }
            class = reg.classes[c as usize].parent;
        }
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
            on_global,
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
    let members = helper("members")?;
    // Where custom accessors go: for the global's own members, the object
    // behind its proxy.
    let (holder, instance, interface) = match part {
        MemberPart::All => (proto.raw, true, true),
        MemberPart::Instance(holder) => (holder, true, false),
        MemberPart::Interface => (proto.raw, false, true),
    };
    let methods = if instance { methods } else { Vec::new() };
    let accessors = if instance { accessors } else { Vec::new() };
    let mut batch = MemberBatch::new(ctx, &members, &[proto.raw]);
    for (name, length, call, pure) in methods {
        batch.push(name, native_pure(name, length, call, pure)?.raw as sys::JSValueRef)?;
    }
    batch.flush()?;
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
            // A global's pure getter keeps the custom accessor, whose
            // dispatch takes the lean path for the global's payload.
            let ok = match (crate::accessors::pure_result(slot), sys::spi::get().set_fast_getter) {
                (Some(result), Some(fast)) if !on_global => unsafe {
                    fast(
                        ctx,
                        holder,
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
                _ => unsafe { install(ctx, holder, n.as_raw(), getter, setter, sys::kJSPropertyAttributeNone) },
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
    if !interface {
        return Ok(());
    }
    // Each on the interface object, then on the prototype; one call into
    // JavaScript per batch instead of two per constant.
    let constant_list = helper("constants")?;
    let mut batch = MemberBatch::new(ctx, &constant_list, &[target.raw, proto.raw]);
    for (name, v) in constants {
        batch.push(name, num(v).raw)?;
    }
    batch.flush()?;
    let mut batch = MemberBatch::new(ctx, &members, &[target.raw]);
    for (name, length, call) in statics {
        batch.push(name, native(name, length, call)?.raw as sys::JSValueRef)?;
    }
    batch.flush()?;
    if let Some(kind) = iterable {
        helper(kind)?.call(None, &[proto.value()])?;
    }
    // The embedder's own steps for this interface (`Scope::after_populate`).
    if let Some(hook) = helper("hooks")?.get_index(id)?.as_object() {
        hook.call(None, &[proto.value(), target.value()])?;
    }
    Ok(())
}

/// Named values of one population step, for the helper `members(target,
/// name, function, ...)` (methods, statics) or `constants(target, proto,
/// name, value, ...)`: defined, not assigned (see there), in one call into
/// JavaScript per batch instead of one per value. The arguments wait in an
/// array on this stack, which JavaScriptCore scans conservatively: it keeps
/// each new function alive until the helper has defined it, without a
/// protection per member.
struct MemberBatch<'h> {
    ctx: sys::JSContextRef,
    helper: &'h Object<'h>,
    /// The helper's leading objects, then `name, value` pairs: `len` set.
    args: [sys::JSValueRef; 2 + 2 * MEMBER_BATCH],
    lead: usize,
    len: usize,
}

/// Values per call of a batch helper (most classes have fewer).
const MEMBER_BATCH: usize = 32;

impl<'h> MemberBatch<'h> {
    fn new(ctx: sys::JSContextRef, helper: &'h Object<'h>, lead: &[sys::JSObjectRef]) -> MemberBatch<'h> {
        let mut args = [ptr::null(); 2 + 2 * MEMBER_BATCH];
        for (slot, o) in args.iter_mut().zip(lead) {
            *slot = *o as sys::JSValueRef;
        }
        MemberBatch { ctx, helper, args, lead: lead.len(), len: lead.len() }
    }

    fn push(&mut self, name: &'static str, value: sys::JSValueRef) -> JsResult<()> {
        if self.len + 2 > self.args.len() {
            self.flush()?;
        }
        self.args[self.len + 1] = value;
        // SAFETY: the live callback/scope context; the thread's string for
        // the static name (`atom_raw`) outlives the call.
        self.args[self.len] = unsafe { sys::JSValueMakeString(self.ctx, crate::string::atom_raw(name)) };
        self.len += 2;
        Ok(())
    }

    fn flush(&mut self) -> JsResult<()> {
        if self.len == self.lead {
            return Ok(());
        }
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        // SAFETY: `args[..len]` are live values of `ctx` (see the type).
        unsafe {
            sys::JSObjectCallAsFunction(self.ctx, self.helper.raw, ptr::null_mut(), self.len, self.args.as_ptr(), &mut exc)
        };
        self.len = self.lead;
        check(self.ctx, exc)
    }
}

/// Population of a disposed realm's interface (reachable from another realm):
/// its state lives in the helper object `h` (`classProtos`, `classTargets`,
/// `populated`, the last recorded at disposal), so the result is the same API
/// eager installation exposed; calling native members then throws as before.
fn populate_disposed(ctx: sys::JSContextRef, id: ClassId, h: Object<'_>) -> JsResult<()> {
    let engine = crate::engine::engine_for_ctx(ctx).ok_or(JsError::ContextDestroyed)?;
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
    let make = h.get("materialize")?.as_object().ok_or(JsError::ContextDestroyed)?;
    for c in chain {
        // A prototype nothing reached before the realm closed is made now,
        // and one whose making failed part way completed (`materialize`
        // repeats its steps): `classProtos` alone does not say it is whole.
        // SAFETY: a number in the live callback context.
        let id = unsafe { Value::from_raw(ctx, sys::JSValueMakeNumber(ctx, f64::from(c))) };
        let proto = make.call(None, &[id])?.as_object();
        let (Some(proto), Some(target)) = (proto, targets.get_index(c)?.as_object()) else {
            return Err(JsError::ContextDestroyed);
        };
        populated.set_index(c, yes)?;
        if let Err(e) = install_members_raw(ctx, &engine, None, c, proto, target, h, MemberPart::All) {
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

/// What a console method does beyond forwarding its arguments.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsoleMethod {
    /// Forwards its arguments at this level.
    Plain(ConsoleLevel),
    Count,
    CountReset,
    Time,
    TimeLog,
    TimeEnd,
    /// `timeStamp`, `profile` and `profileEnd`: Chrome hands these to its
    /// tracing and profiler agents and sends no `Runtime.consoleAPICalled`;
    /// with neither here they do nothing.
    Ignored,
}

macro_rules! console_methods {
    ($($name:literal => $method:expr),* $(,)?) => {
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
                        exc: *mut sys::JSValueRef,
                    ) -> sys::JSValueRef {
                        unsafe { console_call(ctx, $method, argc, argv, exc) }
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
    "log" => ConsoleMethod::Plain(ConsoleLevel::Log),
    "debug" => ConsoleMethod::Plain(ConsoleLevel::Debug),
    "info" => ConsoleMethod::Plain(ConsoleLevel::Info),
    "warn" => ConsoleMethod::Plain(ConsoleLevel::Warning),
    "error" => ConsoleMethod::Plain(ConsoleLevel::Error),
    "dir" => ConsoleMethod::Plain(ConsoleLevel::Dir),
    "dirxml" => ConsoleMethod::Plain(ConsoleLevel::Dirxml),
    "table" => ConsoleMethod::Plain(ConsoleLevel::Table),
    "trace" => ConsoleMethod::Plain(ConsoleLevel::Trace),
    "assert" => ConsoleMethod::Plain(ConsoleLevel::Assert),
    "clear" => ConsoleMethod::Plain(ConsoleLevel::Clear),
    "count" => ConsoleMethod::Count,
    "countReset" => ConsoleMethod::CountReset,
    "group" => ConsoleMethod::Plain(ConsoleLevel::StartGroup),
    "groupCollapsed" => ConsoleMethod::Plain(ConsoleLevel::StartGroupCollapsed),
    "groupEnd" => ConsoleMethod::Plain(ConsoleLevel::EndGroup),
    "time" => ConsoleMethod::Time,
    "timeLog" => ConsoleMethod::TimeLog,
    "timeEnd" => ConsoleMethod::TimeEnd,
    "timeStamp" => ConsoleMethod::Ignored,
    "profile" => ConsoleMethod::Ignored,
    "profileEnd" => ConsoleMethod::Ignored,
}

/// Milliseconds since `start` as Chrome prints them (`0.0625`).
fn console_elapsed(start: std::time::Instant) -> String {
    let ms = start.elapsed().as_nanos() as f64 / 1e6;
    format!("{ms}")
}

/// The message and level of a counting or timing call (console.spec.whatwg.org
/// "count", "time" ...; texts as Chrome's), `None` when it reports nothing.
fn console_label_call(realm: &RealmInner, method: ConsoleMethod, label: String) -> Option<(ConsoleLevel, String)> {
    Some(match method {
        ConsoleMethod::Count => {
            let mut counts = realm.console_counts.borrow_mut();
            let n = counts.entry(label.clone()).or_insert(0);
            *n += 1;
            (ConsoleLevel::Count, format!("{label}: {n}"))
        }
        ConsoleMethod::CountReset => match realm.console_counts.borrow_mut().get_mut(&label) {
            Some(n) => {
                *n = 0;
                return None;
            }
            None => (ConsoleLevel::Warning, format!("Count for '{label}' does not exist")),
        },
        ConsoleMethod::Time => {
            let mut timers = realm.console_timers.borrow_mut();
            if timers.contains_key(&label) {
                (ConsoleLevel::Warning, format!("Timer '{label}' already exists"))
            } else {
                timers.insert(label, std::time::Instant::now());
                return None;
            }
        }
        // Chrome reports timeLog as a plain log.
        ConsoleMethod::TimeLog => match realm.console_timers.borrow().get(&label) {
            Some(&start) => (ConsoleLevel::Log, format!("{label}: {} ms", console_elapsed(start))),
            None => (ConsoleLevel::Warning, format!("Timer '{label}' does not exist")),
        },
        ConsoleMethod::TimeEnd => match realm.console_timers.borrow_mut().remove(&label) {
            Some(start) => (ConsoleLevel::TimeEnd, format!("{label}: {} ms", console_elapsed(start))),
            None => (ConsoleLevel::Warning, format!("Timer '{label}' does not exist")),
        },
        ConsoleMethod::Plain(_) | ConsoleMethod::Ignored => return None,
    })
}

/// Hands a console call to the embedder. A panicking hook is the
/// embedder's bug, not the page's: it is not thrown into the script (the
/// call's own errors, a label's ToString, are).
fn console_notify(hooks: &dyn crate::engine::EngineHooks, realm: &Realm, level: ConsoleLevel, values: &[Value<'_>]) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hooks.console_api(realm, level, values)));
}

unsafe fn console_call(
    ctx: sys::JSContextRef,
    method: ConsoleMethod,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    let undefined = unsafe { sys::JSValueMakeUndefined(ctx) };
    unsafe {
        // Errors (a label's ToString) are thrown to the caller.
        class::guard(ctx, exc, undefined, |scope| {
            let args = Args::from_raw(ctx, argc, argv);
            let level = match method {
                ConsoleMethod::Plain(level) => level,
                ConsoleMethod::Ignored => return Ok(undefined),
                _ => {
                    // The label is converted, and its errors thrown, whether
                    // or not anyone listens.
                    let first = args.get(0);
                    let label = if first.is_undefined() { "default".to_owned() } else { first.to_string()? };
                    let Some((level, message)) = console_label_call(scope.realm, method, label) else {
                        return Ok(undefined);
                    };
                    let Some(hooks) = scope.realm.engine.hooks() else { return Ok(undefined) };
                    let mut values = vec![scope.string(&message)];
                    // timeLog's further arguments follow its message.
                    if method == ConsoleMethod::TimeLog && level == ConsoleLevel::Log {
                        values.extend(args.iter().skip(1));
                    }
                    console_notify(&*hooks, &scope.realm(), level, &values);
                    return Ok(undefined);
                }
            };
            let Some(hooks) = scope.realm.engine.hooks() else { return Ok(undefined) };
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
            console_notify(&*hooks, &scope.realm(), level, &values);
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
    module_type: std::os::raw::c_int,
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
                ty: ModuleType::from_raw(module_type),
                key,
                realm: Rc::downgrade(realm),
                resolve: Persistent::new(scope, Value::from_raw(scope.ctx, resolve as sys::JSValueRef)),
                reject: Persistent::new(scope, Value::from_raw(scope.ctx, reject as sys::JSValueRef)),
                done: false,
            };
            if request.ty == ModuleType::JavaScript {
                let inline = realm.inline_modules.borrow_mut().remove(&request.key);
                if let Some(src) = inline {
                    return request.resolve(scope, &src);
                }
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

    /// Every realm evaluates helpers.js: as ASCII it is an 8-bit JSC string
    /// made without transcoding (`JsString::new`), lexed and hashed at its size.
    #[test]
    fn helpers_source_is_ascii() {
        let at = super::HELPERS_JS.find(|c: char| !c.is_ascii());
        assert!(at.is_none(), "non-ASCII character at byte {at:?} of helpers.js (escape it)");
    }

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
    struct Probe;
    impl crate::JsClass for Probe {
        const NAME: &'static str = "MemberRealmProbe";
        type Data = ();
        type Parent = crate::NoParent;
        fn define(def: &mut crate::ClassDef<Self>) {
            def.constructor(0, |s, target, _| s.wrap_new::<Self>((), target, None));
            def.method("one", 0, |s, _, _| Ok(s.number(1.0)));
        }
    }

    /// The member payloads that hold `inner` as their realm, wherever they are.
    fn payloads_holding(engine: &crate::Engine, realm: &super::RealmInner, inner: *const super::RealmInner) -> usize {
        let holds = |p: &super::class::MemberPayload| {
            unsafe { (*p.realm.as_ptr()).as_ref() }.is_some_and(|w| w.as_ptr() == inner)
        };
        let owned = realm.member_fns.borrow().iter().filter(|p| holds(p)).count();
        let kept = engine.inner.member_payloads.borrow().iter().filter(|p| holds(p)).count();
        owned + kept + engine.inner.retired_payloads().filter(|&p| holds(unsafe { &*p })).count()
    }

    /// Disposing a realm clears the realm its member payloads hold
    /// (`RealmInner::member_fns`), so they do not keep its allocation, and its
    /// functions still throw rather than run.
    #[test]
    fn disposed_realm_member_payloads_drop_their_realm() {
        let engine = crate::Engine::new().unwrap();
        engine.register::<Probe>();
        let keep = engine.create_realm(Default::default()).unwrap();
        let realm = engine.create_realm(Default::default()).unwrap();
        let ptr = std::rc::Rc::as_ptr(&realm.inner);
        {
            let s = realm.scope().unwrap();
            assert_eq!(s.eval("new MemberRealmProbe().one()", None, 1).unwrap().to_number().unwrap(), 1.0);
            let f = s.eval("MemberRealmProbe.prototype.one", None, 1).unwrap();
            keep.scope().unwrap().global().set("foreign", f).unwrap();
        }
        let patched = crate::sys::spi::get().make_function_with_data.is_some();
        let held = payloads_holding(&engine, &realm.inner, ptr);
        assert_eq!(held > 0, patched);
        assert_eq!(realm.inner.member_fns.borrow().len(), held);
        let weak = std::rc::Rc::downgrade(&realm.inner);
        realm.dispose();
        assert_eq!(payloads_holding(&engine, &realm.inner, ptr), 0);
        assert!(realm.inner.member_fns.borrow().is_empty());
        drop(realm);
        assert!(weak.upgrade().is_none());
        let s = keep.scope().unwrap();
        assert!(s
            .eval("try { foreign.call(new MemberRealmProbe()); false } catch (e) { true }", None, 1)
            .unwrap()
            .to_bool());
    }

    #[inline(never)]
    fn realm_with_a_held_member(engine: &crate::Engine, keep: &crate::Realm) -> usize {
        let realm = engine.create_realm(Default::default()).unwrap();
        let s = realm.scope().unwrap();
        assert_eq!(s.eval("new MemberRealmProbe().one()", None, 1).unwrap().to_number().unwrap(), 1.0);
        let f = s.eval("MemberRealmProbe.prototype.one", None, 1).unwrap();
        keep.scope().unwrap().global().set("foreign", f).unwrap();
        let n = realm.inner.member_fns.borrow().len();
        drop(s);
        realm.dispose();
        n
    }

    #[inline(never)]
    fn overwrite_dead_stack() -> u64 {
        let mut junk = [0u64; 16 * 1024];
        for (i, slot) in junk.iter_mut().enumerate() {
            *slot = std::hint::black_box(i as u64);
        }
        std::hint::black_box(junk.iter().sum())
    }

    /// A disposed realm's member payloads are freed once its global object
    /// is dead, and not before: a function of it held elsewhere keeps the
    /// global, and must still find its payload when called.
    #[test]
    fn disposed_realm_member_payloads_go_with_its_global() {
        let engine = crate::Engine::new().unwrap();
        engine.register::<Probe>();
        if crate::sys::spi::get().make_function_with_data.is_none() || crate::sys::spi::get().weak().is_none() {
            return;
        }
        let keep = engine.create_realm(Default::default()).unwrap();
        let n = realm_with_a_held_member(&engine, &keep);
        assert!(n > 0);
        let retired = || engine.inner.retired_payloads().count();
        for _ in 0..3 {
            engine.collect_garbage_now();
            engine.drain_finalized();
        }
        assert_eq!(retired(), n, "a held function keeps its realm's payloads");
        // Called, it finds its payload, and throws as a disposed realm's function.
        assert!(keep
            .scope()
            .unwrap()
            .eval("try { foreign.call(new MemberRealmProbe()); false } catch (e) { true }", None, 1)
            .unwrap()
            .to_bool());
        keep.scope().unwrap().eval("foreign = undefined", None, 1).unwrap();
        overwrite_dead_stack();
        for _ in 0..6 {
            engine.collect_garbage_now();
            engine.drain_finalized();
            if retired() == 0 {
                break;
            }
        }
        assert_eq!(retired(), 0, "the payloads outlived their realm's global");
        assert_eq!(engine.inner.member_payloads.borrow().len(), 0);
    }
}
