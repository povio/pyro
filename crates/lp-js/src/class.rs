//! Host classes: WebIDL-shaped interfaces backed by Rust data.
//!
//! Shape per realm: an interface object (constructor function) whose
//! `prototype` holds methods (real `Function`s), accessors (getter/setter
//! pairs, as `Object.getOwnPropertyDescriptor(Proto, name)` expects), constants
//! and `Symbol.toStringTag`; prototypes chain to the parent interface.
//! Instances are JSC callback objects of a per-class `JSClassRef` whose private
//! pointer is a `Boxed<C::Data>`; brand checks use the class id stored in the
//! header, not the (mutable) JS prototype chain.
//!
//! Native callbacks are monomorphized `extern "C"` trampolines generated from
//! zero-sized closures / fn items, so calls need no per-function allocation or
//! lookup. Panics are caught at the boundary and rethrown as JS errors.
//!
//! GC finalizers never run Rust payload destructors: they enqueue the payload,
//! and [`drain_finalized`] drops it at a safe point.

use crate::engine::EngineInner;
use crate::error::{throw_into, throw_message, JsError, JsResult};
use crate::realm::{IdentityKey, RealmId, Scope};
use crate::string::{utf16_of, JsString};
use crate::value::{Args, Object, Value};
use lp_jsc_sys as sys;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::marker::PhantomData;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

pub type ClassId = u32;

/// Marker for classes without a parent interface.
pub enum NoParent {}

/// A host interface. Implement on a marker type; `Data` is the Rust state
/// stored in each instance (use interior mutability for mutation). A subclass
/// either uses its parent's `Data` type or declares [`ClassDef::upcast`] to
/// project its data to the parent's, so inherited members can borrow it.
pub trait JsClass: 'static {
    const NAME: &'static str;
    type Data: 'static;
    /// Parent interface, or [`NoParent`].
    type Parent: JsClass;
    fn define(def: &mut ClassDef<Self>)
    where
        Self: Sized;
}

impl JsClass for NoParent {
    const NAME: &'static str = "";
    type Data = ();
    type Parent = NoParent;
    fn define(_: &mut ClassDef<Self>) {}
}

