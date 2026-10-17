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
use std::rc::Rc;

/// Stable identifier of a realm within its engine (CDP `executionContextId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RealmId(pub u32);

/// Key identifying the native object behind a wrapper, so the same object is
/// always exposed as the same JS object within a realm (e.g. document pointer
/// and node id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdentityKey(pub u64, pub u64);

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

impl ModuleRequest {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Provides module source text.
    pub fn resolve(mut self, scope: &Scope<'_>, source: &str) -> JsResult<()> {
        self.done = true;
        let mut err: sys::JSStringRef = ptr::null();
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

pub(crate) struct RealmInner {
    pub(crate) id: RealmId,
    name: String,
    origin: Option<String>,
    pub(crate) engine: Rc<EngineInner>,
    ctx: Cell<sys::JSGlobalContextRef>,
    protos: RefCell<Vec<Option<Persistent>>>,
    /// Interface objects as seen by script (proxies over `targets`).
    ctors: RefCell<Vec<Option<Persistent>>>,
    /// The constructor functions behind the interface proxies.
    targets: RefCell<Vec<Option<Persistent>>>,
    /// Whether each class's members are installed in this realm.
    populated: RefCell<Vec<bool>>,
    identity: RefCell<HashMap<IdentityKey, sys::JSWeakRef>>,
    helpers: RefCell<Option<Persistent>>,
    dom_exception: RefCell<Option<Persistent>>,
    module_loader: Cell<*mut c_void>,
    module_fetcher: RefCell<Option<Rc<dyn ModuleFetcher>>>,
    native_callback_hook: RefCell<Option<NativeCallbackHook>>,
    native_callback_hook_active: Cell<bool>,
    /// Sources of top-level modules passed to `eval_module`; JSC requests the
    /// entry module itself through the fetch hook.
    inline_modules: RefCell<HashMap<String, String>>,
    embedder: RefCell<Option<Rc<dyn Any>>>,
    global_class: Option<ClassId>,
    /// Payload of the global object (set once; freed when the realm is dropped,
    /// never while a callback can still borrow it).
    global_payload: Cell<*mut Header>,
}

impl RealmInner {
    pub(crate) fn native_callback_hook(&self) -> Option<NativeCallbackHook> {
        if self.ctx.get().is_null() || self.native_callback_hook_active.get() {
            None
        } else {
            self.native_callback_hook.borrow().clone()
        }
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
        self.engine.realms.borrow_mut().retain(|(g, _)| *g != ctx);
        let loader = self.module_loader.replace(ptr::null_mut());
        if !loader.is_null() {
            unsafe { sys::modules::lp_jsc_module_loader_release(loader) };
        }
        self.module_fetcher.borrow_mut().take();
        // Release captured embedder state without holding the hook cell borrow.
        drop(self.native_callback_hook.take());
        self.protos.borrow_mut().clear();
        self.ctors.borrow_mut().clear();
        self.targets.borrow_mut().clear();
        self.populated.borrow_mut().clear();
        self.helpers.borrow_mut().take();
        self.dom_exception.borrow_mut().take();
        if let Some((_, release, _)) = sys::spi::get().weak() {
            for (_, w) in self.identity.borrow_mut().drain() {
                unsafe { release(self.engine.group(), w) };
            }
        }
        self.embedder.borrow_mut().take();
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
    pub fn id(&self) -> RealmId {
        self.inner.id
    }

    pub fn name(&self) -> &str {
        &self.inner.name
    }

    pub fn origin(&self) -> Option<&str> {
        self.inner.origin.as_deref()
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
        let h = class::new_payload(&self.inner.engine, id, Kind::Instance, self.inner.id, None, data);
        self.inner.global_payload.set(h);
        Ok(())
    }

    /// Attaches embedder state reachable from callbacks via [`Scope::embedder`].
    pub fn set_embedder<T: Any>(&self, data: Rc<T>) {
        *self.inner.embedder.borrow_mut() = Some(data);
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

pub(crate) fn realm_for_ctx(ctx: sys::JSContextRef) -> Option<Rc<RealmInner>> {
    let engine = crate::engine::current_inner()?;
    let global = unsafe { sys::JSContextGetGlobalContext(ctx) };
    engine.realm_for_global(global)
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
        let v = unsafe {
            sys::modules::lp_jsc_module_evaluate(
                self.realm.global_ctx()?,
                source.as_ptr().cast(),
                source.len(),
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
        let f = self.eval(&format!("({function_source}\n)"), None, 1)?;
        let f = f
            .as_object()
            .filter(|o| o.is_function())
            .ok_or_else(|| JsError::Type("Given expression does not evaluate to a function".into()))?;
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
    pub fn call_function_rooted(
        &self,
        function_source: &str,
        this: Option<&Persistent>,
        args: &crate::RootedVec,
    ) -> JsResult<Value<'s>> {
        let f = self.eval(&format!("({function_source}\n)"), None, 1)?;
        let f = f
            .as_object()
            .filter(|o| o.is_function())
            .ok_or_else(|| JsError::Type("Given expression does not evaluate to a function".into()))?;
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
        let h = class::new_payload(&self.realm.engine, u32::MAX, Kind::Closure, self.realm.id, None, data);
        let closure_class = self.realm.engine.classes.borrow().closure;
        let obj = unsafe { sys::JSObjectMake(self.ctx, closure_class, h as *mut c_void) };
        let obj = unsafe { Object::from_raw(self.ctx, obj) };
        let fproto = self.global().get("Function")?.as_object().map(|f| f.get("prototype"));
        if let Some(Ok(p)) = fproto {
            obj.set_prototype(p);
        }
        let nm = self.string(name);
        let _ = self.realm.helper(self, "setName").and_then(|h| h.call(None, &[obj.value(), nm]));
        Ok(obj)
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
        self.realm.helper(self, "then")?.call(None, &[value, fok.value(), ferr.value()])?;
        Ok(())
    }

    // -- host objects ------------------------------------------------------

    /// Returns the wrapper for `data`, reusing the existing one for `key` in this
    /// realm (in which case `data` is dropped).
    pub fn wrap<C: JsClass>(&self, data: C::Data, key: Option<IdentityKey>) -> JsResult<Object<'s>> {
        if let Some(k) = key {
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
        let h = class::new_payload(&self.realm.engine, id, Kind::Instance, self.realm.id, key, data);
        let obj = unsafe { sys::JSObjectMake(self.ctx, class, h as *mut c_void) };
        let obj = unsafe { Object::from_raw(self.ctx, obj) };
        obj.set_prototype(proto);
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
    /// rooted by the caller's frame for the callback's duration, so no guard
    /// is needed.
    pub fn unwrap_arg<C: JsClass>(&self, args: &Args<'s>, i: usize) -> Option<&'s C::Data> {
        self.unwrap_raw::<C>(args.opt(i)?).map(|(d, _)| d)
    }

    /// Payload of the realm's global object, if it is (a subclass of) `C`.
    pub fn global_data<C: JsClass>(&self) -> Option<&'s C::Data> {
        let h = self.realm.global_payload.get();
        if h.is_null() {
            return None;
        }
        let reg = self.realm.engine.classes.borrow();
        // SAFETY: the global payload lives as long as the realm, which this
        // scope borrows for 's.
        unsafe { class::class_data::<C::Data>(&reg, h, reg.id_of::<C>()?) }
    }

    /// Data and object of a `C` wrapper (object is null for the global).
    fn unwrap_raw<C: JsClass>(&self, value: Value<'_>) -> Option<(&'s C::Data, sys::JSObjectRef)> {
        let o = value.as_object()?;
        let h = unsafe { self.header_of_object(o.raw) }?;
        let is_global = h == self.realm.global_payload.get();
        unsafe {
            if (*h).kind != Kind::Instance {
                return None;
            }
            let reg = self.realm.engine.classes.borrow();
            let data = class::class_data::<C::Data>(&reg, h, reg.id_of::<C>()?)?;
            Some((data, if is_global { ptr::null_mut() } else { o.raw }))
        }
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
        let base = self.realm.engine.classes.borrow().base;
        unsafe {
            if sys::JSValueIsObjectOfClass(self.ctx, obj as sys::JSValueRef, base) {
                let h = sys::JSObjectGetPrivate(obj) as *mut Header;
                return (!h.is_null()).then_some(h);
            }
            if obj == sys::JSContextGetGlobalObject(self.ctx) {
                let h = self.realm.global_payload.get();
                return (!h.is_null()).then_some(h);
            }
        }
        None
    }

    /// Payload header of a callback receiver, mapping `null`/global to the
    /// realm's global object.
    ///
    /// # Safety
    /// `this` must be the receiver JSC passed to the current callback.
    pub(crate) unsafe fn header_of_receiver(&self, this: sys::JSObjectRef) -> Option<*mut Header> {
        unsafe {
            let obj = if this.is_null() { sys::JSContextGetGlobalObject(self.ctx) } else { this };
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
    /// cycles through it stay collectable.
    pub fn set_hidden(&self, object: Object<'_>, key: &str, value: Value<'_>) -> JsResult<()> {
        let f = sys::spi::get().set_private_property.ok_or(JsError::Unsupported("JSObjectSetPrivateProperty"))?;
        let ok = unsafe { f(self.ctx, object.raw, JsString::new(key).as_raw(), value.raw) };
        if ok {
            Ok(())
        } else {
            Err(JsError::Type("hidden properties require a host object".into()))
        }
    }

    pub fn get_hidden(&self, object: Object<'_>, key: &str) -> JsResult<Option<Value<'s>>> {
        let f = sys::spi::get().get_private_property.ok_or(JsError::Unsupported("JSObjectGetPrivateProperty"))?;
        let v = unsafe { f(self.ctx, object.raw, JsString::new(key).as_raw()) };
        Ok((!v.is_null()).then(|| unsafe { Value::from_raw(self.ctx, v) }))
    }

    /// Reports an uncaught exception to the engine hooks (console/CDP).
    /// Termination is not an exception and is not reported.
    pub fn report_exception(&self, error: &JsError) {
        if matches!(error, JsError::Terminated) {
            return;
        }
        if let Some(h) = self.realm.engine.hooks() {
            let details = error.details(self);
            h.exception_thrown(&self.realm(), &details);
        }
    }

    /// Describes a value for remote-object protocols: `(type, subtype, className)`
    /// following CDP `Runtime.RemoteObject`.
    pub fn describe(&self, value: Value<'_>) -> JsResult<ValueDescription> {
        let d = self.realm.helper(self, "describe")?.call(None, &[value])?;
        let d = d.as_object().expect("describe result");
        Ok(ValueDescription {
            ty: d.get("type")?.as_string().unwrap_or_default(),
            subtype: d.get("subtype")?.as_string(),
            class_name: d.get("className")?.as_string(),
            description: d.get("description")?.as_string(),
        })
    }

    /// Own property names including non-enumerable and symbols, as an array of
    /// `{name, symbol?, value?, get?, set?, writable, enumerable, configurable, isOwn}`.
    pub fn own_properties(&self, object: Object<'_>, own_only: bool) -> JsResult<Object<'s>> {
        let r = self.realm.helper(self, "properties")?.call(None, &[object.value(), self.boolean(own_only)])?;
        r.as_object().ok_or_else(|| JsError::Type("properties helper failed".into()))
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
        class::guard(ctx, exc, ptr::null(), |scope| {
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
    let id = RealmId(engine.next_realm_id.get());
    engine.next_realm_id.set(id.0 + 1);
    let inner = Rc::new(RealmInner {
        id,
        name: options.name,
        origin: options.origin,
        engine: engine.clone(),
        ctx: Cell::new(ctx),
        protos: RefCell::new(Vec::new()),
        ctors: RefCell::new(Vec::new()),
        targets: RefCell::new(Vec::new()),
        populated: RefCell::new(Vec::new()),
        identity: RefCell::new(HashMap::new()),
        helpers: RefCell::new(None),
        dom_exception: RefCell::new(None),
        module_loader: Cell::new(ptr::null_mut()),
        module_fetcher: RefCell::new(None),
        native_callback_hook: RefCell::new(None),
        native_callback_hook_active: Cell::new(false),
        inline_modules: RefCell::new(HashMap::new()),
        embedder: RefCell::new(None),
        global_class: options.global_class,
        global_payload: Cell::new(ptr::null_mut()),
    });
    engine.realms.borrow_mut().push((ctx, Rc::downgrade(&inner)));
    let realm = Realm { inner };
    {
        let scope = realm.scope()?;
        let helpers = scope.eval(HELPERS_JS, Some("lightpanda://internal/helpers.js"), 1)?;
        *realm.inner.helpers.borrow_mut() = Some(Persistent::new(&scope, helpers));
        let dom_exc = helpers.as_object().expect("helpers").get("DOMException")?;
        *realm.inner.dom_exception.borrow_mut() = Some(Persistent::new(&scope, dom_exc));
        install_classes(&scope)?;
        install_console(&scope)?;
        install_rejection_tracking(&scope)?;
        if engine.caps.modules {
            let h = unsafe { sys::modules::lp_jsc_module_loader_install(ctx, module_fetch, ptr::null_mut()) };
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
    let make_interface = realm.helper(scope, "makeInterface")?;
    let lazy_interface = realm.helper(scope, "lazyInterface")?;
    let populate = make_native_fn(scope, "populate", populate_tramp);
    let n = reg.classes.len();
    let mut protos: Vec<Option<Persistent>> = Vec::with_capacity(n);
    let mut ctors: Vec<Option<Persistent>> = Vec::with_capacity(n);
    let mut targets: Vec<Option<Persistent>> = Vec::with_capacity(n);
    for (id, info) in reg.classes.iter().enumerate() {
        let proto = scope.object();
        let parent_proto = match info.parent {
            Some(p) => protos[p as usize].as_ref().expect("parent proto").get(scope),
            None => object_proto,
        };
        proto.set_prototype(parent_proto);
        let native = match info.def.constructor {
            Some(c) => make_native_fn(scope, info.name, c).value(),
            None => scope.null(),
        };
        let parent_ctor = match info.parent {
            Some(p) => ctors[p as usize].as_ref().expect("parent ctor").get(scope),
            None => scope.null(),
        };
        let target = make_interface.call(
            None,
            &[
                scope.string(info.name),
                native,
                proto.value(),
                parent_ctor,
                scope.number(info.def.ctor_length as f64),
                scope.null(),
            ],
        )?;
        let expose_on = if info.def.exposed { global.value() } else { scope.null() };
        let ctor = lazy_interface.call(
            None,
            &[target, proto.value(), expose_on, scope.string(info.name), scope.number(id as f64), populate.value()],
        )?;
        protos.push(Some(Persistent::new(scope, proto.value())));
        ctors.push(Some(Persistent::new(scope, ctor)));
        targets.push(Some(Persistent::new(scope, target)));
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

/// `populate(id)` for the interface proxies. A disposed realm has nothing to
/// install: return without throwing so its inert objects stay readable.
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
        return undefined;
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
        if let Err(e) = install_members(scope, c) {
            realm.populated.borrow_mut()[c as usize] = false;
            return Err(e);
        }
    }
    Ok(())
}

fn install_members(scope: &Scope<'_>, id: ClassId) -> JsResult<()> {
    let realm = scope.realm;
    let proto = realm.protos.borrow()[id as usize].as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let target = realm.targets.borrow()[id as usize].as_ref().ok_or(JsError::ContextDestroyed)?.get(scope);
    let proto = proto.as_object().expect("prototype object");
    let target_obj = target.as_object().expect("interface target");
    // Copied out so no registry borrow spans calls into JavaScript.
    let (methods, accessors, constants, statics, iterable) = {
        let reg = realm.engine.classes.borrow();
        let d = &reg.classes[id as usize].def;
        (
            d.methods.iter().map(|m| (m.name, m.length, m.call)).collect::<Vec<_>>(),
            d.accessors.iter().map(|a| (a.name, a.get, a.set)).collect::<Vec<_>>(),
            d.constants.clone(),
            d.statics.iter().map(|m| (m.name, m.length, m.call)).collect::<Vec<_>>(),
            d.indexed_iterable,
        )
    };
    let set_length = realm.helper(scope, "setLength")?;
    for (name, length, call) in methods {
        let f = make_native_fn(scope, name, call);
        proto.set(name, f.value())?;
        // JSC callback functions already have a configurable length of 0.
        if length != 0 {
            set_length.call(None, &[f.value(), scope.number(length as f64)])?;
        }
    }
    let accessor = realm.helper(scope, "accessor")?;
    for (name, get, set) in accessors {
        let g = make_native_fn(scope, name, get);
        let st = set.map(|f| make_native_fn(scope, name, f).value()).unwrap_or_else(|| scope.undefined());
        accessor.call(None, &[proto.value(), scope.string(name), g.value(), st])?;
    }
    let constant = realm.helper(scope, "constant")?;
    for (name, v) in constants {
        constant.call(None, &[target, scope.string(name), scope.number(v)])?;
        constant.call(None, &[proto.value(), scope.string(name), scope.number(v)])?;
    }
    for (name, length, call) in statics {
        let f = make_native_fn(scope, name, call);
        target_obj.set(name, f.value())?;
        if length != 0 {
            set_length.call(None, &[f.value(), scope.number(length as f64)])?;
        }
    }
    if iterable {
        realm.helper(scope, "iterable")?.call(None, &[proto.value()])?;
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
        if let Some(h) = s.realm.engine.hooks() {
            let details = JsException::from_value(s, args.get(1));
            h.unhandled_rejection(&s.realm(), &details);
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
            let url = remap_module_url(&key, realm.origin.as_deref());
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
    use super::remap_module_url;

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
