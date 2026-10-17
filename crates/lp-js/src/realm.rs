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

pub(crate) struct RealmInner {
    pub(crate) id: RealmId,
    name: String,
    origin: Option<String>,
    pub(crate) engine: Rc<EngineInner>,
    ctx: Cell<sys::JSGlobalContextRef>,
    protos: RefCell<Vec<Option<Persistent>>>,
    ctors: RefCell<Vec<Option<Persistent>>>,
    identity: RefCell<HashMap<IdentityKey, sys::JSWeakRef>>,
    helpers: RefCell<Option<Persistent>>,
    dom_exception: RefCell<Option<Persistent>>,
    module_loader: Cell<*mut c_void>,
    module_fetcher: RefCell<Option<Rc<dyn ModuleFetcher>>>,
    /// Sources of top-level modules passed to `eval_module`; JSC requests the
    /// entry module itself through the fetch hook.
    inline_modules: RefCell<HashMap<String, String>>,
    embedder: RefCell<Option<Rc<dyn Any>>>,
    global_class: Option<ClassId>,
}

impl RealmInner {
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

    fn dispose(&self) {
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
        self.protos.borrow_mut().clear();
        self.ctors.borrow_mut().clear();
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

    /// Enters the realm. Fails after disposal.
    pub fn scope(&self) -> JsResult<Scope<'_>> {
        let ctx = self.inner.global_ctx()?;
        Ok(Scope::new(ctx as sys::JSContextRef, &self.inner))
    }

    /// Tears the realm down now: callbacks into its functions afterwards throw
    /// "Execution context was destroyed.", persistent prototypes are released
    /// and `EngineHooks::realm_destroyed` fires. Idempotent.
    pub fn dispose(&self) {
        self.inner.dispose();
    }

    /// Sets the payload of the global object (realm created with `global_class`).
    pub fn set_global_data<C: JsClass>(&self, data: C::Data) -> JsResult<()> {
        let ctx = self.inner.global_ctx()?;
        let reg = self.inner.engine.classes.borrow();
        let id = reg.id_of::<C>().ok_or(JsError::Type(format!("{} not registered", C::NAME)))?;
        if self.inner.global_class != Some(id) {
            return Err(JsError::Type(format!("realm global is not a {}", C::NAME)));
        }
        let h = class::new_payload(id, Kind::Instance, self.inner.id, None, data);
        unsafe {
            let global = sys::JSContextGetGlobalObject(ctx);
            let old = sys::JSObjectGetPrivate(global) as *mut Header;
            sys::JSObjectSetPrivate(global, h as *mut c_void);
            if !old.is_null() {
                ((*old).drop_fn)(old);
            }
        }
        Ok(())
    }

    /// Attaches embedder state reachable from callbacks via [`Scope::embedder`].
    pub fn set_embedder<T: Any>(&self, data: Rc<T>) {
        *self.inner.embedder.borrow_mut() = Some(data);
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

    /// Creates a function backed by a Rust closure (may capture state).
    pub fn function(
        &self,
        name: &str,
        f: impl for<'a> Fn(&Scope<'a>, Value<'a>, Args<'a>) -> JsResult<Value<'a>> + 'static,
    ) -> JsResult<Object<'s>> {
        let data: ClosureData = Box::new(f);
        let h = class::new_payload(u32::MAX, Kind::Closure, self.realm.id, None, data);
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
        let h = class::new_payload(id, Kind::Instance, self.realm.id, key, data);
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