/// Raw `JSObjectCallAsFunctionCallback`.
type RawFn = sys::JSObjectCallAsFunctionCallbackRaw;
type IndexedFn = unsafe fn(&Scope<'_>, *mut Header, u32) -> JsResult<Option<sys::JSValueRef>>;
type NamedFn = unsafe fn(&Scope<'_>, *mut Header, &str) -> JsResult<Option<sys::JSValueRef>>;
type NamedSetFn = unsafe fn(&Scope<'_>, *mut Header, &str, sys::JSValueRef) -> JsResult<bool>;
type NamedDeleteFn = unsafe fn(&Scope<'_>, *mut Header, &str) -> JsResult<bool>;
type NamesFn = unsafe fn(&Scope<'_>, *mut Header) -> JsResult<Vec<String>>;

pub(crate) struct MethodSpec {
    pub name: &'static str,
    pub length: u32,
    pub call: RawFn,
}

pub(crate) struct AccessorSpec {
    pub name: &'static str,
    pub get: RawFn,
    pub set: Option<RawFn>,
}

#[derive(Default)]
pub(crate) struct PropertyHooks {
    pub indexed: Option<IndexedFn>,
    pub named: Option<NamedFn>,
    /// `[LegacyOverrideBuiltIns]`: named properties shadow prototype members.
    pub override_builtins: bool,
    pub named_set: Option<NamedSetFn>,
    pub named_delete: Option<NamedDeleteFn>,
    pub names: Option<NamesFn>,
}

/// Builder passed to [`JsClass::define`].
pub struct ClassDef<C: JsClass> {
    pub(crate) methods: Vec<MethodSpec>,
    pub(crate) statics: Vec<MethodSpec>,
    pub(crate) accessors: Vec<AccessorSpec>,
    pub(crate) constants: Vec<(&'static str, f64)>,
    pub(crate) constructor: Option<RawFn>,
    pub(crate) ctor_length: u32,
    pub(crate) hooks: PropertyHooks,
    pub(crate) exposed: bool,
    pub(crate) indexed_iterable: bool,
    pub(crate) upcast: Option<UpcastFn>,
    _c: PhantomData<C>,
}

/// Projects a pointer to a class's data to a pointer to its parent's data.
pub(crate) type UpcastFn = unsafe fn(*const u8) -> *const u8;

/// Produces the value of a zero-sized callable from nothing.
///
/// # Safety
/// `F` must be zero-sized and have no validity invariants (closures and fn
/// items); enforced by the `assert` at registration time and here.
unsafe fn conjure<F: Copy>() -> F {
    assert!(std::mem::size_of::<F>() == 0, "callback must be a non-capturing closure or fn item");
    // SAFETY: zero-sized type, no bytes to initialize.
    // Zero-sized reads are valid from any aligned non-null pointer.
    unsafe { std::ptr::NonNull::<F>::dangling().as_ptr().read() }
}

fn assert_zst<F>() {
    assert!(
        std::mem::size_of::<F>() == 0,
        "host class callbacks must be non-capturing closures or fn items (use the Data type for state)"
    );
}

impl<C: JsClass> ClassDef<C> {
    fn new() -> Self {
        ClassDef {
            methods: Vec::new(),
            statics: Vec::new(),
            accessors: Vec::new(),
            constants: Vec::new(),
            constructor: None,
            ctor_length: 0,
            hooks: PropertyHooks::default(),
            exposed: true,
            indexed_iterable: false,
            upcast: None,
            _c: PhantomData,
        }
    }

    /// Adds an operation on the prototype.
    pub fn method<F>(&mut self, name: &'static str, length: u32, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, Args<'s>) -> JsResult<Value<'s>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.methods.push(MethodSpec { name, length, call: method_tramp::<C, F> });
        self
    }

    /// Adds a read-only attribute.
    pub fn getter<G>(&mut self, name: &'static str, _g: G) -> &mut Self
    where
        G: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Value<'s>> + Copy + 'static,
    {
        assert_zst::<G>();
        self.accessors.push(AccessorSpec { name, get: getter_tramp::<C, G>, set: None });
        self
    }

    /// Adds a read-write attribute.
    pub fn accessor<G, S>(&mut self, name: &'static str, _g: G, _s: S) -> &mut Self
    where
        G: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Value<'s>> + Copy + 'static,
        S: for<'s> Fn(&Scope<'s>, &C::Data, Value<'s>) -> JsResult<()> + Copy + 'static,
    {
        assert_zst::<G>();
        assert_zst::<S>();
        self.accessors.push(AccessorSpec { name, get: getter_tramp::<C, G>, set: Some(setter_tramp::<C, S>) });
        self
    }

    /// Adds a constant on both the interface object and the prototype.
    pub fn constant(&mut self, name: &'static str, value: f64) -> &mut Self {
        self.constants.push((name, value));
        self
    }

    /// Adds a static operation on the interface object.
    pub fn static_method<F>(&mut self, name: &'static str, length: u32, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, Args<'s>) -> JsResult<Value<'s>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.statics.push(MethodSpec { name, length, call: static_tramp::<F> });
        self
    }

    /// Makes the interface constructible. The callback receives `new.target`
    /// (for subclassing, e.g. custom elements) and returns the new object,
    /// typically via [`Scope::wrap_new`]. Without this, `new X()` throws
    /// "Illegal constructor".
    pub fn constructor<F>(&mut self, length: u32, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, Object<'s>, Args<'s>) -> JsResult<Object<'s>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.constructor = Some(ctor_tramp::<F>);
        self.ctor_length = length;
        self
    }

    /// Indexed property getter (`obj[0]`); return `None` for absent indices.
    pub fn indexed<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, u32) -> JsResult<Option<Value<'s>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.indexed = Some(indexed_erased::<C, F>);
        self
    }

    /// Named property getter (`collection.someId`, `storage.key`). Unless
    /// `override_builtins`, prototype members win over named properties.
    pub fn named<F>(&mut self, override_builtins: bool, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<Option<Value<'s>>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named = Some(named_erased::<C, F>);
        self.hooks.override_builtins = override_builtins;
        self
    }

    /// Named property setter; return `false` to fall back to an ordinary own property.
    pub fn named_setter<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str, Value<'s>) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_set = Some(named_set_erased::<C, F>);
        self
    }

    /// Named property deleter; return `false` when the name is not supported.
    pub fn named_deleter<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<bool> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.named_delete = Some(named_delete_erased::<C, F>);
        self
    }

    /// Supported property names for enumeration (`Object.keys`, `for..in`).
    pub fn property_names<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<String>> + Copy + 'static,
    {
        assert_zst::<F>();
        self.hooks.names = Some(names_erased::<C, F>);
        self
    }

    /// Value iterator over an indexed object with `length` (WebIDL
    /// `iterable<V>` on indexed properties): installs `Symbol.iterator`,
    /// `entries`, `keys`, `values` and `forEach` from `Array.prototype`.
    pub fn indexed_iterable(&mut self) -> &mut Self {
        self.indexed_iterable = true;
        self
    }

    /// Declares how to borrow the parent interface's data from this class's
    /// data (required when `Data` differs from the parent's `Data`).
    pub fn upcast<F>(&mut self, _f: F) -> &mut Self
    where
        F: Fn(&C::Data) -> &<C::Parent as JsClass>::Data + Copy + 'static,
    {
        assert_zst::<F>();
        self.upcast = Some(upcast_erased::<C, F>);
        self
    }

    /// Do not expose the interface object on the global (`[LegacyNoInterfaceObject]`-like).
    pub fn not_exposed(&mut self) -> &mut Self {
        self.exposed = false;
        self
    }
}

pub(crate) struct ClassInfo {
    pub upcast: Option<UpcastFn>,
    pub name: &'static str,
    pub cname: CString,
    pub data_type: TypeId,
    pub parent: Option<ClassId>,
    pub instance_class: sys::JSClassRef,
    pub global_class: Option<sys::JSClassRef>,
    pub def: ClassDefErased,
}

/// `ClassDef` without the marker type parameter.
pub(crate) struct ClassDefErased {
    pub methods: Vec<MethodSpec>,
    pub statics: Vec<MethodSpec>,
    pub accessors: Vec<AccessorSpec>,
    pub constants: Vec<(&'static str, f64)>,
    pub constructor: Option<RawFn>,
    pub ctor_length: u32,
    pub hooks: PropertyHooks,
    pub exposed: bool,
    pub indexed_iterable: bool,
}

pub(crate) struct ClassRegistry {
    pub classes: Vec<ClassInfo>,
    by_type: HashMap<TypeId, ClassId>,
    /// Parent JSClass of every wrapper class: carries the finalizer and marks
    /// objects whose private pointer is a `Header`.
    pub base: sys::JSClassRef,
    /// Callable objects backed by Rust closures.
    pub closure: sys::JSClassRef,
}

impl ClassRegistry {
    pub fn new() -> ClassRegistry {
        let base_name = c"LightpandaWrapper";
        let mut def = sys::JSClassDefinition::EMPTY;
        def.className = base_name.as_ptr();
        def.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        def.finalize = Some(finalize_wrapper);
        // SAFETY: static class name, valid definition.
        let base = unsafe { sys::JSClassCreate(&def) };
        let mut cdef = sys::JSClassDefinition::EMPTY;
        cdef.className = c"Function".as_ptr();
        cdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        cdef.parentClass = base;
        cdef.callAsFunction = Some(crate::realm::closure_call);
        let closure = unsafe { sys::JSClassCreate(&cdef) };
        ClassRegistry { classes: Vec::new(), by_type: HashMap::new(), base, closure }
    }

    pub fn id_of<C: JsClass>(&self) -> Option<ClassId> {
        self.by_type.get(&TypeId::of::<C>()).copied()
    }

    pub fn is_subclass(&self, mut class: ClassId, ancestor: ClassId) -> bool {
        loop {
            if class == ancestor {
                return true;
            }
            match self.classes.get(class as usize).and_then(|c| c.parent) {
                Some(p) => class = p,
                None => return false,
            }
        }
    }

    /// Nearest class in the chain (self first) with property hooks.
    pub fn hooks_of(&self, mut class: ClassId) -> Option<&PropertyHooks> {
        loop {
            let info = self.classes.get(class as usize)?;
            let h = &info.def.hooks;
            if h.indexed.is_some() || h.named.is_some() {
                return Some(h);
            }
            class = info.parent?;
        }
    }

    /// Lazily creates the JSClass used when `class` is a realm's global object.
    pub fn global_class(&mut self, class: ClassId) -> sys::JSClassRef {
        let base = self.base;
        let info = &mut self.classes[class as usize];
        *info.global_class.get_or_insert_with(|| {
            let mut def = sys::JSClassDefinition::EMPTY;
            def.className = info.cname.as_ptr();
            // Automatic prototype: the global's [[Prototype]] is immutable in
            // JSC, so this generated object becomes the interface prototype.
            def.parentClass = base;
            unsafe { sys::JSClassCreate(&def) }
        })
    }

    pub fn release_all(&mut self) {
        unsafe {
            for c in &self.classes {
                sys::JSClassRelease(c.instance_class);
                if let Some(g) = c.global_class {
                    sys::JSClassRelease(g);
                }
            }
            sys::JSClassRelease(self.closure);
            sys::JSClassRelease(self.base);
        }
        self.classes.clear();
    }
}

pub(crate) fn register<C: JsClass>(engine: &EngineInner) -> ClassId {
    if let Some(id) = engine.classes.borrow().id_of::<C>() {
        return id;
    }
    let parent = if TypeId::of::<C::Parent>() == TypeId::of::<NoParent>() {
        None
    } else {
        let pid = register::<C::Parent>(engine);
        let reg = engine.classes.borrow();
        let same = reg.classes[pid as usize].data_type == TypeId::of::<C::Data>();
        drop(reg);
        Some((pid, same))
    };
    let mut def = ClassDef::<C>::new();
    C::define(&mut def);
    let parent = match parent {
        Some((pid, same)) => {
            assert!(
                same || def.upcast.is_some(),
                "{}: Data differs from parent {} Data; declare ClassDef::upcast",
                C::NAME,
                C::Parent::NAME
            );
            Some(pid)
        }
        None => None,
    };
    let mut reg = engine.classes.borrow_mut();
    let id = reg.classes.len() as ClassId;
    let cname = CString::new(C::NAME).expect("class name without NUL");
    let has_hooks =
        def.hooks.indexed.is_some() || def.hooks.named.is_some() || parent.is_some_and(|p| reg.hooks_of(p).is_some());
    let mut jsdef = sys::JSClassDefinition::EMPTY;
    jsdef.className = cname.as_ptr();
    jsdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
    jsdef.parentClass = reg.base;
    if has_hooks {
        jsdef.getProperty = Some(get_property_hook);
        jsdef.setProperty = Some(set_property_hook);
        jsdef.deleteProperty = Some(delete_property_hook);
        jsdef.getPropertyNames = Some(property_names_hook);
    }
    // SAFETY: valid definition; cname outlives the class (stored in ClassInfo).
    let instance_class = unsafe { sys::JSClassCreate(&jsdef) };
    reg.classes.push(ClassInfo {
        upcast: def.upcast,
        name: C::NAME,
        cname,
        data_type: TypeId::of::<C::Data>(),
        parent,
        instance_class,
        global_class: None,
        def: ClassDefErased {
            methods: def.methods,
            statics: def.statics,
            accessors: def.accessors,
            constants: def.constants,
            constructor: def.constructor,
            ctor_length: def.ctor_length,
            hooks: def.hooks,
            exposed: def.exposed,
            indexed_iterable: def.indexed_iterable,
        },
    });
    reg.by_type.insert(TypeId::of::<C>(), id);
    id
}

// ---------------------------------------------------------------------------
// Wrapper payloads

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Instance,
    Closure,
}

/// Header at the start of every wrapper payload (`Boxed<T>`).
#[repr(C)]
pub(crate) struct Header {
    pub class: ClassId,
    pub kind: Kind,
    pub realm: RealmId,
    pub key: Option<IdentityKey>,
    pub data_type: TypeId,
    pub drop_fn: unsafe fn(*mut Header),
    pub data_ptr: unsafe fn(*mut Header) -> *const u8,
}

#[repr(C)]
pub(crate) struct Boxed<T> {
    pub header: Header,
    pub data: T,
}

unsafe fn data_ptr_boxed<T>(h: *mut Header) -> *const u8 {
    // SAFETY: h points to a live Boxed<T>.
    unsafe { &(*(h as *mut Boxed<T>)).data as *const T as *const u8 }
}

unsafe fn drop_boxed<T>(h: *mut Header) {
    // SAFETY: h was produced by Box::into_raw(Box<Boxed<T>>) (see new_payload).
    drop(unsafe { Box::from_raw(h as *mut Boxed<T>) });
}

pub(crate) fn new_payload<T: 'static>(
    class: ClassId,
    kind: Kind,
    realm: RealmId,
    key: Option<IdentityKey>,
    data: T,
) -> *mut Header {
    let b = Box::new(Boxed {
        header: Header {
            class,
            kind,
            realm,
            key,
            data_type: TypeId::of::<T>(),
            drop_fn: drop_boxed::<T>,
            data_ptr: data_ptr_boxed::<T>,
        },
        data,
    });
    Box::into_raw(b) as *mut Header
}

