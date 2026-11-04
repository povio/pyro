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
type IndexedFn = unsafe fn(&Scope<'_>, *mut Header, u32) -> JsResult<Option<sys::JSValueRef>>;
type NamedFn = unsafe fn(&Scope<'_>, *mut Header, &str) -> JsResult<Option<sys::JSValueRef>>;
type NamedSetFn = unsafe fn(&Scope<'_>, *mut Header, &str, sys::JSValueRef) -> JsResult<bool>;
type NamedDeleteFn = unsafe fn(&Scope<'_>, *mut Header, &str) -> JsResult<bool>;
type NamesFn = unsafe fn(&Scope<'_>, *mut Header) -> JsResult<Vec<String>>;

pub(crate) struct MethodSpec {
    pub name: &'static str,
    pub length: u32,
    pub call: MemberCall,
}

pub(crate) struct AccessorSpec {
    pub name: &'static str,
    pub get: MemberCall,
    pub set: Option<MemberCall>,
}

/// A member's Rust callback, type-erased, plus the invoker that knows its
/// real type. One invoker exists per class and member kind (not per member),
/// which keeps the number of monomorphized functions small: lp-webapi has
/// ~1,000 members, and one trampoline each made its codegen serial.
#[derive(Clone, Copy)]
pub(crate) struct MemberCall {
    pub invoke: InvokeFn,
    pub f: *const (),
}

/// Calls `f` (a member callback, cast back to its real fn-pointer type) for
/// `this` with `args`.
pub(crate) type InvokeFn = unsafe fn(&Scope<'_>, sys::JSObjectRef, Args<'_>, *const ()) -> JsResult<sys::JSValueRef>;

/// Member callback types: plain fn pointers. Non-capturing closures coerce.
pub type MethodFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Args<'s>) -> JsResult<Value<'s>>;
pub type GetterFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data) -> JsResult<Value<'s>>;
pub type SetterFn<C> = for<'s> fn(&Scope<'s>, &<C as JsClass>::Data, Value<'s>) -> JsResult<()>;
pub type StaticFn = for<'s> fn(&Scope<'s>, Args<'s>) -> JsResult<Value<'s>>;
pub type CtorFn = for<'s> fn(&Scope<'s>, Object<'s>, Args<'s>) -> JsResult<Object<'s>>;

#[derive(Default, Clone, Copy)]
pub(crate) struct PropertyHooks {
    pub indexed: Option<IndexedFn>,
    pub named: Option<NamedFn>,
    /// `[LegacyOverrideBuiltIns]`: named properties shadow prototype members.
    pub override_builtins: bool,
    pub named_set: Option<NamedSetFn>,
    pub named_delete: Option<NamedDeleteFn>,
    pub names: Option<NamesFn>,
}

impl PropertyHooks {
    fn any(&self) -> bool {
        self.indexed.is_some() || self.named.is_some() || self.named_set.is_some() || self.named_delete.is_some()
    }

    /// Own hooks override inherited ones per kind.
    fn inherit(self, parent: &PropertyHooks) -> PropertyHooks {
        let named_own = self.named.is_some();
        PropertyHooks {
            indexed: self.indexed.or(parent.indexed),
            named: self.named.or(parent.named),
            override_builtins: if named_own { self.override_builtins } else { parent.override_builtins },
            named_set: self.named_set.or(parent.named_set),
            named_delete: self.named_delete.or(parent.named_delete),
            names: self.names.or(parent.names),
        }
    }
}

/// Builder passed to [`JsClass::define`].
pub struct ClassDef<C: JsClass> {
    pub(crate) methods: Vec<MethodSpec>,
    pub(crate) statics: Vec<MethodSpec>,
    pub(crate) accessors: Vec<AccessorSpec>,
    pub(crate) constants: Vec<(&'static str, f64)>,
    pub(crate) constructor: Option<MemberCall>,
    pub(crate) ctor_length: u32,
    pub(crate) hooks: PropertyHooks,
    pub(crate) exposed: bool,
    pub(crate) indexed_iterable: bool,
    pub(crate) upcast: Option<UpcastFn>,
    pub(crate) global_receiver: Option<GlobalReceiverFn>,
    _c: PhantomData<C>,
}

/// Resolves the data an operation uses when called with an undefined, null or
/// global receiver (see [`ClassDef::global_receiver`]).
pub(crate) type GlobalReceiverFn = unsafe fn(&Scope<'_>) -> Option<*const u8>;

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
            global_receiver: None,
            _c: PhantomData,
        }
    }

    /// Adds an operation on the prototype.
    pub fn method(&mut self, name: &'static str, length: u32, f: MethodFn<C>) -> &mut Self {
        self.methods.push(MethodSpec {
            name,
            length,
            call: MemberCall { invoke: method_invoke::<C>, f: f as *const () },
        });
        self
    }

    /// Adds a read-only attribute.
    pub fn getter(&mut self, name: &'static str, g: GetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        self.accessors.push(AccessorSpec { name, get, set: None });
        self
    }

    /// Adds a read-write attribute.
    pub fn accessor(&mut self, name: &'static str, g: GetterFn<C>, s: SetterFn<C>) -> &mut Self {
        let get = MemberCall { invoke: getter_invoke::<C>, f: g as *const () };
        let set = MemberCall { invoke: setter_invoke::<C>, f: s as *const () };
        self.accessors.push(AccessorSpec { name, get, set: Some(set) });
        self
    }

    /// Adds a constant on both the interface object and the prototype.
    pub fn constant(&mut self, name: &'static str, value: f64) -> &mut Self {
        self.constants.push((name, value));
        self
    }

    /// Adds a static operation on the interface object.
    pub fn static_method(&mut self, name: &'static str, length: u32, f: StaticFn) -> &mut Self {
        self.statics.push(MethodSpec { name, length, call: MemberCall { invoke: static_invoke, f: f as *const () } });
        self
    }

    /// Makes the interface constructible. The callback receives `new.target`
    /// (for subclassing, e.g. custom elements) and returns the new object,
    /// typically via [`Scope::wrap_new`]. Without this, `new X()` throws
    /// "Illegal constructor".
    pub fn constructor(&mut self, length: u32, f: CtorFn) -> &mut Self {
        self.constructor = Some(MemberCall { invoke: ctor_invoke, f: f as *const () });
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

    /// Resolves the instance used when an operation or attribute of this class
    /// is invoked with an `undefined`/`null`/global receiver, e.g.
    /// `(cond ? history.pushState : history.replaceState)(...)`, which
    /// browsers accept and real libraries (TanStack History) rely on. `F`
    /// returns the realm's singleton (typically from the global's data).
    pub fn global_receiver<F>(&mut self, _f: F) -> &mut Self
    where
        F: for<'s> Fn(&Scope<'s>) -> Option<&'s C::Data> + Copy + 'static,
    {
        assert_zst::<F>();
        self.global_receiver = Some(global_receiver_erased::<C, F>);
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
    pub global_receiver: Option<GlobalReceiverFn>,
    pub name: &'static str,
    pub data_type: TypeId,
    pub parent: Option<ClassId>,
    pub instance_class: sys::JSClassRef,
    pub def: ClassDefErased,
}

/// `ClassDef` without the marker type parameter.
pub(crate) struct ClassDefErased {
    pub methods: Vec<MethodSpec>,
    pub statics: Vec<MethodSpec>,
    pub accessors: Vec<AccessorSpec>,
    pub constants: Vec<(&'static str, f64)>,
    pub constructor: Option<MemberCall>,
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
    /// Interface member functions (methods, accessors, statics, constructors):
    /// callable objects whose private data is a `MemberCall`. Not a `base`
    /// subclass: their private pointer is not a `Header`.
    pub member: sys::JSClassRef,
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
        let mut mdef = sys::JSClassDefinition::EMPTY;
        mdef.className = c"Function".as_ptr();
        mdef.attributes = sys::kJSClassAttributeNoAutomaticPrototype;
        mdef.callAsFunction = Some(member_call);
        mdef.finalize = Some(member_finalize);
        let member = unsafe { sys::JSClassCreate(&mdef) };
        ClassRegistry { classes: Vec::new(), by_type: HashMap::new(), base, closure, member }
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

    /// Effective property hooks of `class` (each kind inherited separately).
    pub fn hooks_of(&self, class: ClassId) -> Option<&PropertyHooks> {
        let h = &self.classes.get(class as usize)?.def.hooks;
        h.any().then_some(h)
    }

    pub fn release_all(&mut self) {
        unsafe {
            for c in &self.classes {
                sys::JSClassRelease(c.instance_class);
            }
            sys::JSClassRelease(self.closure);
            sys::JSClassRelease(self.member);
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
    if let Some(p) = parent {
        let inherited = reg.classes[p as usize].def.hooks;
        def.hooks = def.hooks.inherit(&inherited);
    }
    let has_hooks = def.hooks.any();
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
    // SAFETY: valid definition; JSClassCreate copies the class name.
    let instance_class = unsafe { sys::JSClassCreate(&jsdef) };
    reg.classes.push(ClassInfo {
        upcast: def.upcast,
        global_receiver: def.global_receiver,
        name: C::NAME,
        data_type: TypeId::of::<C::Data>(),
        parent,
        instance_class,
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

/// First word of every object private payload in the VM: wrapper payloads
/// ([`Header`]) and member functions ([`MemberPayload`]). Lets
/// `Scope::header_of_object` tell a wrapper from a member function by reading
/// its private pointer, without the class check (which takes the JSC lock).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum PayloadTag {
    Wrapper = 0x4c50_5752_4150_5052, // "LPWRAPPR"
    Member = 0x4c50_4d45_4d42_4552,  // "LPMEMBER"
}

/// Private payload of a member function object.
#[repr(C)]
pub(crate) struct MemberPayload {
    pub tag: PayloadTag,
    pub call: MemberCall,
}

/// Header at the start of every wrapper payload (`Boxed<T>`).
#[repr(C)]
pub(crate) struct Header {
    pub tag: PayloadTag,
    pub class: ClassId,
    pub kind: Kind,
    pub realm: RealmId,
    pub key: Option<IdentityKey>,
    pub data_type: TypeId,
    pub drop_fn: unsafe fn(*mut Header),
    pub data_ptr: unsafe fn(*mut Header) -> *const u8,
    /// Owning engine's finalization queue (outlives every payload: the VM is
    /// destroyed, finalizing all objects, before the queue is dropped).
    pub queue: *const FinalizeQueue,
}

/// Payload headers awaiting destruction on the engine thread. Finalizers may
/// run on any thread, hence the mutex.
#[derive(Default)]
pub(crate) struct FinalizeQueue(std::sync::Mutex<Vec<usize>>);

impl FinalizeQueue {
    fn push(&self, h: *mut Header) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(h as usize);
    }
    fn take(&self) -> Vec<usize> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
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
    engine: &EngineInner,
    class: ClassId,
    kind: Kind,
    realm: RealmId,
    key: Option<IdentityKey>,
    data: T,
) -> *mut Header {
    let b = Box::new(Boxed {
        header: Header {
            tag: PayloadTag::Wrapper,
            class,
            kind,
            realm,
            key,
            data_type: TypeId::of::<T>(),
            drop_fn: drop_boxed::<T>,
            data_ptr: data_ptr_boxed::<T>,
            queue: std::sync::Arc::as_ptr(&engine.finalized),
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

unsafe extern "C" fn finalize_wrapper(object: sys::JSObjectRef) {
    // Runs inside GC, possibly on a JSC helper thread: no JSC calls besides
    // GetPrivate, no Rust destructors. The header is immutable after creation.
    let h = unsafe { sys::JSObjectGetPrivate(object) } as *mut Header;
    if h.is_null() {
        return;
    }
    unsafe { (*(*h).queue).push(h) };
}

/// Drops payloads of finalized wrappers and prunes dead identity-map entries.
/// Engine thread only, outside GC.
pub(crate) fn drain_finalized(engine: &EngineInner) -> usize {
    let mut total = 0;
    loop {
        let batch = engine.finalized.take();
        if batch.is_empty() {
            return total;
        }
        total += batch.len();
        for h in batch {
            let h = h as *mut Header;
            // SAFETY: each pointer was enqueued exactly once by finalize_wrapper
            // and its JS object is dead, so we are the sole owner.
            unsafe {
                if let Some(key) = (*h).key {
                    let realm =
                        engine.realms.borrow().iter().filter_map(|(_, r)| r.upgrade()).find(|r| r.id == (*h).realm);
                    if let Some(realm) = realm {
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

/// Result shapes used by JSC native callbacks. A raw value needs an explicit
/// temporary root before the scheduling hook can allocate or collect garbage.
pub(crate) trait CallbackResult {
    fn root(&self, _scope: &Scope<'_>) -> Option<crate::Persistent> {
        None
    }
}
impl CallbackResult for bool {}
impl CallbackResult for () {}
impl CallbackResult for sys::JSValueRef {
    fn root(&self, scope: &Scope<'_>) -> Option<crate::Persistent> {
        if self.is_null() {
            None
        } else {
            // SAFETY: this is the value just returned by the native callback.
            Some(crate::Persistent::new(scope, unsafe { Value::from_raw(scope.ctx, *self) }))
        }
    }
}

/// Runs a callback body, converting errors and panics into JS exceptions.
/// Realm lookup, the body, error conversion and the release of the realm
/// handle (which may dispose it and call hooks) all happen inside the unwind
/// boundary.
///
/// Every trampoline instantiates this once per callback (thousands of times),
/// so it only adapts `body` to [`guard_erased`], which holds the logic once.
#[inline]
pub(crate) unsafe fn guard<R: CallbackResult>(
    ctx: sys::JSContextRef,
    exc: *mut sys::JSValueRef,
    fallback: R,
    body: impl FnOnce(&Scope<'_>) -> JsResult<R>,
) -> R {
    let mut body = Some(body);
    let mut out = None;
    let mut step = |scope: &Scope<'_>, step: Step| match step {
        Step::Run => {
            out = Some(body.take().expect("callback body runs once")(scope)?);
            Ok(None)
        }
        Step::Root => Ok(out.as_ref().and_then(|v| v.root(scope))),
    };
    match unsafe { guard_erased(ctx, exc, &mut step) } {
        true => out.expect("successful callback result"),
        false => fallback,
    }
}

/// What [`guard_erased`] asks of the typed callback.
enum Step {
    /// Run the body, keeping its result.
    Run,
    /// Root the kept result (see [`CallbackResult::root`]).
    Root,
}

/// The body of [`guard`]; returns whether the callback's result stands.
/// Dev builds keep one copy. Release builds inline it so the `dyn` call
/// disappears from the JS-to-native hot path.
#[cfg_attr(not(debug_assertions), inline(always))]
unsafe fn guard_erased(
    ctx: sys::JSContextRef,
    exc: *mut sys::JSValueRef,
    step: &mut dyn FnMut(&Scope<'_>, Step) -> JsResult<Option<crate::Persistent>>,
) -> bool {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _depth = DepthGuard::enter();
        let Some(realm) = crate::realm::realm_for_ctx(ctx) else {
            unsafe { throw_message(ctx, "Execution context was destroyed.", exc) };
            return false;
        };
        let scope = Scope::new(ctx, &realm);
        let result = step(&scope, Step::Run);
        let hook = if matches!(result, Err(JsError::Terminated)) || crate::engine::TERMINATING.with(|t| t.get()) {
            None
        } else {
            realm.native_callback_hook()
        };
        // Error::Thrown already owns a Persistent. Other errors materialize only
        // after the hook. Keep successful JS results protected until returning.
        let _return_root = match (&hook, &result) {
            (Some(_), Ok(_)) => step(&scope, Step::Root).ok().flatten(),
            _ => None,
        };
        if let Some(hook) = hook {
            realm.run_native_callback_hook(&scope, hook);
        }
        // Native code that ignored an inner termination must not let the
        // calling JS continue: treat the callback as terminated as well.
        let result = match result {
            Ok(_) if crate::engine::TERMINATING.with(|t| t.get()) => Err(JsError::Terminated),
            r => r,
        };
        match result {
            Ok(_) => true,
            Err(JsError::Terminated) => {
                // Keep terminating: page code may catch the rethrown value.
                crate::engine::rearm_termination(ctx);
                unsafe { throw_message_value(ctx, "JavaScript execution terminated.", exc) };
                false
            }
            Err(e) => {
                unsafe { throw_into(&scope, &e, exc) };
                false
            }
        }
    }));
    match outcome {
        Ok(stands) => stands,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            // Creating the error cannot unwind (no Rust callbacks involved).
            unsafe { throw_message(ctx, &format!("Internal error: {msg}"), exc) };
            false
        }
    }
}

/// Tracks native-callback nesting (see `engine::DEPTH`).
pub(crate) struct DepthGuard;

impl DepthGuard {
    pub(crate) fn enter() -> DepthGuard {
        crate::engine::DEPTH.with(|d| d.set(d.get() + 1));
        DepthGuard
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        let _ = crate::engine::DEPTH.try_with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Throws a primitive string (used to rethrow termination unchanged).
unsafe fn throw_message_value(ctx: sys::JSContextRef, msg: &str, exc: *mut sys::JSValueRef) {
    if !exc.is_null() {
        let s = JsString::new(msg);
        unsafe { *exc = sys::JSValueMakeString(ctx, s.as_raw()) };
    }
}

/// Resolves `this` to class `C`'s data: wrappers of `C` or a subclass, or the
/// realm's global object (also for `undefined`/`null` receivers).
unsafe fn this_data<'a, C: JsClass>(scope: &Scope<'_>, this: sys::JSObjectRef) -> JsResult<&'a C::Data> {
    let h = unsafe { scope.header_of_receiver(this) };
    let reg = scope.engine_inner().classes.borrow();
    let want = reg.id_of::<C>().expect("class registered");
    // SAFETY: h is a live payload of a reachable object (receiver on stack).
    let direct = h.and_then(|h| unsafe {
        if (*h).kind != Kind::Instance {
            None
        } else {
            class_data::<C::Data>(&reg, h, want)
        }
    });
    if let Some(d) = direct {
        return Ok(d);
    }
    let fallback = reg.classes[want as usize].global_receiver;
    drop(reg);
    if let Some(f) = fallback {
        // SAFETY: only for receivers that resolve to the realm's global.
        let global_receiver = this.is_null() || this == scope.global_object_raw();
        if global_receiver {
            if let Some(p) = unsafe { f(scope) } {
                // SAFETY: the pointer refers to C::Data owned by the realm's
                // global payload, which outlives this callback.
                return Ok(unsafe { &*(p as *const C::Data) });
            }
        }
    }
    Err(JsError::Type("Illegal invocation".into()))
}

unsafe fn global_receiver_erased<C, F>(scope: &Scope<'_>) -> Option<*const u8>
where
    C: JsClass,
    F: for<'s> Fn(&Scope<'s>) -> Option<&'s C::Data> + Copy + 'static,
{
    let f = unsafe { conjure::<F>() };
    f(scope).map(|d| d as *const C::Data as *const u8)
}

/// `callAsFunction` of every member function object: the object's private
/// data is its `MemberCall`.
pub(crate) unsafe extern "C" fn member_call(
    ctx: sys::JSContextRef,
    function: sys::JSObjectRef,
    this: sys::JSObjectRef,
    argc: usize,
    argv: *const sys::JSValueRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            let payload = sys::JSObjectGetPrivate(function) as *const MemberPayload;
            if payload.is_null() {
                return Err(JsError::Type("Illegal invocation".into()));
            }
            let call = (*payload).call;
            (call.invoke)(scope, this, Args::from_raw(ctx, argc, argv), call.f)
        })
    }
}

/// Frees a member function object's `MemberCall` (runs inside GC: no JSC
/// calls, no Rust destructors beyond the deallocation).
pub(crate) unsafe extern "C" fn member_finalize(object: sys::JSObjectRef) {
    let p = unsafe { sys::JSObjectGetPrivate(object) } as *mut MemberPayload;
    if !p.is_null() {
        unsafe { drop(Box::from_raw(p)) };
    }
}

unsafe fn method_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let data = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::method` from exactly this fn-pointer type.
    let f: MethodFn<C> = unsafe { std::mem::transmute(f) };
    f(scope, data, args).map(|v| v.raw)
}

unsafe fn getter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    _args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let data = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::getter`/`accessor` from this fn-pointer type.
    let f: GetterFn<C> = unsafe { std::mem::transmute(f) };
    f(scope, data).map(|v| v.raw)
}

unsafe fn setter_invoke<C: JsClass>(
    scope: &Scope<'_>,
    this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    let data = unsafe { this_data::<C>(scope, this) }?;
    // SAFETY: stored by `ClassDef::accessor` from this fn-pointer type.
    let f: SetterFn<C> = unsafe { std::mem::transmute(f) };
    f(scope, data, args.get(0))?;
    Ok(unsafe { sys::JSValueMakeUndefined(scope.ctx) })
}

unsafe fn static_invoke(
    scope: &Scope<'_>,
    _this: sys::JSObjectRef,
    args: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    // SAFETY: stored by `ClassDef::static_method` from this fn-pointer type.
    let f: StaticFn = unsafe { std::mem::transmute(f) };
    f(scope, args).map(|v| v.raw)
}

/// Called by the realm's JS constructor shim as `native(newTarget, argsArray)`.
unsafe fn ctor_invoke(
    scope: &Scope<'_>,
    _this: sys::JSObjectRef,
    a: Args<'_>,
    f: *const (),
) -> JsResult<sys::JSValueRef> {
    // SAFETY: stored by `ClassDef::constructor` from this fn-pointer type.
    let f: CtorFn = unsafe { std::mem::transmute(f) };
    let ctx = scope.ctx;
    let new_target = a.get(0).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
    let list = a.get(1).as_object().ok_or_else(|| JsError::Type("Illegal constructor".into()))?;
    let len = list.get("length")?.to_number()? as usize;
    // Arguments stay reachable through `list` (a live argument array).
    let mut raw = Vec::with_capacity(len);
    for i in 0..len {
        raw.push(list.get_index(i as u32)?.raw);
    }
    let args = Args { ctx, raw: &raw };
    f(scope, new_target, args).map(|o| o.raw as sys::JSValueRef)
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

/// Whether a window's named properties object may have a name. Runs for every
/// name the global lacks, so it stays out of `guard`.
pub(crate) unsafe extern "C" fn named_properties_filter(
    ctx: sys::JSContextRef,
    characters: *const std::ffi::c_void,
    length: usize,
    is_8bit: bool,
) -> bool {
    let name = unsafe {
        if is_8bit {
            crate::realm::NameUnits::Latin1(std::slice::from_raw_parts(characters.cast(), length))
        } else {
            crate::realm::NameUnits::Utf16(std::slice::from_raw_parts(characters.cast(), length))
        }
    };
    catch_unwind(AssertUnwindSafe(|| {
        let realm = crate::realm::realm_for_ctx(ctx)?;
        let named = realm.named_properties()?;
        Some(named.may_resolve(name))
    }))
    .ok()
    .flatten()
    .unwrap_or(false)
}

/// `[[Get]]` of a window's named properties object, for names the filter let
/// through. `ctx` is the window's own, whichever realm is asking.
pub(crate) unsafe extern "C" fn named_properties_get(
    ctx: sys::JSContextRef,
    object: sys::JSObjectRef,
    name: sys::JSStringRef,
    exc: *mut sys::JSValueRef,
) -> sys::JSValueRef {
    unsafe {
        guard(ctx, exc, ptr::null(), |scope| {
            // The named property visibility algorithm: Object.prototype wins.
            let Some(named) = scope.realm.named_properties() else { return Ok(ptr::null()) };
            if on_prototype_chain(ctx, object, name) {
                return Ok(ptr::null());
            }
            let value = named.resolve(scope, &String::from_utf16_lossy(utf16_of(name)))?;
            Ok(value.map_or(ptr::null(), |v| v.raw))
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