    /// Borrows the payload of a `C` (or subclass) wrapper.
    pub fn unwrap<C: JsClass>(&self, value: Value<'_>) -> Option<&'s C::Data> {
        let o = value.as_object()?;
        let base = self.realm.engine.classes.borrow().base;
        unsafe {
            if !sys::JSValueIsObjectOfClass(self.ctx, o.raw as sys::JSValueRef, base) {
                return None;
            }
            let h = sys::JSObjectGetPrivate(o.raw) as *mut Header;
            if h.is_null() || (*h).kind != Kind::Instance {
                return None;
            }
            let reg = self.realm.engine.classes.borrow();
            // The payload lives while the wrapper does; `value` is rooted for 's.
            class::class_data::<C::Data>(&reg, h, reg.id_of::<C>()?)
        }
    }

    /// Payload header of a callback receiver, mapping `null`/global to the
    /// realm's global object.
    ///
    /// # Safety
    /// `this` must be the receiver JSC passed to the current callback.
    pub(crate) unsafe fn header_of_receiver(&self, this: sys::JSObjectRef) -> Option<*mut Header> {
        let base = self.realm.engine.classes.borrow().base;
        unsafe {
            let obj = if this.is_null() { sys::JSContextGetGlobalObject(self.ctx) } else { this };
            if !sys::JSValueIsObjectOfClass(self.ctx, obj as sys::JSValueRef, base) {
                return None;
            }
            let h = sys::JSObjectGetPrivate(obj) as *mut Header;
            (!h.is_null()).then_some(h)
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
    pub fn report_exception(&self, error: &JsError) {
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
    let Some(realm) = realm_for_ctx(ctx) else {
        unsafe { crate::error::throw_message(ctx, "Execution context was destroyed.", exc) };
        return ptr::null();
    };
    let scope = Scope::new(ctx, &realm);
    let h = unsafe { sys::JSObjectGetPrivate(function) } as *mut Header;
    let f = if h.is_null() { None } else { unsafe { class::payload_data::<ClosureData>(h) } };
    let Some(f) = f else {
        unsafe { crate::error::throw_message(ctx, "Illegal invocation", exc) };
        return ptr::null();
    };
    let this_v =
        if this.is_null() { scope.undefined() } else { unsafe { Value::from_raw(ctx, this as sys::JSValueRef) } };
    let args = unsafe { Args::from_raw(ctx, argc, argv) };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&scope, this_v, args))) {
        Ok(Ok(v)) => v.raw,
        Ok(Err(e)) => {
            unsafe { crate::error::throw_into(&scope, &e, exc) };
            ptr::null()
        }
        Err(_) => {
            unsafe { crate::error::throw_message(ctx, "Internal error: panic in native function", exc) };
            ptr::null()
        }
    }
}

// ---------------------------------------------------------------------------
// Realm creation

const HELPERS_JS: &str = include_str!("helpers.js");

pub(crate) fn create(engine: &Rc<EngineInner>, options: RealmOptions) -> JsResult<Realm> {
    let global_jsclass = match options.global_class {
        Some(id) => {
            let mut reg = engine.classes.borrow_mut();
            if id as usize >= reg.classes.len() {
                return Err(JsError::Type("unknown global class".into()));
            }
            reg.global_class(id)
        }
        None => ptr::null(),
    };
    let ctx = unsafe { sys::JSGlobalContextCreateInGroup(engine.group(), global_jsclass) };
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
        identity: RefCell::new(HashMap::new()),
        helpers: RefCell::new(None),
        dom_exception: RefCell::new(None),
        module_loader: Cell::new(ptr::null_mut()),
        module_fetcher: RefCell::new(None),
        inline_modules: RefCell::new(HashMap::new()),
        embedder: RefCell::new(None),
        global_class: options.global_class,
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

fn install_classes(scope: &Scope<'_>) -> JsResult<()> {
    let realm = scope.realm;
    let reg = realm.engine.classes.borrow();
    let global = scope.global();
    let object_proto = global.get("Object")?.as_object().expect("Object").get("prototype")?;
    let make_interface = realm.helper(scope, "makeInterface")?;
    let accessor = realm.helper(scope, "accessor")?;
    let constant = realm.helper(scope, "constant")?;
    let iterable = realm.helper(scope, "iterable")?;
    let set_length = realm.helper(scope, "setLength")?;
    let mut protos: Vec<Option<Persistent>> = Vec::with_capacity(reg.classes.len());
    let mut ctors: Vec<Option<Persistent>> = Vec::with_capacity(reg.classes.len());
    for (id, info) in reg.classes.iter().enumerate() {
        let is_global = realm.global_class == Some(id as ClassId);
        let proto = if is_global { global.prototype().as_object().expect("global prototype") } else { scope.object() };
        let parent_proto = match info.parent {
            Some(p) => protos[p as usize].as_ref().expect("parent proto").get(scope),
            None => object_proto,
        };
        proto.set_prototype(parent_proto);
        for m in &info.def.methods {
            let f = make_native_fn(scope, m.name, m.call);
            proto.set(m.name, f.value())?;
            set_length.call(None, &[f.value(), scope.number(m.length as f64)])?;
        }
        for a in &info.def.accessors {
            let g = make_native_fn(scope, a.name, a.get);
            let s = a.set.map(|s| make_native_fn(scope, a.name, s).value()).unwrap_or_else(|| scope.undefined());
            accessor.call(None, &[proto.value(), scope.string(a.name), g.value(), s])?;
        }
        let native = match info.def.constructor {
            Some(c) => make_native_fn(scope, info.name, c).value(),
            None => scope.null(),
        };
        let parent_ctor = match info.parent {
            Some(p) => ctors[p as usize].as_ref().expect("parent ctor").get(scope),
            None => scope.null(),
        };
        let expose_on = if info.def.exposed { global.value() } else { scope.null() };
        let ctor = make_interface.call(
            None,
            &[
                scope.string(info.name),
                native,
                proto.value(),
                parent_ctor,
                scope.number(info.def.ctor_length as f64),
                expose_on,
            ],
        )?;
        let ctor_obj = ctor.as_object().expect("interface object");
        for (name, v) in &info.def.constants {
            constant.call(None, &[ctor, scope.string(name), scope.number(*v)])?;
            constant.call(None, &[proto.value(), scope.string(name), scope.number(*v)])?;
        }
        for m in &info.def.statics {
            let f = make_native_fn(scope, m.name, m.call);
            ctor_obj.set(m.name, f.value())?;
            set_length.call(None, &[f.value(), scope.number(m.length as f64)])?;
        }
        if info.def.indexed_iterable {
            iterable.call(None, &[proto.value()])?;
        }
        protos.push(Some(Persistent::new(scope, proto.value())));
        ctors.push(Some(Persistent::new(scope, ctor)));
    }
    *realm.protos.borrow_mut() = protos;
    *realm.ctors.borrow_mut() = ctors;
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
    let Some(realm) = realm_for_ctx(ctx) else { return undefined };
    let Some(hooks) = realm.engine.hooks() else { return undefined };
    let scope = Scope::new(ctx, &realm);
    let args = unsafe { Args::from_raw(ctx, argc, argv) };
    if level == ConsoleLevel::Assert {
        if args.get(0).to_bool() {
            return undefined;
        }
        let rest: Vec<Value<'_>> = args.iter().skip(1).collect();
        let _ =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hooks.console_api(&scope.realm(), level, &rest)));
        return undefined;
    }
    let values: Vec<Value<'_>> = args.iter().collect();
    let _ =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hooks.console_api(&scope.realm(), level, &values)));
    undefined
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
    let Some(realm) = realm_for_ctx(ctx as sys::JSContextRef) else { return };
    let scope = Scope::new(ctx as sys::JSContextRef, &realm);
    let key = unsafe { crate::string::to_rust_string(identifier) };
    let mut url = key.clone();
    // JSC resolves root-relative specifiers ("/x.js") against file:///; the
    // page's own origin is what the browser semantics require.
    if let (Some(origin), Some(path)) = (realm.origin.as_deref(), url.strip_prefix("file://")) {
        if !origin.starts_with("file:") {
            url = format!("{}{}", origin.trim_end_matches('/'), path);
        }
    }
    let request = unsafe {
        ModuleRequest {
            url: url.clone(),
            key,
            realm: Rc::downgrade(&realm),
            resolve: Persistent::new(&scope, Value::from_raw(scope.ctx, resolve as sys::JSValueRef)),
            reject: Persistent::new(&scope, Value::from_raw(scope.ctx, reject as sys::JSValueRef)),
            done: false,
        }
    };
    let inline = realm.inline_modules.borrow_mut().remove(&url);
    if let Some(src) = inline {
        let _ = request.resolve(&scope, &src);
        return;
    }
    let fetcher = realm.module_fetcher.borrow().clone();
    match fetcher {
        Some(f) => {
            let r = scope.realm();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.fetch(&r, url, request)));
        }
        None => {
            let _ = request.reject(&scope, &JsError::Type("module loading is not configured for this realm".into()));
        }
    }
}