/// Borrows an instance payload as the data of `want` (the payload's class or an
/// ancestor), following declared upcasts.
///
/// # Safety
/// `h` must be a live instance payload header whose class is registered in `reg`.
pub(crate) unsafe fn class_data<'a, T: 'static>(reg: &ClassRegistry, h: *mut Header, want: ClassId) -> Option<&'a T> {
    unsafe {
        let mut class = (*h).class;
        if !reg.is_subclass(class, want) {
            return None;
        }
        let mut ptr = ((*h).data_ptr)(h);
        while class != want {
            let info = reg.classes.get(class as usize)?;
            if let Some(up) = info.upcast {
                ptr = up(ptr);
            }
            class = info.parent?;
        }
        if reg.classes.get(want as usize)?.data_type != TypeId::of::<T>() {
            return None;
        }
        Some(&*(ptr as *const T))
    }
}

unsafe fn upcast_erased<C, F>(p: *const u8) -> *const u8
where
    C: JsClass,
    F: Fn(&C::Data) -> &<C::Parent as JsClass>::Data + Copy + 'static,
{
    let f = unsafe { conjure::<F>() };
    // SAFETY: p points to a live C::Data (see class_data).
    f(unsafe { &*(p as *const C::Data) }) as *const _ as *const u8
}

/// # Safety
/// `h` must be a live payload header.
pub(crate) unsafe fn payload_data<'a, T: 'static>(h: *mut Header) -> Option<&'a T> {
    unsafe {
        if (*h).data_type != TypeId::of::<T>() {
            return None;
        }
        Some(&(*(h as *mut Boxed<T>)).data)
    }
}

thread_local! {
    static FINALIZED: RefCell<Vec<*mut Header>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn finalize_wrapper(object: sys::JSObjectRef) {
    // Runs inside GC: no JSC calls besides GetPrivate, no Rust destructors.
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if h.is_null() {
        return;
    }
    let pushed = FINALIZED.try_with(|q| match q.try_borrow_mut() {
        Ok(mut q) => {
            q.push(h);
            true
        }
        Err(_) => false,
    });
    // Only possible during thread teardown: leak rather than run destructors in GC.
    let _ = pushed;
}

/// Drops payloads of finalized wrappers and prunes dead identity-map entries.
pub(crate) fn drain_finalized(engine: Option<&EngineInner>) -> usize {
    let mut total = 0;
    loop {
        let batch = FINALIZED.try_with(|q| std::mem::take(&mut *q.borrow_mut())).unwrap_or_default();
        if batch.is_empty() {
            return total;
        }
        total += batch.len();
        for h in batch {
            // SAFETY: each pointer was enqueued exactly once by finalize_wrapper
            // and its JS object is dead, so we are the sole owner.
            unsafe {
                if let (Some(engine), Some(key)) = (engine, (*h).key) {
                    if let Some(realm) =
                        engine.realms.borrow().iter().filter_map(|(_, r)| r.upgrade()).find(|r| r.id == (*h).realm)
                    {
                        realm.prune_identity(key);
                    }
                }
                ((*h).drop_fn)(h);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Trampolines

/// Runs a callback body, converting errors and panics into JS exceptions.
#[inline]
unsafe fn guard<R>(
    ctx: sys::JSContextRef,
    exc: *mut sys::JSValueRef,
    fallback: R,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> R {
    let realm = match crate::realm::realm_for_ctx(ctx) {
        Some(r) => r,
        None => {
            unsafe { throw_message(ctx, "Execution context was destroyed.", exc) };
            return fallback;
        }
    };
    let scope = Scope::new(ctx, &realm);
    match catch_unwind(AssertUnwindSafe(|| body(&scope))) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            unsafe { throw_into(&scope, &e, exc) };
            fallback
        }
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            unsafe { throw_message(ctx, &format!("Internal error: {msg}"), exc) };
            fallback
        }
    }
}

/// Resolves `this` to class `C`'s data: wrappers of `C` or a subclass, or the
/// realm's global object (also for `undefined`/`null` receivers).
unsafe fn this_data<'a, C: JsClass>(scope: &Scope<'_>, this: sys::JSObjectRef) -> JsResult<&'a C::Data> {
    let h = unsafe { scope.header_of_receiver(this) };
    let Some(h) = h else {
        return Err(JsError::Type("Illegal invocation".into()));
    };
    let reg = scope.engine_inner().classes.borrow();
    let want = reg.id_of::<C>().expect("class registered");
    // SAFETY: h is a live payload of a reachable object (receiver on stack).
    unsafe {
        if (*h).kind != Kind::Instance {
            return Err(JsError::Type("Illegal invocation".into()));
        }
        class_data::<C::Data>(&reg, h, want).ok_or_else(|| JsError::Type("Illegal invocation".into()))
    }
}

unsafe extern "C" fn method_tramp<C, F>(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, Args<'s>) -> JsResult<Value<'s>> + Copy + 'static,
{
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let data = this_data::<C>(scope, this)?;
            let args = Args::from_raw(ctx, argc, argv);
            conjure::<F>()(scope, data, args).map(|v| v.raw)
        })
    }
}

unsafe extern "C" fn getter_tramp<C, G>(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    _argc: usize,
    _argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef
where
    C: JsClass,
    G: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Value<'s>> + Copy + 'static,
{
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let data = this_data::<C>(scope, this)?;
            conjure::<G>()(scope, data).map(|v| v.raw)
        })
    }
}

unsafe extern "C" fn setter_tramp<C, S>(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef
where
    C: JsClass,
    S: for<'s> Fn(&Scope<'s>, &C::Data, Value<'s>) -> JsResult<()> + Copy + 'static,
{
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let data = this_data::<C>(scope, this)?;
            let v = Args::from_raw(ctx, argc, argv).get(0);
            conjure::<S>()(scope, data, v)?;
            Ok(sys::JSValueMakeUndefined(ctx))
        })
    }
}

unsafe extern "C" fn static_tramp<F>(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef
where
    F: for<'s> Fn(&Scope<'s>, Args<'s>) -> JsResult<Value<'s>> + Copy + 'static,
{
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let args = Args::from_raw(ctx, argc, argv);
            conjure::<F>()(scope, args).map(|v| v.raw)
        })
    }
}

/// Called by the realm's JS constructor shim as `native(newTarget, argsArray)`.
unsafe extern "C" fn ctor_tramp<F>(
    ctx: sys::JSContextRef,
    _function: sys::JSObjectRef,
    _this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef
where
    F: for<'s> Fn(&Scope<'s>, Object<'s>, Args<'s>) -> JsResult<Object<'s>> + Copy + 'static,
{
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let a = Args::from_raw(ctx, argc, argv);
            let new_target = a.get(0).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
            let list = a.get(1).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
            let len = list.get("length")?.to_number()? as usize;
            // Arguments stay reachable through `list` (a live argument array).
            let mut raw = Vec::with_capacity(len);
            for i in 0..len {
                raw.push(list.get_index(i as u32)?.raw);
            }
            let args = Args { ctx, raw: &raw };
            conjure::<F>()(scope, new_target, args).map(|o| o.raw as sys::JSValueRef)
        })
    }
}

unsafe fn indexed_erased<C, F>(scope: &Scope<'_>, h: *mut Header, index: u32) -> JsResult<Option<sys::JSValueRef>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, u32) -> JsResult<Option<Value<'s>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data, index)?.map(|v| v.raw))
}

unsafe fn named_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &str) -> JsResult<Option<sys::JSValueRef>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<Option<Value<'s>>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    Ok(f(scope, data, name)?.map(|v| v.raw))
}

unsafe fn named_set_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &str, v: sys::JSValueRef) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str, Value<'s>) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    unsafe { conjure::<F>()(scope, data, name, Value::from_raw(scope.ctx, v)) }
}

unsafe fn named_delete_erased<C, F>(scope: &Scope<'_>, h: *mut Header, name: &str) -> JsResult<bool>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data, &str) -> JsResult<bool> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    f(scope, data, name)
}

unsafe fn names_erased<C, F>(scope: &Scope<'_>, h: *mut Header) -> JsResult<Vec<String>>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>, &C::Data) -> JsResult<Vec<String>> + Copy + 'static,
{
    let data = unsafe { hook_data::<C>(scope, h) }?;
    let f = unsafe { conjure::<F>() };
    f(scope, data)
}

/// Data of class `C` for a property-hook receiver (hooks may be inherited).
unsafe fn hook_data<'a, C: JsClass>(scope: &Scope<'_>, h: *mut Header) -> JsResult<&'a C::Data> {
    let reg = scope.engine_inner().classes.borrow();
    let want = reg.id_of::<C>().expect("class registered");
    unsafe { class_data::<C::Data>(&reg, h, want) }.ok_or_else(|| JsError::Type("Illegal invocation".into()))
}

/// Parses a canonical array index ("0", "17"; not "01", "-1", "4294967295").
fn array_index(name: &[u16]) -> Option<u32> {
    if name.is_empty() || name.len() > 10 || (name.len() > 1 && name[0] == b'0' as u16) {
        return None;
    }
    let mut n: u64 = 0;
    for &c in name {
        if !(b'0' as u16..=b'9' as u16).contains(&c) {
            return None;
        }
        n = n * 10 + (c - b'0' as u16) as u64;
    }
    (n < u32::MAX as u64).then_some(n as u32)
}

/// Shared prologue of the property hooks: realm scope, payload header and the
/// class's hooks (copied out so no registry borrow is held during callbacks).
struct HookCtx {
    header: *mut Header,
    indexed: Option<IndexedFn>,
    named: Option<NamedFn>,
    override_builtins: bool,
    named_set: Option<NamedSetFn>,
    named_delete: Option<NamedDeleteFn>,
    names: Option<NamesFn>,
}

unsafe fn hook_ctx(scope: &Scope<'_>, object: sys::JSObjectRef) -> Option<HookCtx> {
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if h.is_null() {
        return None;
    }
    let reg = scope.engine_inner().classes.borrow();
    let hooks = reg.hooks_of(unsafe { (*h).class })?;
    Some(HookCtx {
        header: h,
        indexed: hooks.indexed,
        named: hooks.named,
        override_builtins: hooks.override_builtins,
        named_set: hooks.named_set,
        named_delete: hooks.named_delete,
        names: hooks.names,
    })
}

/// True if `name` resolves on the object's prototype chain (named properties
/// must not shadow prototype members without `[LegacyOverrideBuiltIns]`).
unsafe fn on_prototype_chain(ctx: sys::JSContextRef, object: sys::JSObjectRef, name: sys::JSStringRef) -> bool {
    unsafe {
        let proto = sys::JSObjectGetPrototype(ctx, object);
        sys::JSValueIsObject(ctx, proto) && sys::JSObjectHasProperty(ctx, proto as sys::JSObjectRef, name)
    }
}

unsafe extern "C" fn get_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(ptr::null()) };
            let units = utf16_of(name);
            if let Some(idx) = array_index(units) {
                if let Some(f) = hc.indexed {
                    return Ok(f(scope, hc.header, idx)?.unwrap_or(ptr::null()));
                }
            }
            if let Some(f) = hc.named {
                if !hc.override_builtins && on_prototype_chain(ctx, object, name) {
                    return Ok(ptr::null());
                }
                let n = String::from_utf16_lossy(units);
                return Ok(f(scope, hc.header, &n)?.unwrap_or(ptr::null()));
            }
            Ok(ptr::null())
        })
    }
}

unsafe extern "C" fn set_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    value: sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> bool {
    unsafe {
        guard(ctx, exc, false, |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(false) };
            let units = utf16_of(name);
            if hc.indexed.is_some() && array_index(units).is_some() && hc.named_set.is_none() {
                // Indexed properties without a setter are read-only: ignore (sloppy) writes.
                return Ok(true);
            }
            if let Some(f) = hc.named_set {
                if !hc.override_builtins && on_prototype_chain(ctx, object, name) {
                    return Ok(false);
                }
                return f(scope, hc.header, &String::from_utf16_lossy(units), value);
            }
            Ok(false)
        })
    }
}

unsafe extern "C" fn delete_property_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> bool {
    unsafe {
        guard(ctx, exc, false, |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(false) };
            match hc.named_delete {
                Some(f) => f(scope, hc.header, &String::from_utf16_lossy(utf16_of(name))),
                None => Ok(false),
            }
        })
    }
}

unsafe extern "C" fn property_names_hook(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    acc: sys::JSPropertyNameAccumulatorRef,
) {
    unsafe {
        guard(ctx, ptr::null_mut(), (), |scope| {
            let Some(hc) = hook_ctx(scope, object) else { return Ok(()) };
            if hc.indexed.is_some() {
                let len = Object::from_raw(ctx, object).get("length")?;
                if len.is_number() {
                    for i in 0..len.to_number()? as u32 {
                        sys::JSPropertyNameAccumulatorAddName(acc, JsString::new(&i.to_string()).as_raw());
                    }
                }
            }
            if let Some(f) = hc.names {
                for n in f(scope, hc.header)? {
                    sys::JSPropertyNameAccumulatorAddName(acc, JsString::new(&n).as_raw());
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::array_index;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn array_index_parsing() {
        assert_eq!(array_index(&u("0")), Some(0));
        assert_eq!(array_index(&u("42")), Some(42));
        assert_eq!(array_index(&u("4294967294")), Some(4294967294));
        assert_eq!(array_index(&u("4294967295")), None);
        assert_eq!(array_index(&u("01")), None);
        assert_eq!(array_index(&u("-1")), None);
        assert_eq!(array_index(&u("length")), None);
        assert_eq!(array_index(&u("")), None);
    }
}
